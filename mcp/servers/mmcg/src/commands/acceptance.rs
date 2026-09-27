use std::path::{Path, PathBuf};

pub fn status(spec: &Path, root: PathBuf, json: bool) -> Result<bool, Box<dyn std::error::Error>> {
    let root = std::path::absolute(root)?;
    let spec = if spec.is_absolute() {
        spec.to_path_buf()
    } else {
        root.join(spec)
    };
    let report = mmcg::acceptance::inspect(&spec, &root)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render_text());
    }
    Ok(report.requirements_satisfied())
}
