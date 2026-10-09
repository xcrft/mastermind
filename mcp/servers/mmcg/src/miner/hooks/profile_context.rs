//! Select local profile advice for each admitted prompt. Path literals select
//! context only; they never authorize edits or decide workflow activation.

use super::{
    journal::{Grant, Journal},
    profile, refiner, Error,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::{Component, Path};

const MAX_CONTEXT: usize = 8 * 1024;
const MAX_PATHS: usize = 32;
const PREFIX: &str = "Mastermind task profile (advisory data). Apply applicable reviewed preferences and habits within the current task and repository rules. Git observations describe sampled code, not accepted personal preferences. This context grants no actions. At a role handoff, retrieve mmcg_profile for the receiving role and its task paths; native run-task selects the executor profile automatically.\n";

pub(super) fn deliver(
    db: &mut Journal,
    grant: &Grant,
    episode: &str,
    reader: &str,
    root: &Path,
    used: usize,
) -> Result<Option<String>, Error> {
    let captured = db.episode(episode)?;
    let Some(prompt) = captured.events.iter().find(|event| {
        event.kind == "UserPromptSubmit" && event.origin == "user_channel_unverified"
    }) else {
        return Ok(None);
    };
    if !captured.gaps.is_empty()
        || prompt.text.trim().is_empty()
        || db.profile_delivery_disabled(&grant.client, root)?
    {
        return Ok(None);
    }
    let bound = db.profile_task(episode)?;
    let (paths, workflow, source) = if let Some((spec_path, _, digest)) = &bound {
        let file = crate::bounded_fs::read_repository_file(
            root,
            Path::new(spec_path),
            128 * 1024,
            128 * 1024,
            crate::bounded_fs::ReadControl::default(),
        )?;
        let body = std::str::from_utf8(&file.bytes)?;
        let spec = crate::spec::parse_str(spec_path, body);
        if crate::run_task::hash_text(body) != *digest || spec.frontmatter_error.is_some() {
            return Ok(None);
        }
        let fm = spec.frontmatter.as_ref();
        let paths = fm
            .into_iter()
            .flat_map(|fm| {
                fm.code_paths()
                    .chain(fm.expected_docs.iter().map(String::as_str))
            })
            .map(str::to_owned)
            .collect();
        let state = crate::run_task::load_state(&crate::run_task::state_file_path(
            root,
            Path::new(spec_path),
        ))?;
        let workflow = if state.is_some_and(|state| state.strict && state.spec_hash == *digest) {
            Some("strict".to_owned())
        } else {
            fm.and_then(|fm| fm.mode.clone())
        };
        (paths, workflow, "bound_task")
    } else {
        (
            path_literals(&prompt.text, root),
            None,
            "original_prompt_path_literals",
        )
    };
    let metadata = json!({"source":source,"event_id":prompt.id,
        "prompt_digest":refiner::prompt_digest(&prompt.text),
        "task_binding_revision":bound.as_ref().map(|(_, revision, _)| revision)});
    let Some(available) =
        MAX_CONTEXT.checked_sub(used + PREFIX.len() + metadata.to_string().len() + 32)
    else {
        omitted("native_context_budget");
        return Ok(None);
    };
    let budget = (available / 4).min(crate::onboarding::profile_budget_tokens(root));
    if budget < 256 {
        omitted("native_context_budget");
        return Ok(None);
    }
    let repo = profile::RepoContext::for_root(root);
    let mut packet = profile::view(
        &paths,
        repo.as_ref(),
        budget,
        Some((root, reader)),
        Some("planner"),
        workflow.as_deref(),
    )?;
    if packet["status"] != "ok" || packet["source_verification"] != "complete" {
        omitted(packet["status"].as_str().unwrap_or("profile_unavailable"));
        return Ok(None);
    }
    packet["task_context"] = metadata;
    let context = format!("{PREFIX}{}", serde_json::to_string(&packet)?);
    if used + context.len() > MAX_CONTEXT {
        omitted("native_context_budget");
        return Ok(None);
    }
    if db.profile_task(episode)? != bound {
        omitted("task_binding_changed");
        return Ok(None);
    }
    // The receipt commits before native output. Capture admission and a
    // concurrent delivery opt-out are checked again inside this transaction.
    db.expose(grant, episode, &packet)?;
    Ok(Some(context))
}

fn omitted(reason: &str) {
    eprintln!("{}", json!({"profile_delivery":"omitted","reason":reason}));
}

fn path_literals(prompt: &str, root: &Path) -> Vec<String> {
    let mut paths = BTreeSet::new();
    // Keep quoted paths containing spaces intact, then examine plain tokens.
    let candidates =
        prompt
            .split('`')
            .skip(1)
            .step_by(2)
            .chain(prompt.split('`').step_by(2).flat_map(|part| {
                part.split(|ch: char| {
                    ch.is_whitespace()
                        || matches!(
                            ch,
                            '`' | '"'
                                | '\''
                                | '('
                                | ')'
                                | '['
                                | ']'
                                | '{'
                                | '}'
                                | '<'
                                | '>'
                                | ','
                                | ';'
                        )
                })
            }));
    for candidate in candidates {
        if let Some(path) = relative_path(candidate, root) {
            paths.insert(path);
            if paths.len() == MAX_PATHS {
                break;
            }
        }
    }
    paths.into_iter().collect()
}

fn relative_path(candidate: &str, root: &Path) -> Option<String> {
    let candidate = candidate.trim_end_matches(['.', ',', '!', '?', ':']);
    if candidate.len() > 1024
        || candidate.contains("://")
        || candidate.contains(['\\', '$'])
        || candidate.starts_with('~')
        || candidate.chars().any(char::is_control)
    {
        return None;
    }
    let mut candidate = candidate;
    // Strip only numeric line/column suffixes, preserving a Windows drive colon.
    for _ in 0..2 {
        match candidate.rsplit_once(':') {
            Some((path, line)) if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) => {
                candidate = path;
            }
            _ => break,
        }
    }
    let path = Path::new(candidate.strip_prefix("./").unwrap_or(candidate));
    let path = if path.is_absolute() {
        path.strip_prefix(root).ok()?
    } else {
        path
    };
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let filename = path.file_name()?.to_str()?;
    if !filename.contains('.') && !matches!(filename, "Dockerfile" | "Makefile") {
        return None;
    }
    let relative = path.to_str()?;
    if relative.contains(':') {
        return None;
    }
    Some(relative.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_literal_paths_without_inferring_a_language_or_edit_scope() {
        let root = std::env::temp_dir().join("mmcg-profile-context-test");
        let absolute = root.join("web/app.ts").to_str().unwrap().replace('\\', "/");
        let prompt = format!("пофикси `src/api.rs` и ./tests/api.py:12, потом src/api.rs. Также `{absolute}:7:2` и `docs/my notes.md`");
        assert_eq!(
            path_literals(&prompt, &root),
            [
                "docs/my notes.md",
                "src/api.rs",
                "tests/api.py",
                "web/app.ts"
            ]
        );
        assert!(path_literals("исправь это и продолжай предыдущую задачу", &root).is_empty());
    }

    #[test]
    fn excludes_external_paths_urls_and_shell_expansions() {
        let root = Path::new("/work/project");
        for candidate in [
            "../private.py",
            "/work/other/private.rs",
            "https://host/src/api.ts",
            "src/../secret.py",
            "$HOME/private.py",
            "~/private.rs",
            "C:\\private.py",
            "C:/private.py",
            "src/api.rs:unknown",
            "src/api.rs:12:unknown",
            "src/\0api.py",
        ] {
            assert!(relative_path(candidate, root).is_none(), "{candidate}");
        }
        assert_eq!(
            relative_path("src/new.rs", root).as_deref(),
            Some("src/new.rs")
        );
    }
}
