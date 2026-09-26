//! Explicit interpretation of exact inbox observations as unreviewed proposals.

use super::{collection, habit, profile, store};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Serialize)]
pub struct PreferenceDraft {
    pub statement: String,
    pub category: String,
    pub scope: Option<String>,
}

pub fn propose_preference(
    id: &str,
    revision: &str,
    draft: PreferenceDraft,
) -> Result<(), Box<dyn std::error::Error>> {
    use super::feedback;
    if !collection::valid_id(id) || !collection::valid_id(revision) {
        return Err("candidate id and revision must be full 64-character keys".into());
    }
    let statement = feedback::single_line(&draft.statement, "statement", 8, 200)?;
    if !feedback::CATEGORIES.contains(&draft.category.as_str()) {
        return Err(format!(
            "category must be one of {}",
            feedback::CATEGORIES.join(", ")
        )
        .into());
    }
    if feedback::looks_secret(&statement) || crate::indexer::secret_like_documentation(&statement) {
        return Err("refusing to record text that looks like a credential".into());
    }
    if let Some(scope) = &draft.scope {
        feedback::validate_scope(scope)?;
    }
    let path = store::ProfileStore::db_path().ok_or("could not resolve profile store")?;
    let existing = store::ProfileStore::open_read_only(&path)?;
    if existing.collected_candidate(id)?.is_none() {
        return Err("candidate not found".into());
    }
    drop(existing);
    let (receipt, _) = profile::publish_profile(
        &path,
        &profile::profile_path()?,
        false,
        |db| {
            let (candidate, repository) = current_candidate(db, id, revision)?;
            let scope = draft
                .scope
                .clone()
                .unwrap_or_else(|| format!("project:{}", candidate.project));
            let key = store::feedback_key(&statement, &draft.category, &scope);
            let mut prospective = db.feedback_candidate_bindings(&key)?;
            // New support must fit the complete verification budget now. An
            // existing citation can be repaired independently when several
            // sources moved at once; the full set is rechecked on acceptance.
            if !prospective.iter().any(|old| old.id == candidate.id) {
                prospective.push(candidate.clone());
                let mut verifier = feedback::QuoteSourceVerifier::new();
                if !prospective
                    .iter()
                    .all(|item| verifier.current_observation(item))
                {
                    return Err("prospective preference sources are unavailable or exceed the verification budget; inspect or dismiss a source first".into());
                }
            }
            let request = serde_json::to_vec(&serde_json::json!([
                "candidate-preference-v1",
                statement,
                draft.category,
                scope,
                candidate.project,
                candidate.project_root,
                repository
            ]))?;
            let request_digest = crate::hex::encode(&Sha256::digest(request));
            Ok(db.propose_candidate_feedback(
                &candidate,
                &statement,
                &draft.category,
                &scope,
                &repository,
                &request_digest,
            )?)
        },
        |_| None,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"proposal":receipt,
        "note":"Unreviewed preference with exact provenance. Inspect feedback show and accept its review revision before publication."}))?
    );
    Ok(())
}

fn current_candidate(
    db: &store::ProfileStore,
    id: &str,
    revision: &str,
) -> Result<(store::CollectedCandidate, String), Box<dyn std::error::Error>> {
    let candidate = db.collected_candidate(id)?.ok_or("candidate not found")?;
    if candidate.revision != revision || !candidate.present || candidate.status != "pending" {
        return Err("candidate changed, removed or dismissed; inspect it again".into());
    }
    let source = db
        .collection_source(&candidate.source)?
        .ok_or("candidate source missing")?;
    let root = Path::new(&candidate.project_root);
    if profile::persona_project_id(root).as_deref() != Some(candidate.project.as_str())
        || profile::persona_repository_id(root).unwrap_or_default() != source.repository
    {
        return Err("candidate project identity changed; collect again".into());
    }
    if collection::Verifier::new().freshness(&candidate) != "current" {
        return Err(
            "candidate source is unavailable, changed or unsupported; collect and inspect again"
                .into(),
        );
    }
    Ok((candidate, source.repository))
}

#[derive(Serialize)]
pub struct HabitDraft {
    pub episode: String,
    pub when: String,
    pub behavior: String,
    pub outcome: String,
    pub exception: String,
    pub global: bool,
    pub role: String,
    pub workflow: String,
}

impl HabitDraft {
    fn validate(self) -> Result<Self, Box<dyn std::error::Error>> {
        if !matches!(self.role.as_str(), "" | "planner" | "executor" | "auditor") {
            return Err("role must be planner, executor or auditor".into());
        }
        Ok(Self {
            episode: habit::episode(&self.episode)?,
            when: habit::field(&self.when, "when", 8)?,
            behavior: habit::field(&self.behavior, "behavior", 8)?,
            outcome: habit::field(&self.outcome, "outcome", 8)?,
            exception: habit::field(&self.exception, "exception", 0)?,
            workflow: habit::field(&self.workflow, "workflow", 0)?,
            global: self.global,
            role: self.role,
        })
    }
}

pub fn propose_habit(
    id: &str,
    revision: &str,
    draft: HabitDraft,
    target: Option<i64>,
) -> Result<(), Box<dyn std::error::Error>> {
    if target.is_some_and(|id| id <= 0) {
        return Err("target habit must be a positive ID".into());
    }
    if !collection::valid_id(id) || !collection::valid_id(revision) {
        return Err("candidate id and revision must be full 64-character keys".into());
    }
    let draft = draft.validate()?;
    let path = store::ProfileStore::db_path().ok_or("could not resolve profile store")?;
    // Reject unknown IDs without creating a database or profile. Recheck the
    // exact row and source only after taking the publication lock.
    let existing = store::ProfileStore::open_read_only(&path)?;
    if existing.collected_candidate(id)?.is_none() {
        return Err("candidate not found".into());
    }
    drop(existing);
    let (receipt, _) = profile::publish_profile(
        &path,
        &profile::profile_path()?,
        false,
        |db| {
            let (candidate, repository) = current_candidate(db, id, revision)?;
            super::hooks::require_episode(&candidate, &draft.episode)?;
            let scope = if draft.global {
                "global".to_string()
            } else {
                format!("project:{}", candidate.project)
            };
            let request = serde_json::json!([
                "candidate-habit-v1",
                draft,
                scope,
                candidate.project,
                candidate.project_root,
                repository
            ]);
            // Preserve the exact legacy no-target request bytes. A selected
            // generation is a different request and cannot redirect a receipt.
            let request = match target {
                Some(id) => serde_json::json!(["candidate-habit-target-v1", id, request]),
                None => request,
            };
            let request = serde_json::to_vec(&request)?;
            let request_digest = crate::hex::encode(&Sha256::digest(request));
            Ok(db.propose_candidate_habit(
                &candidate,
                &store::NewHabit {
                    when: &draft.when,
                    behavior: &draft.behavior,
                    outcome: &draft.outcome,
                    exception: &draft.exception,
                    scope: &scope,
                    role: &draft.role,
                    workflow: &draft.workflow,
                },
                &draft.episode,
                &repository,
                &request_digest,
                target,
            )?)
        },
        |_| None,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "proposal":receipt,
            "note":"Proposal recorded with exact provenance. Semantic support and independent episodes require review before habit observe."
        }))?
    );
    Ok(())
}
