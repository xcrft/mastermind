//! Explicit, bounded background mining. Existing journal leases and completed
//! checkpoints remain the only work queue. No login service is installed.

use super::{hash, journal::Journal, worker, Error, EXTRACTOR};
use crate::bounded_fs::{
    self, AtomicWriteExpectation, BoundedReadError, ReadControl, RootCapability, StableFileIdentity,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Child;
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DOCUMENT_BYTES: u64 = 64 * 1024;
const EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(100);
const HEARTBEAT: Duration = Duration::from_secs(2);
const START_WAIT: Duration = Duration::from_secs(8);
const STOP_WAIT: Duration = Duration::from_secs(5);
const RECORD_READ_ATTEMPTS: usize = 3;

#[derive(Clone, Debug, Default)]
pub struct StartOptions {
    pub processor: Option<PathBuf>,
    pub provider: Option<String>,
    pub args: Vec<String>,
    pub timeout: Option<u64>,
    pub limit: Option<usize>,
    pub max_calls: Option<u64>,
    pub max_runtime: Option<u64>,
}

pub(super) trait BatchControl {
    fn client(&self) -> &str;
    fn before_attempt(&mut self) -> Result<(), Error>;
    fn before_publish(&mut self) -> Result<(), Error>;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Settings {
    processor: Option<PathBuf>,
    provider: Option<String>,
    args: Vec<String>,
    timeout: u64,
    limit: usize,
    max_calls: u64,
    max_runtime: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Config {
    schema: u32,
    worker_id: String,
    client: String,
    project_root: PathBuf,
    capture_generation: i64,
    settings: Settings,
    processor: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema: u32,
    worker_id: String,
    run_id: String,
    config_revision: String,
    status: String,
    reason: Option<String>,
    pid: Option<u32>,
    started_at_ms: u64,
    updated_at_ms: u64,
    attempts: u64,
    completed: u64,
    drafts: u64,
    next_after: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopRequest {
    schema: u32,
    run_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pulse {
    schema: u32,
    run_id: String,
    at_ms: u64,
}

struct Stored<T> {
    value: T,
    identity: StableFileIdentity,
}

struct Store {
    root: RootCapability,
}

impl Store {
    fn directory(id: &str) -> Result<PathBuf, Error> {
        check_hex(id, 64)?;
        Ok(std::env::home_dir()
            .ok_or("worker_home_unavailable")?
            .join(".mastermind/persona-workers")
            .join(id))
    }

    fn prepare(id: &str) -> Result<Self, Error> {
        let (root, _) = bounded_fs::prepare_file_target(&Self::directory(id)?.join("config.json"))?;
        root.set_root_directory_mode(0o700)?;
        Ok(Self { root })
    }

    fn existing(id: &str) -> Result<Option<Self>, Error> {
        match RootCapability::open(&Self::directory(id)?) {
            Ok(root) => Ok(Some(Self { root })),
            Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.canonical_root().join(name)
    }

    fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<Stored<T>>, Error> {
        self.read_control(
            name,
            ReadControl {
                deadline: Some(Instant::now() + Duration::from_secs(2)),
                interrupted: None,
            },
        )
    }

    fn read_control<T: DeserializeOwned>(
        &self,
        name: &str,
        control: ReadControl<'_>,
    ) -> Result<Option<Stored<T>>, Error> {
        let limit = Instant::now() + Duration::from_secs(2);
        let control = ReadControl {
            deadline: Some(
                control
                    .deadline
                    .map_or(limit, |deadline| deadline.min(limit)),
            ),
            ..control
        };
        let mut attempt = 0;
        let file = loop {
            attempt += 1;
            match bounded_fs::read_regular_file_with_capability(
                &self.root,
                &self.path(name),
                DOCUMENT_BYTES,
                DOCUMENT_BYTES,
                control,
            ) {
                Ok(file) => break file,
                Err(BoundedReadError::SnapshotChanged) if attempt < RECORD_READ_ATTEMPTS => {
                    // Mutable direct records are atomically replaced. Re-read
                    // the entire record, under the same root and deadline.
                    self.root.verify()?;
                }
                Err(BoundedReadError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            }
        };
        let json = crate::setup::parse_json_unique(&file.bytes)
            .map_err(|_| "worker_record_invalid_json")?;
        Ok(Some(Stored {
            value: serde_json::from_value(json).map_err(|_| "worker_record_invalid_schema")?,
            identity: file.identity,
        }))
    }

    fn write<T: Serialize>(
        &self,
        name: &str,
        value: &T,
        previous: Option<StableFileIdentity>,
    ) -> Result<StableFileIdentity, Error> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() as u64 > DOCUMENT_BYTES {
            return Err("worker_record_too_large".into());
        }
        let path = self.path(name);
        let expectation = match previous {
            Some(identity) => AtomicWriteExpectation::File(identity),
            None => AtomicWriteExpectation::Missing(
                bounded_fs::inspect_absent_path(&self.root, &path, ReadControl::default())?
                    .ok_or("worker_record_changed")?,
            ),
        };
        bounded_fs::write_atomic_regular_file_expected_with_capability(
            &self.root,
            &path,
            &bytes,
            true,
            expectation,
        )?;
        Ok(bounded_fs::read_regular_file_with_capability(
            &self.root,
            &path,
            DOCUMENT_BYTES,
            0,
            ReadControl::default(),
        )?
        .identity)
    }

    fn lifecycle(&self) -> Result<bounded_fs::StableFileLock, Error> {
        bounded_fs::try_locked_regular_file_with_capability(
            &self.root,
            &self.path("lifecycle.lock"),
        )
        .map_err(|_| "worker_lifecycle_busy_or_unavailable".into())
    }

    fn owner(&self) -> Result<Option<bounded_fs::StableFileLock>, Error> {
        Ok(
            bounded_fs::try_locked_existing_regular_file_with_capability(
                &self.root,
                &self.path("owner.lock"),
            )?,
        )
    }

    fn request_stop(&self, run_id: &str) -> Result<(), Error> {
        let old = self.read::<StopRequest>("stop.json")?;
        self.write(
            "stop.json",
            &StopRequest {
                schema: 1,
                run_id: run_id.into(),
            },
            old.map(|record| record.identity),
        )?;
        Ok(())
    }

    fn stopped(&self, run_id: &str) -> Result<bool, Error> {
        self.stopped_control(run_id, ReadControl::default())
    }

    fn stopped_control(&self, run_id: &str, control: ReadControl<'_>) -> Result<bool, Error> {
        let stop = self.read_control::<StopRequest>("stop.json", control)?;
        if let Some(stop) = stop {
            if stop.value.schema != 1 {
                return Err("worker_stop_record_invalid".into());
            }
            check_hex(&stop.value.run_id, 32)?;
            return Ok(stop.value.run_id == run_id);
        }
        Ok(false)
    }
}

fn now() -> Result<u64, Error> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}

fn check_hex(value: &str, size: usize) -> Result<(), Error> {
    if value.len() != size
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("worker_identifier_invalid".into());
    }
    Ok(())
}

fn slot(client: &str, root: &Path) -> Result<(String, PathBuf), Error> {
    super::client(client)?;
    let root = root.canonicalize()?;
    if !root.is_dir() || root.to_str().is_none() {
        return Err("worker_project_root_invalid".into());
    }
    Ok((hash(&json!(["persona-worker-slot-v1", client, root])), root))
}

fn platform() -> Result<(), Error> {
    if !cfg!(unix) {
        return Err(
            "background semantic mining currently requires macOS or Linux process supervision"
                .into(),
        );
    }
    Ok(())
}

fn settings(options: StartOptions, previous: Option<&Settings>) -> Result<Settings, Error> {
    let has_selector = options.processor.is_some() || options.provider.is_some();
    if !has_selector && (!options.args.is_empty() || previous.is_none()) {
        return Err("worker_requires_an_explicit_processor_or_saved_configuration".into());
    }
    let saved = previous;
    let result = Settings {
        processor: if has_selector {
            options.processor
        } else {
            saved.and_then(|s| s.processor.clone())
        },
        provider: if has_selector {
            options.provider
        } else {
            saved.and_then(|s| s.provider.clone())
        },
        args: if has_selector {
            options.args
        } else {
            saved.map(|s| s.args.clone()).unwrap_or_default()
        },
        timeout: options.timeout.or(saved.map(|s| s.timeout)).unwrap_or(60),
        limit: options.limit.or(saved.map(|s| s.limit)).unwrap_or(4),
        max_calls: options
            .max_calls
            .or(saved.map(|s| s.max_calls))
            .unwrap_or(64),
        max_runtime: options
            .max_runtime
            .or(saved.map(|s| s.max_runtime))
            .unwrap_or(3600),
    };
    if !(1..=120).contains(&result.timeout)
        || !(1..=16).contains(&result.limit)
        || !(1..=10_000).contains(&result.max_calls)
        || !(1..=86_400).contains(&result.max_runtime)
    {
        return Err("worker_budget_out_of_range".into());
    }
    validate_selection(
        result.processor.as_deref(),
        result.provider.as_deref(),
        &result.args,
    )?;
    Ok(result)
}

fn validate_selection(
    processor: Option<&Path>,
    provider: Option<&str>,
    args: &[String],
) -> Result<(), Error> {
    match (processor, provider) {
        (Some(path), None) if path.is_absolute() => {}
        (None, Some("claude")) if args.is_empty() => {}
        _ => return Err("worker_processor_selection_invalid".into()),
    }
    if args.len() > 32
        || args.iter().map(String::len).sum::<usize>() > 32 * 1024
        || args.iter().any(|arg| {
            let lowered = arg.to_ascii_lowercase();
            arg.len() > 8192
                || arg.contains('\0')
                || super::super::feedback::looks_secret(arg)
                || [
                    "api-key",
                    "api_key",
                    "access-token",
                    "access_token",
                    "authorization",
                    "password",
                    "secret=",
                ]
                .iter()
                .any(|name| lowered.contains(name))
        })
    {
        return Err("worker_arguments_oversized_or_credential_like".into());
    }
    Ok(())
}

struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Shared by explicit and managed batches. A replaced executable is a new
/// processor identity. Transitive script dependencies and remote model versions
/// are outside this local executable binding.
pub(super) fn processor_fingerprint(
    root: &Path,
    processor: Option<&Path>,
    provider: Option<&str>,
    args: &[String],
) -> Result<Value, Error> {
    validate_selection(processor, provider, args)?;
    let path = match processor {
        Some(path) => path.canonicalize()?,
        None => crate::setup::resolve_native_cli("claude", root)
            .map_err(|_| "worker_provider_executable_unavailable")?,
    };
    let parent = RootCapability::open(path.parent().ok_or("worker_executable_invalid")?)?;
    let mut writer = HashWriter(Sha256::new());
    let file = bounded_fs::copy_regular_file_with_capability(
        &parent,
        &path,
        EXECUTABLE_BYTES,
        ReadControl {
            deadline: Some(Instant::now() + Duration::from_secs(10)),
            interrupted: Some(&worker::interrupted),
        },
        None,
        &mut writer,
    )?;
    #[cfg(unix)]
    if file.identity.attributes() & 0o111 == 0 {
        return Err("worker_processor_not_executable".into());
    }
    Ok(json!({
        "path":processor.map(|_| &path),"provider":provider,
        "arguments_digest":hash(&json!(args)),"protocol":EXTRACTOR,
        "executable":{"path":path,"sha256":crate::hex::encode(&writer.0.finalize()),"bytes":file.declared_len}
    }))
}

fn validate_config(config: &Config, id: &str) -> Result<(), Error> {
    if config.schema != 1
        || config.worker_id != id
        || config.capture_generation <= 0
        || slot(&config.client, &config.project_root)?.0 != id
    {
        return Err("worker_configuration_binding_invalid".into());
    }
    settings(StartOptions::default(), Some(&config.settings))?;
    Ok(())
}

fn revision(config: &Config) -> Result<String, Error> {
    Ok(hash(&serde_json::to_value(config)?))
}

fn validate_state(state: &State, config: &Config) -> Result<(), Error> {
    check_hex(&state.run_id, 32)?;
    if state.schema != 1
        || state.worker_id != config.worker_id
        || state.config_revision != revision(config)?
        || state.attempts > config.settings.max_calls
        || !matches!(
            state.status.as_str(),
            "starting" | "running" | "stopped" | "failed" | "budget_exhausted"
        )
    {
        return Err("worker_state_binding_invalid".into());
    }
    Ok(())
}

pub fn status(client: &str, root: &Path) -> Result<Value, Error> {
    platform()?;
    let (id, root) = slot(client, root)?;
    let Some(store) = Store::existing(&id)? else {
        return Ok(
            json!({"schema":1,"worker_id":id,"client":client,"project_root":root,"status":"not_configured","autostart":false}),
        );
    };
    status_at(&store, &id)
}

fn status_at(store: &Store, id: &str) -> Result<Value, Error> {
    let config = store
        .read::<Config>("config.json")?
        .ok_or("worker_configuration_missing")?;
    validate_config(&config.value, id)?;
    let state = store
        .read::<State>("state.json")?
        .ok_or("worker_state_missing")?;
    validate_state(&state.value, &config.value)?;
    // Do not retain an available-lock probe during heartbeat/stop record I/O.
    let held = store.owner()?.is_none();
    let pulse = store.read::<Pulse>("heartbeat.json")?;
    let pulse = pulse
        .map(|record| record.value)
        .filter(|pulse| pulse.schema == 1 && pulse.run_id == state.value.run_id);
    let age = pulse
        .as_ref()
        .map(|pulse| now().map(|time| time.saturating_sub(pulse.at_ms)))
        .transpose()?;
    // The child publishes its PID before its watchdog writes the first pulse.
    // Apply the same grace period to that startup gap as to a delayed pulse.
    let unresponsive = age.unwrap_or(now()?.saturating_sub(state.value.started_at_ms)) > 10_000;
    let active = matches!(state.value.status.as_str(), "starting" | "running");
    let observed = if !held && active {
        "interrupted"
    } else if held && store.stopped(&state.value.run_id)? {
        "stopping"
    } else if held && state.value.status == "running" && unresponsive {
        "unresponsive"
    } else {
        &state.value.status
    };
    Ok(json!({
        "schema":1,"worker_id":id,"client":config.value.client,"project_root":config.value.project_root,
        "status":observed,"owner":if held {"held"} else {"available"},
        "config_revision":revision(&config.value)?,
        "configuration":{"processor":config.value.processor,"timeout":config.value.settings.timeout,"limit":config.value.settings.limit,
            "max_calls":config.value.settings.max_calls,"max_runtime":config.value.settings.max_runtime,"capture_generation":config.value.capture_generation},
        "run":state.value,"heartbeat_age_ms":age,"autostart":false,
        "checkpoint_store":"persona-events.db:hook_analysis","output":"unreviewed_drafts_only",
        "permission_effect":"none","retry_policy":"stop_on_processor_failure_explicit_restart"
    }))
}

pub fn start(client: &str, root: &Path, options: StartOptions) -> Result<Value, Error> {
    platform()?;
    let (id, root) = slot(client, root)?;
    let grant = Journal::open(false)?
        .grant(client, &root)?
        .ok_or("worker_capture_not_configured")?;
    if !grant.enabled {
        return Err("worker_capture_revoked".into());
    }
    let store = Store::prepare(&id)?;
    let _lifecycle = store.lifecycle()?;
    let previous = store.read::<Config>("config.json")?;
    if let Some(previous) = &previous {
        validate_config(&previous.value, &id)?;
    }
    let mut selected = settings(
        options,
        previous.as_ref().map(|value| &value.value.settings),
    )?;
    let fingerprint = processor_fingerprint(
        &root,
        selected.processor.as_deref(),
        selected.provider.as_deref(),
        &selected.args,
    )?;
    if selected.processor.is_some() {
        selected.processor = Some(PathBuf::from(
            fingerprint["executable"]["path"]
                .as_str()
                .ok_or("worker_executable_invalid")?,
        ));
    }
    let config = Config {
        schema: 1,
        worker_id: id.clone(),
        client: client.into(),
        project_root: root,
        capture_generation: grant.generation,
        settings: selected,
        processor: fingerprint,
    };
    let owner = acquire_start_owner(
        &store,
        previous.as_ref(),
        ReadControl {
            deadline: Some(Instant::now() + START_WAIT),
            interrupted: None,
        },
    )?;
    if owner.is_none() {
        if previous
            .as_ref()
            .is_none_or(|previous| previous.value != config)
        {
            return Err("worker_running_configuration_differs_stop_first".into());
        }
        let mut report = status_at(&store, &id)?;
        report["started"] = json!(false);
        return Ok(report);
    }
    let previous_state = store.read::<State>("state.json")?;
    let config_identity =
        store.write("config.json", &config, previous.map(|value| value.identity))?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| "worker_run_id_unavailable")?;
    let run_id = crate::hex::encode(&nonce);
    let mut state = State {
        schema: 1,
        worker_id: id.clone(),
        run_id: run_id.clone(),
        config_revision: revision(&config)?,
        status: "starting".into(),
        reason: None,
        pid: None,
        started_at_ms: now()?,
        updated_at_ms: now()?,
        attempts: 0,
        completed: 0,
        drafts: 0,
        next_after: None,
    };
    let state_identity = store.write(
        "state.json",
        &state,
        previous_state.map(|value| value.identity),
    )?;
    drop(owner);
    let mut child = match spawn(&id, &run_id, &config.project_root) {
        Ok(child) => child,
        Err(error) => {
            state.status = "failed".into();
            state.reason = Some("worker_spawn_failed".into());
            store.write("state.json", &state, Some(state_identity))?;
            return Err(error);
        }
    };
    let deadline = Instant::now() + START_WAIT;
    let child_id = child.id();
    let ready = || match ready_report(&store, &id, &run_id, child_id, config_identity) {
        Err(error) => {
            let _ = store.request_stop(&run_id);
            Err(error)
        }
        report => report,
    };
    loop {
        if let Some(report) = ready()? {
            return Ok(report);
        }
        if child.try_wait()?.is_some() {
            // A fast attempt may publish both ready and terminal state between
            // the previous observation and try_wait. Its bound terminal record
            // still proves startup, independently of whether mining succeeded.
            if let Some(report) = ready()? {
                return Ok(report);
            }
            return Err("worker_exited_before_ready_inspect_status".into());
        }
        if Instant::now() >= deadline {
            store.request_stop(&run_id)?;
            return Err("worker_start_confirmation_timed_out_stop_requested".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn acquire_start_owner(
    store: &Store,
    previous: Option<&Stored<Config>>,
    control: ReadControl<'_>,
) -> Result<Option<bounded_fs::StableFileLock>, Error> {
    loop {
        control.check()?;
        match store.owner() {
            Ok(Some(owner)) => return Ok(Some(owner)),
            Ok(None) => {
                if let Some(config) = previous {
                    let current = store
                        .read_control::<Config>("config.json", control)?
                        .ok_or("worker_configuration_missing")?;
                    if current.identity != config.identity {
                        return Err("worker_configuration_changed".into());
                    }
                    let state = store
                        .read_control::<State>("state.json", control)?
                        .ok_or("worker_state_missing")?;
                    validate_state(&state.value, &config.value)?;
                    if state.value.status == "running" && state.value.pid.is_some() {
                        return Ok(None);
                    }
                }
                // A terminal record or a child not yet admitted cannot prove
                // a running owner; a status reader may hold this brief probe.
            }
            Err(error) if missing(&error) && previous.is_none() => {
                return Ok(Some(bounded_fs::try_locked_regular_file_with_capability(
                    &store.root,
                    &store.path("owner.lock"),
                )?));
            }
            Err(error) => return Err(error),
        }
        wait_for_owner(control)?;
    }
}

fn ready_report(
    store: &Store,
    id: &str,
    run_id: &str,
    pid: u32,
    config_identity: StableFileIdentity,
) -> Result<Option<Value>, Error> {
    let config = store
        .read::<Config>("config.json")?
        .ok_or("worker_configuration_missing")?;
    validate_config(&config.value, id)?;
    if config.identity != config_identity {
        return Err("worker_configuration_changed".into());
    }
    let state = store
        .read::<State>("state.json")?
        .ok_or("worker_state_missing")?;
    validate_state(&state.value, &config.value)?;
    if state.value.run_id != run_id
        || state.value.pid != Some(pid)
        || state.value.status == "starting"
    {
        return Ok(None);
    }
    // Admission is read without probing the lock. Only after this child has
    // published its PID may status_at perform a liveness observation.
    let mut report = status_at(store, id)?;
    if store
        .read::<Config>("config.json")?
        .is_none_or(|current| current.identity != config_identity)
    {
        return Err("worker_configuration_changed".into());
    }
    if report["run"]["run_id"] != run_id
        || report["run"]["pid"] != pid
        || report["run"]["status"] == "starting"
    {
        return Ok(None);
    }
    report["started"] = json!(true);
    Ok(Some(report))
}

fn missing(error: &Error) -> bool {
    error.downcast_ref::<BoundedReadError>().is_some_and(|error| matches!(error, BoundedReadError::Io(inner) if inner.kind()==std::io::ErrorKind::NotFound))
}

#[cfg(unix)]
fn spawn(id: &str, run_id: &str, root: &Path) -> Result<Child, Error> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(std::env::current_exe()?.canonicalize()?);
    command
        .args([
            "miner", "hooks", "worker", "run", "--id", id, "--run-id", run_id,
        ])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is the only post-fork operation. No allocator or locks are
    // used in this closure. The child remains explicitly controlled by files.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    Ok(command.spawn()?)
}

#[cfg(not(unix))]
fn spawn(_id: &str, _run_id: &str, _root: &Path) -> Result<Child, Error> {
    Err("background semantic mining requires Unix".into())
}

pub fn stop(client: &str, root: &Path) -> Result<Value, Error> {
    platform()?;
    let (id, _) = slot(client, root)?;
    let Some(store) = Store::existing(&id)? else {
        return status(client, root);
    };
    let _lifecycle = store.lifecycle()?;
    let report = status_at(&store, &id)?;
    let run_id = report["run"]["run_id"]
        .as_str()
        .ok_or("worker_run_id_missing")?;
    if report["owner"] != "held" && report["run"]["status"] != "starting" {
        return Ok(report);
    }
    // A parent can exit while its admitted child is waiting for a status
    // probe to release owner.lock. Cancel that run before it acquires owner.
    store.request_stop(run_id)?;
    let deadline = Instant::now() + STOP_WAIT;
    loop {
        let mut report = status_at(&store, &id)?;
        report["stop_requested"] = json!(true);
        if report["owner"] != "held" || Instant::now() >= deadline {
            return Ok(report);
        }
        std::thread::sleep(POLL);
    }
}

struct Runtime {
    store: Store,
    config: Config,
    config_identity: StableFileIdentity,
    state: State,
    state_identity: StableFileIdentity,
    deadline: Instant,
    reason: Arc<Mutex<Option<String>>>,
}

impl Runtime {
    fn record(&mut self) -> Result<(), Error> {
        self.state.updated_at_ms = now()?;
        self.state_identity =
            self.store
                .write("state.json", &self.state, Some(self.state_identity))?;
        Ok(())
    }

    fn halt(&self, reason: &str) -> Error {
        set_reason(&self.reason, reason);
        worker::request_stop();
        reason.to_owned().into()
    }

    fn admission(&self) -> Result<(), Error> {
        if worker::interrupted() {
            return Err("worker_interrupted".into());
        }
        if Instant::now() >= self.deadline {
            return Err(self.halt("worker_runtime_budget_exhausted"));
        }
        if self.store.stopped(&self.state.run_id)? {
            return Err(self.halt("worker_stop_requested"));
        }
        let current = self
            .store
            .read::<Config>("config.json")?
            .ok_or("worker_configuration_missing")?;
        if current.identity != self.config_identity || current.value != self.config {
            return Err(self.halt("worker_configuration_changed"));
        }
        let grant = Journal::open(false)?
            .grant(&self.config.client, &self.config.project_root)?
            .ok_or("worker_capture_not_configured")?;
        if !grant.enabled || grant.generation != self.config.capture_generation {
            return Err(self.halt("worker_capture_revoked_or_restarted"));
        }
        let current = processor_fingerprint(
            &self.config.project_root,
            self.config.settings.processor.as_deref(),
            self.config.settings.provider.as_deref(),
            &self.config.settings.args,
        )?;
        if current != self.config.processor {
            return Err(self.halt("worker_processor_changed"));
        }
        Ok(())
    }
}

impl BatchControl for Runtime {
    fn client(&self) -> &str {
        &self.config.client
    }

    fn before_attempt(&mut self) -> Result<(), Error> {
        self.admission()?;
        if self.state.attempts >= self.config.settings.max_calls {
            return Err(self.halt("worker_call_budget_exhausted"));
        }
        // Charge conservatively before any model input. A crash after this
        // write may consume one unused reservation, never an uncounted call.
        self.state.attempts += 1;
        self.record()
    }

    fn before_publish(&mut self) -> Result<(), Error> {
        self.admission()
    }
}

fn set_reason(reason: &Mutex<Option<String>>, value: &str) {
    if let Ok(mut current) = reason.lock() {
        if current.is_none() {
            *current = Some(value.into());
        }
    }
}

struct Watchdog {
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    fn start(
        id: &str,
        run_id: &str,
        deadline: Instant,
        reason: Arc<Mutex<Option<String>>>,
    ) -> Result<Self, Error> {
        let store = Store::existing(id)?.ok_or("worker_store_missing")?;
        let project_config = store
            .read::<Config>("config.json")?
            .ok_or("worker_configuration_missing")?
            .value;
        let project = RootCapability::open(&project_config.project_root)?;
        let mut pulse_identity = store
            .read::<Pulse>("heartbeat.json")?
            .map(|pulse| pulse.identity);
        let run_id = run_id.to_owned();
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        let thread = std::thread::spawn(move || {
            let mut last_pulse = Instant::now() - HEARTBEAT;
            while !finished.load(Ordering::Relaxed) {
                let result = (|| -> Result<Option<&str>, Error> {
                    if Instant::now() >= deadline {
                        return Ok(Some("worker_runtime_budget_exhausted"));
                    }
                    if store.stopped(&run_id)? {
                        return Ok(Some("worker_stop_requested"));
                    }
                    project.verify()?;
                    if last_pulse.elapsed() >= HEARTBEAT {
                        let current = store
                            .read::<Config>("config.json")?
                            .ok_or("worker_configuration_missing")?;
                        if current.value != project_config {
                            return Ok(Some("worker_configuration_changed"));
                        }
                        let grant = Journal::open(false)?
                            .grant(&project_config.client, &project_config.project_root)?
                            .ok_or("worker_capture_not_configured")?;
                        if !grant.enabled || grant.generation != project_config.capture_generation {
                            return Ok(Some("worker_capture_revoked_or_restarted"));
                        }
                        pulse_identity = Some(store.write(
                            "heartbeat.json",
                            &Pulse {
                                schema: 1,
                                run_id: run_id.clone(),
                                at_ms: now()?,
                            },
                            pulse_identity,
                        )?);
                        last_pulse = Instant::now();
                    }
                    Ok(None)
                })();
                match result {
                    Ok(None) => {}
                    Ok(Some(stop)) => {
                        set_reason(&reason, stop);
                        worker::request_stop();
                        break;
                    }
                    Err(_) => {
                        set_reason(&reason, "worker_control_unavailable");
                        worker::request_stop();
                        break;
                    }
                }
                std::thread::sleep(POLL);
            }
        });
        Ok(Self {
            done,
            thread: Some(thread),
        })
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct StartingRecords {
    config: Stored<Config>,
    state: Stored<State>,
}

fn starting_records(
    store: &Store,
    id: &str,
    run_id: &str,
    control: ReadControl<'_>,
) -> Result<StartingRecords, Error> {
    let config = store
        .read_control::<Config>("config.json", control)?
        .ok_or("worker_configuration_missing")?;
    validate_config(&config.value, id)?;
    let state = store
        .read_control::<State>("state.json", control)?
        .ok_or("worker_state_missing")?;
    validate_state(&state.value, &config.value)?;
    if state.value.run_id != run_id || state.value.status != "starting" || state.value.pid.is_some()
    {
        return Err("worker_run_not_admitted".into());
    }
    if store.stopped_control(run_id, control)? {
        return Err("worker_stop_requested".into());
    }
    control.check()?;
    Ok(StartingRecords { config, state })
}

fn try_starting_owner(
    store: &Store,
    records: &StartingRecords,
    control: ReadControl<'_>,
) -> Result<Option<bounded_fs::StableFileLock>, Error> {
    let validate = || -> Result<(), Error> {
        let current = starting_records(
            store,
            &records.config.value.worker_id,
            &records.state.value.run_id,
            control,
        )?;
        if current.config.identity != records.config.identity
            || current.state.identity != records.state.identity
        {
            return Err("worker_starting_records_changed".into());
        }
        Ok(())
    };
    validate()?;
    let Some(owner) = store.owner()? else {
        // A status inspection may briefly own the lock. Only this still-bound
        // starting run may wait; no processor attempt has been reserved yet.
        return Ok(None);
    };
    validate()?;
    Ok(Some(owner))
}

fn wait_for_owner(control: ReadControl<'_>) -> Result<(), Error> {
    control.check()?;
    let remaining = control
        .deadline
        .ok_or("worker_start_deadline_missing")?
        .checked_duration_since(Instant::now())
        .ok_or(BoundedReadError::DeadlineExceeded)?;
    std::thread::sleep(remaining.min(Duration::from_millis(20)));
    Ok(())
}

fn acquire_starting_owner(
    store: &Store,
    id: &str,
    run_id: &str,
    control: ReadControl<'_>,
) -> Result<(bounded_fs::StableFileLock, StartingRecords), Error> {
    let records = starting_records(store, id, run_id, control)?;
    let elapsed = now()?.saturating_sub(records.state.value.started_at_ms);
    let remaining = records
        .config
        .value
        .settings
        .max_runtime
        .saturating_mul(1000)
        .saturating_sub(elapsed);
    let limit = Instant::now() + Duration::from_millis(remaining);
    let control = ReadControl {
        deadline: Some(
            control
                .deadline
                .ok_or("worker_start_deadline_missing")?
                .min(limit),
        ),
        ..control
    };
    loop {
        control.check()?;
        if let Some(owner) = try_starting_owner(store, &records, control)? {
            return Ok((owner, records));
        }
        wait_for_owner(control)?;
    }
}

pub fn run(id: &str, run_id: &str) -> Result<Value, Error> {
    platform()?;
    check_hex(id, 64)?;
    check_hex(run_id, 32)?;
    let store = Store::existing(id)?.ok_or("worker_store_missing")?;
    let _cancellation = worker::Cancellation::install()?;
    let (_owner, records) = acquire_starting_owner(
        &store,
        id,
        run_id,
        ReadControl {
            deadline: Some(Instant::now() + START_WAIT),
            interrupted: Some(&worker::interrupted),
        },
    )?;
    let StartingRecords { config, state } = records;
    let elapsed = now()?.saturating_sub(state.value.started_at_ms);
    let remaining = config
        .value
        .settings
        .max_runtime
        .saturating_mul(1000)
        .saturating_sub(elapsed);
    let deadline = Instant::now() + Duration::from_millis(remaining);
    let reason = Arc::new(Mutex::new(None));
    let mut runtime = Runtime {
        store,
        config: config.value,
        config_identity: config.identity,
        state: state.value,
        state_identity: state.identity,
        deadline,
        reason: reason.clone(),
    };
    runtime.state.status = "running".into();
    runtime.state.pid = Some(std::process::id());
    runtime.record()?;
    let outcome = (|| -> Result<(), Error> {
        let _watchdog = Watchdog::start(id, run_id, deadline, reason.clone())?;
        let observer = Journal::open(false)?;
        let mut last_version = None;
        let mut last_pass = Instant::now();
        let mut last_admission = Instant::now() - HEARTBEAT;
        while !worker::interrupted() {
            if last_admission.elapsed() >= HEARTBEAT {
                runtime.admission()?;
                last_admission = Instant::now();
            }
            if runtime.state.attempts >= runtime.config.settings.max_calls {
                return Err(runtime.halt("worker_call_budget_exhausted"));
            }
            let version = observer.data_version()?;
            if last_version != Some(version) || last_pass.elapsed() >= Duration::from_secs(30) {
                // Hash IDs are not chronological. Every pass, including after
                // restart, begins at None. Durable completed revisions dedupe.
                let mut after = None;
                loop {
                    let config = runtime.config.clone();
                    let remaining = config
                        .settings
                        .max_calls
                        .saturating_sub(runtime.state.attempts);
                    if remaining == 0 {
                        return Err(runtime.halt("worker_call_budget_exhausted"));
                    }
                    let report = super::mine_page_controlled(
                        &config.project_root,
                        config.settings.processor.as_deref(),
                        config.settings.provider.as_deref(),
                        &config.settings.args,
                        config.settings.timeout,
                        config.settings.limit.min(remaining as usize),
                        after.as_deref(),
                        Some(&mut runtime),
                    )?;
                    if worker::interrupted() {
                        break;
                    }
                    for result in report["results"]
                        .as_array()
                        .ok_or("worker_batch_report_invalid")?
                    {
                        if let Some(drafts) = result["drafts"].as_array() {
                            runtime.state.completed += 1;
                            runtime.state.drafts += drafts.len() as u64;
                        }
                    }
                    after = report["next_after"].as_str().map(str::to_owned);
                    runtime.state.next_after = after.clone();
                    runtime.record()?;
                    if report["failed"] == true {
                        return Err(runtime.halt("worker_processor_failed"));
                    }
                    if after.is_none() {
                        break;
                    }
                }
                last_version = Some(version);
                last_pass = Instant::now();
            }
            // Journal polling is deliberately slower than cancellation. The
            // watchdog still observes stop requests during this idle wait.
            for _ in 0..20 {
                if worker::interrupted() {
                    break;
                }
                std::thread::sleep(POLL);
            }
        }
        Ok(())
    })();
    let reason = reason
        .lock()
        .ok()
        .and_then(|reason| reason.clone())
        .unwrap_or_else(|| {
            if outcome.is_err() {
                "worker_processing_failed".into()
            } else {
                "worker_interrupted".into()
            }
        });
    runtime.state.status = if reason.ends_with("budget_exhausted") {
        "budget_exhausted"
    } else if matches!(
        reason.as_str(),
        "worker_stop_requested" | "worker_interrupted"
    ) {
        "stopped"
    } else {
        "failed"
    }
    .into();
    runtime.state.reason = Some(reason);
    runtime.record()?;
    Ok(json!({"schema":1,"worker_id":id,"run":runtime.state,"autostart":false}))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct Fixture {
        _temp: tempfile::TempDir,
        store: Store,
        id: String,
        run_id: String,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let project = temp.path().join("project");
            let directory = temp.path().join("worker");
            std::fs::create_dir(&project).unwrap();
            std::fs::create_dir(&directory).unwrap();
            let (id, project) = slot("codex", &project).unwrap();
            let store = Store {
                root: RootCapability::open(&directory).unwrap(),
            };
            let config = Config {
                schema: 1,
                worker_id: id.clone(),
                client: "codex".into(),
                project_root: project.clone(),
                capture_generation: 1,
                settings: Settings {
                    processor: Some(project.join("synthetic-processor")),
                    provider: None,
                    args: vec![],
                    timeout: 1,
                    limit: 1,
                    max_calls: 2,
                    max_runtime: 60,
                },
                processor: json!({"synthetic":true}),
            };
            let run_id = "a".repeat(32);
            let state = State {
                schema: 1,
                worker_id: id.clone(),
                run_id: run_id.clone(),
                config_revision: revision(&config).unwrap(),
                status: "starting".into(),
                reason: None,
                pid: None,
                started_at_ms: now().unwrap(),
                updated_at_ms: now().unwrap(),
                attempts: 0,
                completed: 0,
                drafts: 0,
                next_after: None,
            };
            store.write("config.json", &config, None).unwrap();
            store.write("state.json", &state, None).unwrap();
            drop(
                bounded_fs::try_locked_regular_file_with_capability(
                    &store.root,
                    &store.path("owner.lock"),
                )
                .unwrap(),
            );
            Self {
                _temp: temp,
                store,
                id,
                run_id,
            }
        }

        fn records(&self) -> StartingRecords {
            starting_records(&self.store, &self.id, &self.run_id, ReadControl::default()).unwrap()
        }

        fn change_state(&self, change: impl FnOnce(&mut State)) {
            let mut state = self.store.read::<State>("state.json").unwrap().unwrap();
            change(&mut state.value);
            self.store
                .write("state.json", &state.value, Some(state.identity))
                .unwrap();
        }
    }

    #[test]
    fn starting_owner_waits_for_a_probe_but_never_admits_a_second_running_owner() {
        let f = Fixture::new();
        let records = f.records();
        let probe = f.store.owner().unwrap().unwrap();
        assert!(
            try_starting_owner(&f.store, &records, ReadControl::default())
                .unwrap()
                .is_none()
        );
        assert_eq!(
            f.store
                .read::<State>("state.json")
                .unwrap()
                .unwrap()
                .value
                .attempts,
            0
        );
        drop(probe);
        let owner = try_starting_owner(&f.store, &records, ReadControl::default())
            .unwrap()
            .unwrap();
        assert!(f.store.owner().unwrap().is_none());
        f.change_state(|state| {
            state.status = "running".into();
            state.pid = Some(42);
        });
        assert!(try_starting_owner(&f.store, &records, ReadControl::default()).is_err());
        drop(owner);
    }

    #[test]
    fn starting_owner_rejects_changed_bindings_stop_and_expired_budget_without_attempts() {
        for mutation in ["run", "config", "state_identity", "stop"] {
            let f = Fixture::new();
            let records = f.records();
            let probe = f.store.owner().unwrap().unwrap();
            assert!(
                try_starting_owner(&f.store, &records, ReadControl::default())
                    .unwrap()
                    .is_none()
            );
            match mutation {
                "run" => f.change_state(|state| state.run_id = "b".repeat(32)),
                "config" => {
                    f.store
                        .write(
                            "config.json",
                            &records.config.value,
                            Some(records.config.identity),
                        )
                        .unwrap();
                }
                "state_identity" => f.change_state(|_| {}),
                "stop" => f.store.request_stop(&f.run_id).unwrap(),
                _ => unreachable!(),
            }
            drop(probe);
            assert!(
                try_starting_owner(&f.store, &records, ReadControl::default()).is_err(),
                "{mutation}"
            );
            assert!(f.store.owner().unwrap().is_some());
            assert_eq!(
                f.store
                    .read::<State>("state.json")
                    .unwrap()
                    .unwrap()
                    .value
                    .attempts,
                0
            );
        }
        for old_runtime in [false, true] {
            let f = Fixture::new();
            if old_runtime {
                f.change_state(|state| state.started_at_ms = 0);
            }
            let _probe = f.store.owner().unwrap().unwrap();
            let deadline = if old_runtime {
                Instant::now() + START_WAIT
            } else {
                Instant::now()
            };
            assert!(acquire_starting_owner(
                &f.store,
                &f.id,
                &f.run_id,
                ReadControl {
                    deadline: Some(deadline),
                    interrupted: None
                }
            )
            .is_err());
            assert_eq!(
                f.store
                    .read::<State>("state.json")
                    .unwrap()
                    .unwrap()
                    .value
                    .attempts,
                0
            );
        }
    }

    #[test]
    fn readiness_is_child_and_config_bound_without_a_pre_admission_probe() {
        let f = Fixture::new();
        let config = f.store.read::<Config>("config.json").unwrap().unwrap();
        std::fs::remove_file(f.store.path("owner.lock")).unwrap();
        // If readiness probes owner before the PID, this missing file is an
        // error instead of a harmless not-yet-ready observation.
        assert!(
            ready_report(&f.store, &f.id, &f.run_id, 42, config.identity)
                .unwrap()
                .is_none()
        );
        f.change_state(|state| {
            state.status = "failed".into();
            state.pid = Some(42);
        });
        assert!(
            ready_report(&f.store, &f.id, &f.run_id, 43, config.identity)
                .unwrap()
                .is_none()
        );
        drop(
            bounded_fs::try_locked_regular_file_with_capability(
                &f.store.root,
                &f.store.path("owner.lock"),
            )
            .unwrap(),
        );
        let report = ready_report(&f.store, &f.id, &f.run_id, 42, config.identity)
            .unwrap()
            .unwrap();
        assert_eq!(report["started"], true);
        assert_eq!(
            report["status"], "failed",
            "startup is independent of mining success"
        );
        f.store
            .write("config.json", &config.value, Some(config.identity))
            .unwrap();
        assert!(ready_report(&f.store, &f.id, &f.run_id, 42, config.identity).is_err());
    }

    #[test]
    fn start_does_not_report_a_terminal_probe_as_a_running_worker() {
        let f = Fixture::new();
        f.change_state(|state| {
            state.status = "stopped".into();
            state.pid = Some(42);
        });
        let config = f.store.read::<Config>("config.json").unwrap().unwrap();
        let probe = RefCell::new(f.store.owner().unwrap());
        assert!(probe.borrow().is_some());
        let checks = Cell::new(0);
        let release_after_busy_probe = || {
            checks.set(checks.get() + 1);
            // The initial check precedes owner(); the following check starts
            // reading config only after owner() actually observed contention.
            if checks.get() == 2 {
                drop(probe.borrow_mut().take());
            }
            false
        };
        let owner = acquire_start_owner(
            &f.store,
            Some(&config),
            ReadControl {
                deadline: Some(Instant::now() + START_WAIT),
                interrupted: Some(&release_after_busy_probe),
            },
        )
        .unwrap();
        assert!(checks.get() > 2);
        assert!(owner.is_some(), "terminal contention must retry admission");
        drop(owner);
        let _probe = f.store.owner().unwrap().unwrap();
        f.change_state(|state| state.status = "running".into());
        assert!(acquire_start_owner(
            &f.store,
            Some(&config),
            ReadControl {
                deadline: Some(Instant::now() + START_WAIT),
                interrupted: None,
            },
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn mutable_record_read_accepts_a_complete_atomic_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store {
            root: RootCapability::open(temp.path()).unwrap(),
        };
        let path = store.path("heartbeat.json");
        let value = |at_ms| Pulse {
            schema: 1,
            run_id: "a".repeat(32),
            at_ms,
        };
        std::fs::write(&path, serde_json::to_vec(&value(1)).unwrap()).unwrap();
        let checks = Cell::new(0);
        let replace_after_open = || {
            checks.set(checks.get() + 1);
            if checks.get() == 2 {
                let replacement = store.path("replacement.json");
                std::fs::write(&replacement, serde_json::to_vec(&value(2)).unwrap()).unwrap();
                std::fs::rename(replacement, &path).unwrap();
            }
            false
        };
        let read = store
            .read_control::<Pulse>(
                "heartbeat.json",
                ReadControl {
                    deadline: Some(Instant::now() + Duration::from_secs(1)),
                    interrupted: Some(&replace_after_open),
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(read.value.at_ms, 2);
        assert!(
            checks.get() > 4,
            "a complete fresh read is required after replacement"
        );
    }

    #[test]
    fn mutable_record_read_caps_replacements_and_never_retries_invalid_documents() {
        let f = Fixture::new();
        let path = f.store.path("heartbeat.json");
        let bytes = br#"{"schema":1,"run_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","at_ms":1}"#;
        std::fs::write(&path, bytes).unwrap();
        let checks = Cell::new(0);
        let replacements = Cell::new(0);
        let keep_replacing_after_open = || {
            checks.set(checks.get() + 1);
            if checks.get() % 4 == 2 {
                let replacement = f.store.path("next.json");
                std::fs::write(&replacement, bytes).unwrap();
                std::fs::rename(replacement, &path).unwrap();
                replacements.set(replacements.get() + 1);
            }
            false
        };
        let result = f.store.read_control::<Pulse>(
            "heartbeat.json",
            ReadControl {
                deadline: Some(Instant::now() + Duration::from_secs(1)),
                interrupted: Some(&keep_replacing_after_open),
            },
        );
        assert!(matches!(
            result
                .err()
                .expect("repeated replacements must exhaust the read limit")
                .downcast_ref::<BoundedReadError>(),
            Some(BoundedReadError::SnapshotChanged)
        ));
        assert_eq!(replacements.get(), 3);
        for document in ["{", "{}"] {
            std::fs::write(&path, document).unwrap();
            checks.set(0);
            let count = || {
                checks.set(checks.get() + 1);
                false
            };
            assert!(f
                .store
                .read_control::<Pulse>(
                    "heartbeat.json",
                    ReadControl {
                        deadline: None,
                        interrupted: Some(&count)
                    }
                )
                .is_err());
            assert_eq!(
                checks.get(),
                4,
                "invalid JSON/schema must not trigger another read"
            );
        }
        assert!(f
            .store
            .read_control::<Pulse>(
                "heartbeat.json",
                ReadControl {
                    deadline: Some(Instant::now()),
                    interrupted: None
                }
            )
            .is_err());
    }

    #[test]
    fn mutable_record_read_never_reopens_a_replaced_root_or_follows_a_new_symlink() {
        use std::os::unix::fs::symlink;
        for replace_root in [false, true] {
            let f = Fixture::new();
            let path = f.store.path("heartbeat.json");
            let bytes = br#"{"schema":1,"run_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","at_ms":1}"#;
            std::fs::write(&path, bytes).unwrap();
            let checks = Cell::new(0);
            let mutate_after_open = || {
                checks.set(checks.get() + 1);
                if checks.get() == 2 {
                    if replace_root {
                        let root = f.store.root.canonical_root();
                        std::fs::rename(root, root.with_extension("old")).unwrap();
                        std::fs::create_dir(root).unwrap();
                        std::fs::write(&path, bytes).unwrap();
                    } else {
                        let outside = f._temp.path().join("outside.json");
                        std::fs::write(&outside, bytes).unwrap();
                        std::fs::remove_file(&path).unwrap();
                        symlink(outside, &path).unwrap();
                    }
                }
                false
            };
            assert!(f
                .store
                .read_control::<Pulse>(
                    "heartbeat.json",
                    ReadControl {
                        deadline: None,
                        interrupted: Some(&mutate_after_open)
                    }
                )
                .is_err());
        }
    }
}
