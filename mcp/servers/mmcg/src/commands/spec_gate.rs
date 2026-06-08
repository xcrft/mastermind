use std::path::{Path, PathBuf};

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
    let store = mmcg::store::Store::open(index_path).ok();
    let mut report = mmcg::verify_spec::run(&parsed, store.as_ref(), &root);
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
) -> Result<(), Box<dyn std::error::Error>> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
    let parsed =
        mmcg::spec::parse_file(spec).map_err(|e| format!("parse {}: {e}", spec.display()))?;
    let store = mmcg::store::Store::open(index_path)?;
    let report = mmcg::audit_spec::run(&parsed, &store, &root, since)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render_text());
    }
    match mmcg::lessons::append_if_drift_or_broken(&root, spec, &report) {
        Ok(true) if !json => eprintln!("  appended lesson → .mastermind/tasks/_lessons.md"),
        Err(e) if !json => eprintln!("  warning: lessons append failed: {e}"),
        _ => {}
    }
    if report.has_failures() {
        std::process::exit(1);
    }
    Ok(())
}
