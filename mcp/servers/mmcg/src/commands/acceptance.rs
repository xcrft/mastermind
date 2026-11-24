use std::path::{Path, PathBuf};

pub fn status(spec: &Path, root: PathBuf, json: bool) -> Result<bool, Box<dyn std::error::Error>> {
    let (root, spec) = super::task_paths(spec, root)?;
    let report = mmcg::acceptance::inspect(&spec, &root)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render_text());
    }
    Ok(report.requirements_satisfied())
}
