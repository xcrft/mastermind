//! Evidence-backed descriptions of a person's behavior in a situation.
//! Proposals quote actual human session turns. Review is an explicit local
//! action; agents receive only observed, scoped descriptions.

use super::feedback::{self, VerifiedQuote};
use super::profile::{persona_project_id, persona_repository_id, profile_path, publish_profile};
use super::store::{self, NewHabit, NewHabitEvidence};
use std::io::IsTerminal;
use std::path::Path;

pub(super) fn field(
    value: &str,
    name: &str,
    min: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let clean = feedback::single_line(value, name, min, 200)?;
    if feedback::looks_secret(&clean) || crate::indexer::secret_like_documentation(&clean) {
        return Err(format!("refusing to record {name} that looks like a credential").into());
    }
    Ok(clean)
}

pub(super) fn episode(value: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value = feedback::single_line(value, "episode", 3, 128)?;
    if feedback::looks_secret(&value) || crate::indexer::secret_like_documentation(&value) {
        return Err("refusing to record an episode that looks like a credential".into());
    }
    Ok(value)
}

fn source_evidence<'a>(
    source: &'a VerifiedQuote,
    episode: &'a str,
    relation: &'a str,
    project_id: &'a str,
    repository_id: &'a str,
) -> NewHabitEvidence<'a> {
    NewHabitEvidence {
        source: &source.source,
        source_path: &source.source_path,
        line_no: source.line_no as i64,
        record_digest: &source.record_digest,
        episode,
        project: project_id,
        repository: repository_id,
        quote: &source.quote,
        at: &source.at,
        relation,
    }
}

fn source_project(
    project_root: &Path,
    source: &VerifiedQuote,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let root = project_root.canonicalize()?;
    if source
        .claude_project
        .as_ref()
        .is_some_and(|slug| *slug != feedback::claude_project_slug(&root))
        || source.project_cwd.as_deref() != Some(root.as_path())
    {
        return Err(
            "transcript lacks an exact project-root binding to the supplied checkout".into(),
        );
    }
    let project_id = persona_project_id(&root).ok_or("cannot identify project root")?;
    let repository_id = persona_repository_id(&root).unwrap_or_default();
    Ok((project_id, repository_id))
}

/// Start a candidate with one exact human quote. Project scope is the default;
/// cross-project scope must clear the stronger review gate.
#[allow(clippy::too_many_arguments)]
pub fn propose(
    project_root: &Path,
    transcript: &Path,
    quote: &str,
    episode_id: &str,
    when: &str,
    behavior: &str,
    outcome: &str,
    exception: &str,
    global: bool,
    role: Option<&str>,
    workflow: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let when = field(when, "when", 8)?;
    let behavior = field(behavior, "behavior", 8)?;
    let outcome = field(outcome, "outcome", 8)?;
    let exception = field(exception, "exception", 0)?;
    let episode_id = episode(episode_id)?;
    let role = role.unwrap_or("");
    if !role.is_empty() && !matches!(role, "planner" | "executor" | "auditor") {
        return Err("role must be planner, executor or auditor".into());
    }
    let workflow = match workflow {
        Some(value) => field(value, "workflow", 1)?,
        None => String::new(),
    };
    let source = feedback::verified_habit_quote(transcript, quote)?;
    let (project_id, repository_id) = source_project(project_root, &source)?;
    let scope = if global {
        "global".to_string()
    } else {
        format!("project:{project_id}")
    };
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let (habit, _) = publish_profile(
        &db_path,
        &profile_path()?,
        false,
        |db| {
            Ok(db.record_habit(
                &NewHabit {
                    when: &when,
                    behavior: &behavior,
                    outcome: &outcome,
                    exception: &exception,
                    scope: &scope,
                    role,
                    workflow: &workflow,
                },
                &source_evidence(
                    &source,
                    &episode_id,
                    "supports",
                    &project_id,
                    &repository_id,
                ),
            )?)
        },
        |_| None,
    )?;
    println!(
        "Habit {}: {} ({} episode(s), {} project(s)). Review its evidence before observing.",
        habit.id, habit.status, habit.episodes, habit.projects
    );
    Ok(())
}

