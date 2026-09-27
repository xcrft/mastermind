//! An explicit intake-to-task handoff. The local marker is written before SQL
//! publication so a crash cannot silently downgrade this task to an unbound run.
//! Both stores contain only source identifiers and digests, never prompt copies.

use super::*;
use crate::bounded_fs::{AtomicWriteExpectation, RootCapability, StableFileIdentity};
use crate::miner::hooks::refiner::{Action, Intent};
use intake::IntakeReceipt;

const LIMIT: u64 = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskBinding {
    schema: u32,
    pub revision: String,
    pub intake_id: String,
    pub session_id: String,
    client: String,
    project_root: String,
    repository_identity: String,
    pub spec_path: String,
    spec_sha256: String,
    origin_sha256: String,
    capture_generation: i64,
    session_epoch: u64,
    previous_revision: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    status: String,
    binding: TaskBinding,
}

fn marker_path(root: &Path, spec: &Path) -> PathBuf {
    crate::run_task::state_file_path(root, spec).with_extension("intake.json")
}

fn read_marker(
    root: &RootCapability,
    path: &Path,
) -> Result<Option<(Marker, StableFileIdentity)>, Error> {
    match bounded_fs::read_regular_file_with_capability(
        root,
        path,
        LIMIT,
        LIMIT,
        ReadControl::default(),
    ) {
        Ok(file) => {
            let marker: Marker = serde_json::from_slice(&file.bytes)?;
            if !matches!(marker.status.as_str(), "prepared" | "bound")
                || marker.binding.schema != 1
                || marker.binding.revision != revision(&marker.binding)?
            {
                return Err("intake task marker is invalid".into());
            }
            Ok(Some((marker, file.identity)))
        }
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn write_marker(
    root: &RootCapability,
    path: &Path,
    marker: &Marker,
    previous: Option<StableFileIdentity>,
) -> Result<(), Error> {
    let expectation = match previous {
        Some(identity) => AtomicWriteExpectation::File(identity),
        None => AtomicWriteExpectation::Missing(
            bounded_fs::inspect_absent_path(root, path, ReadControl::default())?
                .ok_or("task handoff appeared concurrently")?,
        ),
    };
    bounded_fs::write_atomic_regular_file_expected_with_capability(
        root,
        path,
        &serde_json::to_vec_pretty(marker)?,
        true,
        expectation,
    )?;
    Ok(())
}

fn revision(binding: &TaskBinding) -> Result<String, Error> {
    let mut value = serde_json::to_value(binding)?;
    value["revision"] = json!("");
    Ok(hash(&json!(["intake-task-v1", value])))
}

fn origin(receipt: &IntakeReceipt) -> Result<String, Error> {
    if receipt.status != "offered" || receipt.schema != 1 {
        return Err("only an offered intake can be bound".into());
    }
    let response = receipt.response.as_ref().ok_or("intake has no response")?;
    super::super::refiner::parse_response(&receipt.input, &serde_json::to_vec(response)?)?;
    if response.action == Action::Ask
        || !matches!(
            response.workflow_intent,
            Intent::ActivateMastermind | Intent::ContinueActive
        )
    {
        return Err("intake does not request Mastermind task work".into());
    }
    // Tool observations and episode closure are deliberately not in this hash.
    Ok(hash(&json!([
        "intake-origin-v1",
        receipt.input,
        response,
        receipt.config_revision,
        receipt.session_epoch,
        receipt.task_binding_revision
    ])))
}

fn record(conn: &Connection, revision: &str) -> Result<Option<TaskBinding>, Error> {
    let data: Option<String> = conn
        .query_row(
            "SELECT data FROM hook_task_binding WHERE revision=?1",
            [revision],
            |r| r.get(0),
        )
        .optional()?;
    data.map(|data| Ok(serde_json::from_str(&data)?))
        .transpose()
}

fn source(conn: &Connection, binding: &TaskBinding) -> Result<IntakeReceipt, Error> {
    let data: String = conn.query_row(
        "SELECT data FROM hook_intake WHERE id=?1",
        [&binding.intake_id],
        |r| r.get(0),
    )?;
    let receipt: IntakeReceipt = serde_json::from_str(&data)?;
    if origin(&receipt)? != binding.origin_sha256
        || receipt.input.project_root != binding.project_root
        || receipt.input.session_id != binding.session_id
        || receipt.input.client != binding.client
        || receipt.input.capture_generation != binding.capture_generation
    {
        return Err("bound intake source changed".into());
    }
    let grant = read_grant(conn, &binding.client, Path::new(&binding.project_root))?
        .ok_or("intake capture grant is absent")?;
    if !grant.enabled
        || grant.generation != binding.capture_generation
        || grant.pending != 0
        || !grant.gap.is_empty()
    {
        return Err("bound intake capture is no longer current".into());
    }
    let ep = load_episode(conn, &receipt.input.episode_id)?;
    let session: Session = serde_json::from_str(&conn.query_row(
        "SELECT data FROM hook_session WHERE id=?1",
        [&binding.session_id],
        |r| r.get::<_, String>(0),
    )?)?;
    if session.capture_version != CAPTURE_VERSION
        || !session.gaps.is_empty()
        || session.id != binding.session_id
        || session.project_root != binding.project_root
        || session.client != binding.client
        || session.generation != binding.capture_generation
        || ep.session != binding.session_id
        || ep.generation != binding.capture_generation
        || !ep.gaps.is_empty()
        || !ep.events.iter().any(|event| {
            event.id == receipt.input.event_id
                && event.kind == "UserPromptSubmit"
                && event.origin == "user_channel_unverified"
                && event.text == receipt.input.original
                && super::super::refiner::prompt_digest(&event.text) == receipt.input.prompt_digest
        })
    {
        return Err("bound intake evidence is incomplete".into());
    }
    Ok(receipt)
}

fn task_identity(root: &RootCapability, spec: &Path) -> Result<(String, String, String), Error> {
    let relative =
        bounded_fs::normalize_repository_relative_path(&root.repository_relative(spec)?)?;
    let file = bounded_fs::read_regular_file_with_capability(
        root,
        Path::new(&relative),
        LIMIT,
        LIMIT,
        ReadControl::default(),
    )?;
    let text = std::str::from_utf8(&file.bytes)?;
    if text.trim().is_empty() {
        return Err("task spec is empty".into());
    }
    let repository = crate::facts::repository_identity(root.canonical_root())?;
    root.verify()?;
    Ok((repository, relative, crate::run_task::hash_text(text)))
}

fn validate_task(root: &RootCapability, binding: &TaskBinding) -> Result<(), Error> {
    let identity = task_identity(root, Path::new(&binding.spec_path))?;
    if root.canonical_root().to_string_lossy() != binding.project_root
        || identity
            != (
                binding.repository_identity.clone(),
                binding.spec_path.clone(),
                binding.spec_sha256.clone(),
            )
    {
        return Err("bound task changed; bind a current intake to the reviewed spec".into());
    }
    if crate::miner::hooks::fence::pending(&binding.client, root.canonical_root())? {
        return Err("intake capture delivery is incomplete".into());
    }
    Ok(())
}

/// Called before taking a journal writer transaction. Filesystem and Git work
/// never run while holding the SQL writer. The caller compares the epoch again.
pub(super) fn active(
    conn: &Connection,
    session_id: &str,
) -> Result<(u64, Option<TaskBinding>), Error> {
    let session: Session = serde_json::from_str(&conn.query_row(
        "SELECT data FROM hook_session WHERE id=?1",
        [session_id],
        |r| r.get::<_, String>(0),
    )?)?;
    let result = (|| -> Result<TaskBinding, Error> {
        if !session.started || !session.gaps.is_empty() {
            return Err("session is not active".into());
        }
        let binding = record(
            conn,
            session.task_binding.as_deref().ok_or("no bound task")?,
        )?
        .ok_or("bound task is absent")?;
        let root = RootCapability::open(Path::new(&session.project_root))?;
        validate_task(&root, &binding)?;
        source(conn, &binding)?;
        let marker = read_marker(
            &root,
            &marker_path(root.canonical_root(), Path::new(&binding.spec_path)),
        )?
        .ok_or("task handoff is incomplete")?
        .0;
        if marker.status != "bound" || marker.binding != binding {
            return Err("task binding changed".into());
        }
        let state = crate::run_task::load_state(&crate::run_task::state_file_path(
            root.canonical_root(),
            Path::new(&binding.spec_path),
        ))?;
        if state.is_some_and(|state| state.status == "learned") {
            return Err("task is completed".into());
        }
        Ok(binding)
    })();
    Ok((session.task_epoch, result.ok()))
}

pub(super) fn epoch(conn: &Connection, session: &str) -> Result<u64, Error> {
    let session: Session = serde_json::from_str(&conn.query_row(
        "SELECT data FROM hook_session WHERE id=?1",
        [session],
        |r| r.get::<_, String>(0),
    )?)?;
    Ok(session.task_epoch)
}

pub(in crate::miner) fn bind(
    id: &str,
    spec: &Path,
    repo: &Path,
    expected: Option<&str>,
) -> Result<Value, Error> {
    let root = RootCapability::open(repo)?;
    let (_, relative, _) = task_identity(&root, spec)?;
    let state_path = crate::run_task::state_file_path(root.canonical_root(), Path::new(&relative));
    let _controller = crate::run_task::controller_lock(root.canonical_root(), &state_path)?;
    let identity = task_identity(&root, Path::new(&relative))?;
    let path = marker_path(root.canonical_root(), Path::new(&relative));
    let old = read_marker(&root, &path)?;
    let mut db = Journal::open(true)?;
    // A missing local marker does not erase SQL history or its compare-and-swap
    // boundary. Only an explicit retry of the latest binding can restore it.
    let missing_predecessor = if old.is_none() {
        let saved: Option<String> = db.conn.query_row(
            "SELECT data FROM hook_task_binding WHERE project_root=?1 AND spec_path=?2 ORDER BY rowid DESC LIMIT 1",
            params![root.canonical_root().to_string_lossy(), relative],
            |row| row.get(0),
        ).optional()?;
        saved
            .map(|data| serde_json::from_str::<TaskBinding>(&data))
            .transpose()?
    } else {
        None
    };
    if let Some(binding) = &missing_predecessor {
        if binding.schema != 1
            || binding.revision != revision(binding)?
            || binding.project_root != root.canonical_root().to_string_lossy()
            || binding.spec_path != relative
        {
            return Err("saved task binding is invalid".into());
        }
        if expected != Some(binding.revision.as_str()) {
            return Err("task intake marker is missing; recovery requires --expected-binding for the latest journal binding".into());
        }
        if binding.intake_id == id {
            validate_task(&root, binding)?;
            write_marker(
                &root,
                &path,
                &Marker {
                    status: "bound".into(),
                    binding: binding.clone(),
                },
                None,
            )?;
            return Ok(json!({"status":"bound","recovered":true,"binding":binding}));
        }
    }
    // An exact retry only finishes the local publication. It must not reactivate
    // an ended session or move a newer session pointer back to this task.
    if let Some((marker, file)) = &old {
        if marker.binding.intake_id == id {
            if record(&db.conn, &marker.binding.revision)?.as_ref() == Some(&marker.binding) {
                if marker.status == "prepared" {
                    write_marker(
                        &root,
                        &path,
                        &Marker {
                            status: "bound".into(),
                            binding: marker.binding.clone(),
                        },
                        Some(*file),
                    )?;
                }
                return Ok(json!({"status":"bound","binding":marker.binding}));
            }
        } else if expected != Some(marker.binding.revision.as_str()) {
            return Err(
                "task already has an intake; replacement requires --expected-binding".into(),
            );
        }
    } else if expected.is_some() && missing_predecessor.is_none() {
        return Err("expected task binding is absent".into());
    }
    let used: bool = db.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM hook_task_binding WHERE intake=?1)",
        [id],
        |r| r.get(0),
    )?;
    if used {
        return Err("intake is already bound to a different task or superseded binding".into());
    }
    let receipt = db.intake(id)?;
    let origin_sha256 = origin(&receipt)?;
    if receipt.input.project_root != root.canonical_root().to_string_lossy() {
        return Err("intake belongs to another project".into());
    }
    let grant = db
        .grant(&receipt.input.client, root.canonical_root())?
        .ok_or("capture is not configured")?;
    if crate::miner::hooks::fence::pending(&grant.client, root.canonical_root())?
        || !intake::admitted_prompt(&db.conn, &grant, &receipt.input, true)?
        || epoch(&db.conn, &receipt.input.session_id)? != receipt.session_epoch
    {
        return Err("intake is no longer the current admitted prompt".into());
    }
    let predecessor = old
        .as_ref()
        .map(|(marker, _)| {
            if marker.status == "prepared" && marker.binding.intake_id == id {
                marker.binding.previous_revision.as_deref()
            } else {
                Some(marker.binding.revision.as_str())
            }
        })
        .unwrap_or_else(|| {
            missing_predecessor
                .as_ref()
                .map(|binding| binding.revision.as_str())
        });
    if receipt
        .response
        .as_ref()
        .is_some_and(|r| r.workflow_intent == Intent::ContinueActive)
        && (receipt.input.active_task.as_deref() != Some(relative.as_str())
            || receipt.task_binding_revision.as_deref() != predecessor)
    {
        return Err("continuation may only bind its exact active task".into());
    }
    let mut binding = TaskBinding {
        schema: 1,
        revision: String::new(),
        intake_id: id.into(),
        session_id: receipt.input.session_id.clone(),
        client: grant.client.clone(),
        project_root: grant.project_root.clone(),
        repository_identity: identity.0,
        spec_path: identity.1,
        spec_sha256: identity.2,
        origin_sha256,
        capture_generation: grant.generation,
        session_epoch: receipt
            .session_epoch
            .checked_add(1)
            .ok_or("task epoch exhausted")?,
        previous_revision: old
            .as_ref()
            .map(|(marker, _)| marker.binding.revision.clone())
            .or_else(|| {
                missing_predecessor
                    .as_ref()
                    .map(|binding| binding.revision.clone())
            }),
    };
    // The prepared retry retains its original predecessor, hence its revision.
    if let Some((marker, _)) = &old {
        if marker.binding.intake_id == id {
            binding.previous_revision = marker.binding.previous_revision.clone();
        }
    }
    binding.revision = revision(&binding)?;
    let prepared = Marker {
        status: "prepared".into(),
        binding: binding.clone(),
    };
    write_marker(&root, &path, &prepared, old.as_ref().map(|(_, file)| *file))?;
    validate_task(&root, &binding)?;
    let prepared_file = read_marker(&root, &path)?.ok_or("prepared task handoff disappeared")?;
    if prepared_file.0.binding != binding {
        return Err("prepared task handoff changed".into());
    }
    let tx = db
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !intake::admitted_prompt(&tx, &grant, &receipt.input, true)?
        || epoch(&tx, &binding.session_id)? != receipt.session_epoch
        || source(&tx, &binding)?.input != receipt.input
    {
        return Err("intake admission changed before handoff".into());
    }
    tx.execute("INSERT INTO hook_task_binding(revision,intake,project_root,spec_path,data) VALUES(?1,?2,?3,?4,?5)",
        params![binding.revision, binding.intake_id, binding.project_root, binding.spec_path, serde_json::to_string(&binding)?])?;
    let mut session: Session = serde_json::from_str(&tx.query_row(
        "SELECT data FROM hook_session WHERE id=?1",
        [&binding.session_id],
        |r| r.get::<_, String>(0),
    )?)?;
    session.task_binding = Some(binding.revision.clone());
    session.task_epoch = binding.session_epoch;
    save_session(&tx, &session)?;
    tx.commit()?;
    // A failure leaves a prepared barrier, never a runnable unbound task.
    validate_task(&root, &binding)?;
    write_marker(
        &root,
        &path,
        &Marker {
            status: "bound".into(),
            binding: binding.clone(),
        },
        Some(prepared_file.1),
    )?;
    Ok(json!({"status":"bound","binding":binding}))
}

