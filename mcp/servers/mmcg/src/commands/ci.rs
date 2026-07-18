use std::path::{Path, PathBuf};

pub struct CiOpts {
    pub since: String,
    pub root: PathBuf,
    pub bundle_dir: Option<PathBuf>,
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

    if spec_paths.is_empty() {
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