/// Add an independent episode, a limitation, or a counterexample to a claim.
pub fn cite(
    id: i64,
    project_root: &Path,
    transcript: &Path,
    quote: &str,
    episode_id: &str,
    relation: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(relation, "supports" | "contradicts" | "limits") {
        return Err("relation must be supports, contradicts or limits".into());
    }
    let episode_id = episode(episode_id)?;
    let source = feedback::verified_habit_quote(transcript, quote)?;
    let (project_id, repository_id) = source_project(project_root, &source)?;
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let (habit, _) = publish_profile(
        &db_path,
        &profile_path()?,
        false,
        |db| {
            Ok(db.add_habit_evidence(
                id,
                &source_evidence(&source, &episode_id, relation, &project_id, &repository_id),
            )?)
        },
        |_| None,
    )?;
    let habit = habit.ok_or_else(|| format!("habit {id} does not exist"))?;
    if habit.status == "rejected" {
        return Err(format!("habit {id} was rejected; no evidence was added").into());
    }
    println!(
        "Habit {}: {} ({} episode(s), {} project(s), {} counterexample(s)).",
        id, habit.status, habit.episodes, habit.projects, habit.contradictions
    );
    Ok(())
}

pub fn list() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let Some(db) = store::ProfileStore::open_optional_read_only(&db_path)? else {
        println!("No habits recorded.");
        return Ok(());
    };
    let habits = db.habits()?;
    if habits.is_empty() {
        println!("No habits recorded.");
    }
    for habit in habits {
        println!(
            "{}\t{}\tgeneration={}\t{} episode(s)\t{}\t{} → {}",
            habit.id,
            habit.status,
            habit.generation,
            habit.episodes,
            habit.scope,
            habit.when,
            habit.behavior
        );
    }
    Ok(())
}

/// Regenerate the static style.md snapshot after source files have changed.
pub fn refresh() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let (_, published) = publish_profile(&db_path, &profile_path()?, false, |_| Ok(()), |_| None)?;
    println!(
        "Refreshed style.md at profile revision {}.",
        published.revision
    );
    Ok(())
}

pub fn show(id: i64) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let db = store::ProfileStore::open_read_only(&db_path)?;
    let habit = db
        .habit(id)?
        .ok_or_else(|| format!("habit {id} does not exist"))?;
    println!(
        "Habit {}\t{}\t{}\trole={}\tworkflow={}",
        id, habit.status, habit.scope, habit.role, habit.workflow
    );
    println!("When: {}", habit.when);
    println!(
        "Generation: {} (root {}, renewed from {:?}, renewed as {:?})",
        habit.generation, habit.generation_root, habit.renewed_from, habit.renewed_as
    );
    if let Some(status) = habit.retired_status() {
        println!("Effective terminal status: {status}");
    }
    println!("Behavior: {}", habit.behavior);
    println!("Outcome: {}", habit.outcome);
    println!("Review revision: {}", habit.review_revision());
    println!(
        "Observed revision: {}",
        habit.observed_revision.as_deref().unwrap_or("none")
    );
    let current = super::profile::habit_sources_current(
        &db,
        &habit,
        &mut feedback::QuoteSourceVerifier::new(),
    );
    println!(
        "Source verification: {}",
        current.err().unwrap_or("current")
    );
    if !habit.exception.is_empty() {
        println!("Exception: {}", habit.exception);
    }
    println!(
        "Support: {} episode(s), {} session(s), {} project(s); {} counterexample(s).",
        habit.episodes, habit.sources, habit.projects, habit.contradictions
    );
    for item in db.habit_evidence(id)? {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}:{}\t{}",
            item.id,
            item.status,
            item.at,
            item.relation,
            item.episode,
            item.project,
            item.source_path,
            item.line_no,
            item.quote
        );
    }
    for change in db.habit_rebinds(id)? {
        println!(
            "Rebound evidence {}: {} {}:{} ({}) → {} {}:{} ({})",
            change.evidence_id,
            change.old_at,
            change.old_source_path,
            change.old_line_no,
            change.old_record_digest,
            change.new_at,
            change.new_source_path,
            change.new_line_no,
            change.new_record_digest,
        );
    }
    println!(
        "Candidate proposals: {}",
        db.candidate_habit_history(None, Some(id))?
    );
    println!("Review history: {}", db.habit_review_history(id)?);
    println!("Relations: {}", db.habit_relations(id)?);
    println!("Generations: {}", db.habit_generations(id)?);
    Ok(())
}

/// Reconsider a retired description without inheriting its evidence or review.
pub fn renew(id: i64, revision: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !std::io::stdin().is_terminal() {
        return Err("habit renew requires the author's interactive terminal".into());
    }
    if id <= 0 || !super::collection::valid_id(revision) {
        return Err("renewal requires a positive habit ID and full review revision".into());
    }
    let path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    store::ProfileStore::open_read_only(&path)?;
    let (receipt, _) = publish_profile(
        &path,
        &profile_path()?,
        false,
        |db| Ok(db.renew_habit(id, revision)?),
        |_| None,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"renewal":receipt,
            "note":"A new generation copies only the description. Cite evidence and review it separately. Renewal does not establish a return of the behavior or replace another active habit. Exact retries return the existing child's current stored status."
        }))?
    );
    Ok(())
}

