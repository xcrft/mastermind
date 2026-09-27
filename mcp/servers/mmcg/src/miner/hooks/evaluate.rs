//! Offline evaluation seam for synthetic refiner inputs, without hook admission.
//!
//! The selected processor still runs with normal process permissions. This path
//! uses the production refiner protocol but never reads or publishes a journal,
//! emits native context, binds an active task or grants workflow authority.

use super::{refiner, worker, Error, RefinerConfig};
use crate::bounded_fs::{self, ReadControl};
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

const MAX_INPUT_BYTES: u64 = 128 * 1024;

fn read_input(path: &Path) -> Result<refiner::Input, &'static str> {
    let (root, target) =
        bounded_fs::open_file_target(path).map_err(|_| "input_unavailable_or_unsafe")?;
    let input = bounded_fs::read_regular_file_with_capability(
        &root,
        &target,
        MAX_INPUT_BYTES,
        MAX_INPUT_BYTES,
        ReadControl {
            deadline: Some(Instant::now() + Duration::from_secs(5)),
            interrupted: None,
        },
    )
    .map_err(|_| "input_unavailable_or_unsafe")?;
    let value = crate::setup::parse_json_unique(&input.bytes).map_err(|_| "input_json_invalid")?;
    serde_json::from_value(value).map_err(|_| "input_schema_invalid")
}

pub(super) fn evaluate(input_path: &Path, config: &RefinerConfig) -> Result<Value, Error> {
    let started = Instant::now();
    let result = (|| {
        let input = read_input(input_path)?;
        config.validate().map_err(|_| "configuration_invalid")?;
        let _cancellation =
            worker::Cancellation::install().map_err(|_| "process_supervision_unavailable")?;
        let response =
            refiner::process(&input, config).map_err(|_| "processing_or_validation_failed")?;
        if worker::interrupted() {
            return Err("interrupted");
        }
        Ok(response)
    })();
    Ok(match result {
        Ok(response) => json!({
            "schema": 1,
            "status": "evaluated",
            "admission": false,
            "response": serde_json::to_value(response)?,
            "elapsed_ms": started.elapsed().as_millis(),
        }),
        Err(reason) => json!({
            "schema": 1,
            "status": "failed",
            "admission": false,
            "reason": reason,
            "elapsed_ms": started.elapsed().as_millis(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_reader_rejects_duplicate_unknown_and_oversized_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.json");
        let original = "Explain this setting.";
        let value = json!({
            "id": "synthetic-intake",
            "event_id": "synthetic-event",
            "episode_id": "synthetic-episode",
            "session_id": "synthetic-session",
            "client": "codex",
            "project_root": directory.path(),
            "prompt_digest": refiner::prompt_digest(original),
            "capture_generation": 1,
            "original": original,
            "active_task": null,
        });
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(read_input(&path).unwrap().original, original);
        let mut unknown = value.clone();
        unknown["activate"] = json!(true);
        std::fs::write(&path, serde_json::to_vec(&unknown).unwrap()).unwrap();
        assert_eq!(read_input(&path).unwrap_err(), "input_schema_invalid");
        std::fs::write(&path, b"{\"id\":\"first\",\"id\":\"second\"}").unwrap();
        assert_eq!(read_input(&path).unwrap_err(), "input_json_invalid");
        std::fs::write(&path, vec![b' '; MAX_INPUT_BYTES as usize + 1]).unwrap();
        assert_eq!(
            read_input(&path).unwrap_err(),
            "input_unavailable_or_unsafe"
        );
    }

    #[cfg(unix)]
    #[test]
    fn input_reader_never_follows_a_final_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.json");
        let link = directory.path().join("input.json");
        std::fs::write(&source, b"{}").unwrap();
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert_eq!(
            read_input(&link).unwrap_err(),
            "input_unavailable_or_unsafe"
        );
    }
}
