use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct CiOpts {
    pub since: String,
    pub root: PathBuf,
    pub bundle_dir: Option<PathBuf>,
    pub changed_only: bool,
    pub require_executor_report: bool,
}

fn changed_task_folders(root: &Path, since: &str) -> Result<BTreeSet<PathBuf>, String> {
    if since.starts_with('-') {
        return Err("since ref must not start with '-'".into());
    }
    let range = format!("{since}..HEAD");
    let output = Command::new("git")
        .args([
            "diff",
            "--name-only",
            "-z",
            &range,
            "--",
            ".mastermind/tasks",
        ])
        .current_dir(root)
        .output()
        .map_err(|error| format!("git diff changed tasks: {error}"))?;
    if !output.status.success() {
        return Err("git diff changed tasks failed".into());
    }
    let mut folders = BTreeSet::new();
    for raw in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        let relative = std::str::from_utf8(raw)
            .map_err(|_| "changed task path is not valid UTF-8".to_string())?;
        let path = Path::new(relative);
        if let Some(parent) = path.parent() {
            if parent.parent() == Some(Path::new(".mastermind/tasks")) {
                folders.insert(parent.to_path_buf());
            }
        }
    }
    Ok(folders)
}

pub fn run(opts: CiOpts, index_path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let root = opts
        .root
        .canonicalize()
        .map_err(|e| format!("canonicalize {}: {e}", opts.root.display()))?;

    eprintln!("mastermind ci — {}", root.display());
    eprintln!();

    let mut store = mmcg::store::Store::open(index_path)?;
    let indexer = mmcg::indexer::Indexer::new(&root);
    let stats = indexer.index_all(&mut store, false)?;
    eprintln!(
        "  index: {} symbols, {} edges ({} files indexed, {} unchanged)",
        stats.symbols_total, stats.edges_total, stats.files_indexed, stats.files_unchanged,
    );

    let tasks_dir = root.join(".mastermind").join("tasks");
    if !tasks_dir.is_dir() {
        eprintln!("  no .mastermind/tasks/ — nothing to verify");
        return Ok(true);
    }

    let mut spec_paths: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&tasks_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let spec = path.join("spec.md");
                if spec.is_file() {
                    spec_paths.push(spec);
                }
            }
        }
    }
    spec_paths.sort();

    if opts.changed_only {
        let changed = changed_task_folders(&root, &opts.since)?;
        spec_paths.retain(|spec| {
            spec.parent()
                .and_then(|parent| parent.strip_prefix(&root).ok())
                .is_some_and(|parent| changed.contains(parent))
        });
    }

    if spec_paths.is_empty() {
        if opts.changed_only {
            eprintln!(
                "  [FAIL] no changed task contracts found for {}..HEAD",
                opts.since
            );
            return Ok(false);
        }
        eprintln!("  no spec.md files found in .mastermind/tasks/");
        return Ok(true);
    }

    eprintln!("  specs found: {}", spec_paths.len());
    eprintln!();

    let mut all_ok = true;

    for spec_path in &spec_paths {
        let spec_name = spec_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| spec_path.display().to_string());

        let parsed = match mmcg::spec::parse_file(spec_path) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("  [FAIL] {spec_name} — parse error: {e}");
                all_ok = false;
                continue;
            }
        };

        let verify_report = mmcg::verify_spec::run(&parsed, Some(&store), &root);
        if verify_report.has_failures() {
            eprintln!("  [FAIL] {spec_name} — verify-spec errors:");
            for line in verify_report.render_text().lines() {
                eprintln!("         {line}");
            }
            all_ok = false;
            continue;
        }

        let executor_report_path = spec_path
            .parent()
            .map(|d| d.join("executor-report.md"))
            .filter(|p| p.is_file());

        if executor_report_path.is_none()
            && (opts.require_executor_report || opts.bundle_dir.is_some())
        {
            eprintln!(
                "  [FAIL] {spec_name} — canonical executor-report.md is required for CI evidence"
            );
            all_ok = false;
            continue;
        }

        let executor_report = match executor_report_path
            .as_deref()
            .map(mmcg::executor_report::parse_file)
            .transpose()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  [FAIL] {spec_name} — executor-report parse error: {e}");
                all_ok = false;
                continue;
            }
        };

        let audit_report = match mmcg::audit_spec::run_with_report(
            &parsed,
            &store,
            &root,
            &opts.since,
            executor_report.as_ref(),
        ) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  [FAIL] {spec_name} — audit error: {e}");
                all_ok = false;
                continue;
            }
        };

        let verdict_str = match audit_report.verdict {
            mmcg::audit_spec::Verdict::Held => "HELD",
            mmcg::audit_spec::Verdict::Drift => "DRIFT",
            mmcg::audit_spec::Verdict::Broken => "BROKEN",
        };
        let icon = match audit_report.verdict {
            mmcg::audit_spec::Verdict::Held => "✅",
            mmcg::audit_spec::Verdict::Drift => "⚠️ ",
            mmcg::audit_spec::Verdict::Broken => "❌",
        };

        eprintln!("  {icon} {spec_name} — {verdict_str}");
        for finding in &audit_report.findings {
            eprintln!(
                "       → {}",
                mmcg::audit_spec::render_finding_text(finding)
            );
        }

        if let Some(bundle_dir) = &opts.bundle_dir {
            std::fs::create_dir_all(bundle_dir)
                .map_err(|e| format!("create bundle dir {}: {e}", bundle_dir.display()))?;
            let bundle_name = format!("{spec_name}.bundle.json");
            let bundle_path = bundle_dir.join(&bundle_name);
            let er_path_str = executor_report_path
                .as_deref()
                .map(|p| p.display().to_string());
            let bundle = mmcg::audit_spec::Bundle::from_report_full(
                &audit_report,
                executor_report.as_ref(),
                Some(&parsed),
                er_path_str.as_deref(),
                Some(&root),
            );
            let manifest = bundle
                .into_manifest(&root)
                .map_err(|e| format!("build audit manifest for {spec_name}: {e}"))?;
            let envelope = mmcg::audit_bundle::seal_checked(manifest, &root)
                .map_err(|e| format!("seal audit manifest for {spec_name}: {e}"))?;
            let json = serde_json::to_vec_pretty(&envelope)
                .map_err(|e| format!("serialize bundle for {spec_name}: {e}"))?;
            mmcg::audit_bundle::write_atomic(&bundle_path, &json, false)
                .map_err(|e| format!("write bundle {}: {e}", bundle_path.display()))?;
            eprintln!("       bundle → {}", bundle_path.display());
        }

        if audit_report.has_failures() {
            all_ok = false;
        }
    }

    eprintln!();
    if all_ok {
        eprintln!("mastermind ci — all specs passed");
    } else {
        eprintln!("mastermind ci — FAILED (one or more specs broken)");
    }

    Ok(all_ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn changed_task_folders_only_returns_changed_canonical_tasks() {
        let root = std::env::temp_dir().join(format!(
            "mmcg-ci-changed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join(".mastermind/tasks/001-old")).unwrap();
        fs::create_dir_all(root.join(".mastermind/tasks/002-changed")).unwrap();
        fs::write(root.join(".mastermind/tasks/001-old/spec.md"), "old\n").unwrap();
        fs::write(
            root.join(".mastermind/tasks/002-changed/spec.md"),
            "before\n",
        )
        .unwrap();
        fs::write(root.join("README.md"), "before\n").unwrap();

        git(&root, &["init", "-q", "--initial-branch=main"]);
        git(&root, &["config", "user.email", "t@t"]);
        git(&root, &["config", "user.name", "t"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "baseline"]);
        let baseline = git(&root, &["rev-parse", "HEAD"]);

        fs::write(
            root.join(".mastermind/tasks/002-changed/executor-report.md"),
            "report\n",
        )
        .unwrap();
        fs::write(root.join("README.md"), "after\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "change one task"]);

        let changed = changed_task_folders(&root, &baseline).unwrap();
        assert_eq!(
            changed,
            BTreeSet::from([PathBuf::from(".mastermind/tasks/002-changed")])
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn changed_task_folders_rejects_option_like_refs() {
        assert!(changed_task_folders(Path::new("."), "--output=/tmp/x").is_err());
    }

    #[test]
    fn changed_only_ci_fails_when_executor_report_is_missing() {
        let root = std::env::temp_dir().join(format!(
            "mmcg-ci-report-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.py"), "def value(): return 1\n").unwrap();
        git(&root, &["init", "-q", "--initial-branch=main"]);
        git(&root, &["config", "user.email", "t@t"]);
        git(&root, &["config", "user.name", "t"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "baseline"]);
        let baseline = git(&root, &["rev-parse", "HEAD"]);

        let task_dir = root.join(".mastermind/tasks/001-change");
        fs::create_dir_all(&task_dir).unwrap();
        fs::write(root.join("src/lib.py"), "def value(): return 2\n").unwrap();
        fs::write(
            task_dir.join("spec.md"),
            "---\nmode: verified\ntouches:\n  - file: src/lib.py\n---\n# Change\n## Goals\n- Change value\n## Scope\n- `src/lib.py`\n## Acceptance Criteria\n- [ ] Value is two\n## Tests Plan\n- focused test\n## Final Verification\n- repository gate\n",
        )
        .unwrap();
        git(&root, &["add", "-A"]);
        git(
            &root,
            &["commit", "-q", "-m", "changed task without report"],
        );

        let ok = run(
            CiOpts {
                since: baseline,
                root: root.clone(),
                bundle_dir: None,
                changed_only: true,
                require_executor_report: true,
            },
            &root.join(".mastermind/mmcg.db"),
        )
        .unwrap();
        assert!(!ok, "CI evidence must fail closed without executor report");
        fs::remove_dir_all(root).ok();
    }
}