/// Retire one reviewed description and observe its explicitly selected replacement.
pub fn supersede(
    old: i64,
    new: i64,
    old_revision: &str,
    new_revision: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if !std::io::stdin().is_terminal() {
        return Err("habit supersede requires the author's interactive terminal".into());
    }
    if old <= 0 || new <= 0 || old == new {
        return Err("replacement requires two different positive habit IDs".into());
    }
    if !super::collection::valid_id(old_revision) || !super::collection::valid_id(new_revision) {
        return Err("both review revisions must be full 64-character keys".into());
    }
    let path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    store::ProfileStore::open_read_only(&path)?;
    let (receipt, _) = publish_profile(
        &path,
        &profile_path()?,
        false,
        |db| {
            if let Some(receipt) =
                db.habit_supersession_retry(old, new, old_revision, new_revision)?
            {
                return Ok(receipt);
            }
            let old_habit = db.habit(old)?.ok_or("old habit missing")?;
            let new_habit = db.habit(new)?.ok_or("new habit missing")?;
            if old_habit.review_revision() != old_revision
                || new_habit.review_revision() != new_revision
            {
                return Err("review revision changed; inspect both habits again".into());
            }
            super::profile::habit_sources_current(
                db,
                &new_habit,
                &mut feedback::QuoteSourceVerifier::new(),
            )?;
            Ok(db.supersede_habit(old, new, old_revision, new_revision)?)
        },
        |_| None,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "relation":"supersedes","receipt":receipt,
            "note":"Stored statuses shown. Publication revalidates current sources. A committed retry does not observe the successor again."
        }))?
    );
    Ok(())
}

/// An observed habit is a reviewed description, not an accepted instruction.
pub fn review(
    id: i64,
    status: &str,
    revision: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    if status == "observed" && !std::io::stdin().is_terminal() {
        return Err(
            "observe needs an interactive terminal: review `miner habit show` yourself".into(),
        );
    }
    if status == "observed" && !revision.is_some_and(super::collection::valid_id) {
        return Err(
            "observe requires the full 64-character review revision from habit show".into(),
        );
    }
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    store::ProfileStore::open_read_only(&db_path)?;
    let (result, _) = publish_profile(
        &db_path,
        &profile_path()?,
        false,
        |db| {
            if status == "observed" {
                let habit = db
                    .habit(id)?
                    .ok_or_else(|| format!("habit {id} does not exist"))?;
                if revision != Some(habit.review_revision().as_str()) {
                    return Err("habit review revision changed; inspect habit show again".into());
                }
                super::profile::habit_sources_current(
                    db,
                    &habit,
                    &mut feedback::QuoteSourceVerifier::new(),
                )?;
            }
            Ok(db.review_habit(id, status, revision)??)
        },
        |_| None,
    )?;
    println!("Habit {} is {}.", result.id, result.status);
    Ok(())
}

/// Dismiss one incorrectly attributed citation without erasing the record.
pub fn dismiss(id: i64, evidence_id: i64) -> Result<(), Box<dyn std::error::Error>> {
    if !std::io::stdin().is_terminal() {
        return Err(
            "dismiss needs an interactive terminal: inspect `miner habit show` first".into(),
        );
    }
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let (changed, _) = publish_profile(
        &db_path,
        &profile_path()?,
        false,
        |db| Ok(db.dismiss_habit_evidence(id, evidence_id)?),
        |_| None,
    )?;
    if !changed {
        return Err(format!("active evidence {evidence_id} for habit {id} does not exist").into());
    }
    println!("Evidence {evidence_id} for habit {id} dismissed; inspect habit show for its current status.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossy_claude_slug_cannot_rebind_evidence_to_another_root() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a-b");
        let second = dir.path().join("a/b");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let first = first.canonicalize().unwrap();
        let second = second.canonicalize().unwrap();
        assert_eq!(
            feedback::claude_project_slug(&first),
            feedback::claude_project_slug(&second)
        );
        let source = VerifiedQuote {
            quote: "Check the rollout separately".into(),
            at: "2026-09-25".into(),
            source: "session:fixture".into(),
            claude_project: Some(feedback::claude_project_slug(&first)),
            project_cwd: Some(first.clone()),
            source_path: "fixture.jsonl".into(),
            line_no: 1,
            record_digest: "a".repeat(64),
        };
        assert!(source_project(&first, &source).is_ok());
        assert!(source_project(&second, &source).is_err());
    }
}
