use std::path::{Path, PathBuf};

pub enum Action<'a> {
    Prepare,
    FollowUp,
    Run(&'a mmcg::review_invocation::Options),
    Submit(&'a Path),
    Status,
}

pub fn dispatch(
    spec: &Path,
    root: PathBuf,
    action: Action<'_>,
    json: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    let root = std::path::absolute(root)?;
    let spec = if spec.is_absolute() {
        spec.to_path_buf()
    } else {
        root.join(spec)
    };
    let report = match action {
        Action::FollowUp => {
            let packet = mmcg::task_review::follow_up(&spec, &root)?;
            println!("{}", serde_json::to_string_pretty(&packet)?);
            return Ok(true);
        }
        Action::Prepare => {
            let request = mmcg::task_review::prepare(&spec, &root)?;
            // The request is also the portable interface for an external reviewer.
            println!("{}", serde_json::to_string_pretty(&request)?);
            return Ok(true);
        }
        Action::Run(options) => mmcg::review_invocation::run(&spec, &root, options)?,
        Action::Submit(path) => {
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            };
            mmcg::task_review::submit(&spec, &root, &path)?
        }
        Action::Status => mmcg::task_review::inspect(&spec, &root)?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render_text());
    }
    Ok(report.approved())
}
