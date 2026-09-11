use std::path::{Path, PathBuf};

fn open_validated_index(
    index_path: &Path,
    root: &Path,
) -> Result<Option<mmcg::store::Store>, Box<dyn std::error::Error>> {
    match std::fs::symlink_metadata(index_path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(format!(
                "index path `{}` is not a regular file",
                index_path.display()
            )
            .into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!("cannot inspect index `{}`: {error}", index_path.display()).into());
        }
    }
    let store = mmcg::store::Store::open_read_only(index_path)?;
    if !store.schema_current()? {
        return Err(format!(
            "index schema at `{}` is missing or outdated; rebuild with `mastermind index .`",
            index_path.display()
        )
        .into());
    }
    mmcg::indexer::validate_index_root(&store, root)
        .map_err(|error| format!("index/root mismatch: {error}"))?;
    let populated = store.symbol_count()? > 0;
    store.ensure_source_snapshot_current()?;
    Ok(populated.then_some(store))
}

pub fn verify(
    spec: &Path,
    root: PathBuf,
    json: bool,
    require_index: bool,
    strict: bool,
    index_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
    let parsed =
        mmcg::spec::parse_file(spec).map_err(|e| format!("parse {}: {e}", spec.display()))?;
    let store = open_validated_index(index_path, &root)?;
    let mut report = mmcg::verify_spec::run(&parsed, store.as_ref(), &root);
    if let Some(store) = &store {
        store.ensure_source_snapshot_current()?;
    }
    if (strict || require_index) && store.is_none() {
        report.push_error(mmcg::verify_spec::Finding::StrictViolation {
            reason: "no index — run `mastermind index .` (required by --strict / --require-index)"
                .into(),
        });
    }
    if strict {
        for f in mmcg::verify_spec::strict_check(&parsed) {
            report.push_error(f);
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render_text());
    }
    if report.has_failures() {
        std::process::exit(1);
    }
    Ok(())
}

pub fn audit(
    spec: &Path,
    since: &str,
    root: PathBuf,
    json: bool,
    index_path: &Path,
    executor_report_path: Option<&Path>,
    bundle_path: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
    let source_path = std::path::absolute(spec)?;
    let parsed = mmcg::spec::parse_file(&source_path)
        .map_err(|e| format!("parse {}: {e}", source_path.display()))?;
    let store = open_validated_index(index_path, &root)?.ok_or_else(|| {
        format!(
            "no populated index at `{}`; run `mastermind index .`",
            index_path.display()
        )
    })?;

    let executor_report = executor_report_path
        .map(mmcg::executor_report::parse_file)
        .transpose()
        .map_err(|e| format!("executor report: {e}"))?;

    let report =
        mmcg::audit_spec::run_with_report(&parsed, &store, &root, since, executor_report.as_ref())?;
    store.ensure_source_snapshot_current()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render_text());
    }

    if let Some(bundle_path) = bundle_path {
        let er_path_str = executor_report_path.map(|p| p.display().to_string());
        let bundle = mmcg::audit_spec::Bundle::from_report_full(
            &report,
            executor_report.as_ref(),
            Some(&parsed),
            er_path_str.as_deref(),
            Some(&root),
        );
        let manifest = bundle.into_manifest(&root)?;
        let envelope = mmcg::audit_bundle::seal_checked(manifest, &root)?;
        let bundle_json = serde_json::to_vec_pretty(&envelope)?;
        mmcg::audit_bundle::write_atomic(bundle_path, &bundle_json, false)
            .map_err(|e| format!("write bundle {}: {e}", bundle_path.display()))?;
        if !json {
            eprintln!("  bundle → {}", bundle_path.display());
        }
    }

    match mmcg::lessons::append_audit_candidate(&root, spec, &report) {
        Ok(true) if !json => {
            eprintln!("  appended lesson candidate → .mastermind/tasks/_lessons.md")
        }
        Err(e) if !json => eprintln!("  warning: lessons append failed: {e}"),
        _ => {}
    }
    if report.has_failures() {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_index_is_optional_without_creating_its_parent() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let index_path = root.join("missing/index.db");

        assert!(open_validated_index(&index_path, &root).unwrap().is_none());
        assert!(!index_path.exists());
        assert!(!root.join("missing").exists());
    }

    #[test]
    fn empty_foreign_index_does_not_bypass_repository_binding() {
        let requested = tempfile::tempdir().unwrap();
        let indexed = tempfile::tempdir().unwrap();
        let requested_root = requested.path().canonicalize().unwrap();
        let indexed_root = indexed.path().canonicalize().unwrap();
        let index_path = indexed_root.join("empty.db");
        let store = mmcg::store::Store::open(&index_path).unwrap();
        store
            .set_meta("index_root", &indexed_root.to_string_lossy())
            .unwrap();
        drop(store);

        let error = match open_validated_index(&index_path, &requested_root) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("foreign index must fail root validation"),
        };
        assert!(error.contains("index/root mismatch"), "{error}");
    }

    #[test]
    fn empty_index_for_the_requested_repository_remains_optional() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let index_path = root.join("empty.db");
        let store = mmcg::store::Store::open(&index_path).unwrap();
        store
            .set_meta("index_root", &root.to_string_lossy())
            .unwrap();
        drop(store);

        assert!(open_validated_index(&index_path, &root).unwrap().is_none());
    }
}