/// Live source validation for new work. Historical completed receipts do not
/// call this function: forgetting source text must not rewrite past outcomes.
pub(in crate::miner) fn load(root: &Path, spec: &Path) -> Result<Option<(String, Value)>, Error> {
    let root = RootCapability::open(root)?;
    let path = marker_path(root.canonical_root(), spec);
    let Some((marker, _)) = read_marker(&root, &path)? else {
        if super::path()?.exists() {
            let db = Journal::open(false)?;
            let exists: bool = db.conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='hook_task_binding')", [], |r| r.get(0))?;
            if exists {
                let relative = bounded_fs::normalize_repository_relative_path(
                    &root.repository_relative(spec)?,
                )?;
                let known: bool = db.conn.query_row("SELECT EXISTS(SELECT 1 FROM hook_task_binding WHERE project_root=?1 AND spec_path=?2)",
                    params![root.canonical_root().to_string_lossy(), relative], |r| r.get(0))?;
                if known {
                    return Err(
                        "task intake marker disappeared; restore or explicitly replace it".into(),
                    );
                }
            }
        }
        return Ok(None);
    };
    if marker.status != "bound" {
        return Err("intake task handoff is prepared but incomplete".into());
    }
    let db = Journal::open(false)?;
    if record(&db.conn, &marker.binding.revision)?.as_ref() != Some(&marker.binding) {
        return Err("intake task handoff has no matching journal receipt".into());
    }
    validate_task(&root, &marker.binding)?;
    let receipt = source(&db.conn, &marker.binding)?;
    Ok(Some((
        marker.binding.revision.clone(),
        json!({
            "schema":1,"binding":marker.binding,"original":receipt.input.original,
            "response":receipt.response,"authority":"source_data_only","semantic_preservation":"unverified"
        }),
    )))
}
