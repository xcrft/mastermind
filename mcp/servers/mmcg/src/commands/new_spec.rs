use std::fs;
use std::path::Path;

pub enum Mode {
    Lite,
    Strict,
}

impl Mode {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "lite" => Ok(Mode::Lite),
            "strict" => Ok(Mode::Strict),
            other => Err(format!("unknown mode {other:?} — use `lite` or `strict`")),
        }
    }
}

pub fn run(description: &str, mode: Mode, root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let tasks_dir = root.join(".mastermind").join("tasks");
    if !tasks_dir.exists() {
        fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("create .mastermind/tasks/: {e}"))?;
    }

    let next_n = next_task_number(&tasks_dir)?;
    let slug = slugify(description);
    let dir_name = format!("{:03}-{}", next_n, slug);
    let task_dir = tasks_dir.join(&dir_name);
    fs::create_dir_all(&task_dir)
        .map_err(|e| format!("create {}: {e}", task_dir.display()))?;

    let spec_path = task_dir.join("spec.md");
    let content = render_spec(description, next_n, &mode);
    fs::write(&spec_path, &content)
        .map_err(|e| format!("write {}: {e}", spec_path.display()))?;

    println!("Created {}", spec_path.display());
    Ok(())
}

fn next_task_number(tasks_dir: &Path) -> Result<u32, Box<dyn std::error::Error>> {
    let mut max: u32 = 0;
    if let Ok(entries) = fs::read_dir(tasks_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if let Some(prefix) = s.split('-').next() {
                if let Ok(n) = prefix.parse::<u32>() {
                    if n > max {
                        max = n;
                    }
                }
            }
        }
    }
    Ok(max + 1)
}

fn slugify(s: &str) -> String {
    let slug: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.len() > 40 {
        slug[..40].trim_end_matches('-').to_string()
    } else {
        slug
    }
}

fn render_spec(description: &str, n: u32, mode: &Mode) -> String {
    let id = format!("{:03}", n);
    match mode {
        Mode::Lite => format!(
            "---\nid: \"{id}\"\ntitle: {description}\nrisk: low\n---\n\n\
            # Goal\n\n{description}\n\n\
            # Scope\n\n\
            # Pre-edit snapshot\n\n\
            # Verify\n"
        ),
        Mode::Strict => format!(
            "---\nid: \"{id}\"\ntitle: {description}\nrisk: medium\n\n\
            touches:\n  - file: <path>\n    language: <lang>\n    symbols:\n      - name: <symbol>\n        callers: 0\n\n\
            verify:\n  - cmd: \"\"\n\n\
            expected_docs: []\n---\n\n\
            # Goal\n\n{description}\n\n\
            # Scope\n\n\
            # Pre-edit snapshot\n\n\
            # Tests Plan\n\n\
            # Documentation Plan\n\n\
            # Observability Plan\n\n\
            # Performance Considerations\n\n\
            # Rollback / Migration\n"
        ),
    }
}
