//! Formatter and linter scopes of a mined repository. A convention that a
//! repository's own tooling decides belongs to the repository, not to the
//! author, so the profile counts it apart from personal style.
//!
//! Detection is heuristic and reads the mined snapshot, not the working tree:
//! a configuration file governs its directory, a tool invoked from CI or task
//! files governs the whole repository, and Go is always gofmt-formatted. The
//! current configuration is applied to older commits as well, which errs toward
//! withholding a personal claim rather than inventing one.

use crate::diff::{run_bounded_git_with_limit, WorkingTreeDiffError};
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) const INDENT: u8 = 1;
pub(super) const BRACE: u8 = 2;
pub(super) const LINE_LENGTH: u8 = 4;
pub(super) const QUOTES: u8 = 8;
pub(super) const DECLARATION: u8 = 16;
const FORMATTED: u8 = INDENT | BRACE | LINE_LENGTH | QUOTES;

/// Tool names, indexed by `ToolScope::tool` and by the bits of `Governed::tools`.
pub(super) const TOOLS: [&str; 11] = [
    "gofmt",
    "rustfmt",
    "prettier",
    "biome",
    "dprint",
    "black",
    "ruff",
    "clang-format",
    "dotnet-format",
    "editorconfig",
    "eslint",
];
const GOFMT: usize = 0;
const RUSTFMT: usize = 1;
const PRETTIER: usize = 2;
const BIOME: usize = 3;
const DPRINT: usize = 4;
const BLACK: usize = 5;
const RUFF: usize = 6;
const CLANG_FORMAT: usize = 7;
const DOTNET_FORMAT: usize = 8;
const EDITORCONFIG: usize = 9;
const ESLINT: usize = 10;

const RUST: &[&str] = &["rs"];
const SCRIPT: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];
const PYTHON: &[&str] = &["py"];
const GO: &[&str] = &["go"];
const C_FAMILY: &[&str] = &["c", "h", "cc", "cpp", "cxx", "hpp", "hh"];
const CSHARP: &[&str] = &["cs"];
/// Every source extension.
const ANY: &[&str] = &[];

/// Substrings that show a CI or task file runs a tool over the repository.
const INVOCATIONS: [(&[&str], usize, &[&str], u8); 8] = [
    (&["cargo fmt", "rustfmt"], RUSTFMT, RUST, FORMATTED),
    (&["prettier"], PRETTIER, SCRIPT, FORMATTED),
    (
        &["biome format", "biome check", "biome ci"],
        BIOME,
        SCRIPT,
        FORMATTED,
    ),
    (
        &["psf/black", "black .", "black --"],
        BLACK,
        PYTHON,
        FORMATTED,
    ),
    (&["ruff format", "ruff-format"], RUFF, PYTHON, FORMATTED),
    (&["clang-format"], CLANG_FORMAT, C_FAMILY, FORMATTED),
    (
        &["dotnet format", "dotnet-format"],
        DOTNET_FORMAT,
        CSHARP,
        FORMATTED,
    ),
    (&["eslint"], ESLINT, SCRIPT, DECLARATION),
];

/// Configuration and task files read for their contents, bounding both the
/// number of reads and, through the git output limit, their total size.
const MAX_CONTENT_READS: usize = 400;
const LISTING_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const CONTENT_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

/// One tool governing source files with `extensions` under `dir`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ToolScope {
    /// Directory prefix with a trailing `/`, or empty for the whole repository.
    dir: String,
    tool: usize,
    extensions: &'static [&'static str],
    features: u8,
}

/// What tooling decides for one file: feature bits and the tools involved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Governed {
    pub(super) features: u8,
    pub(super) tools: u16,
}

pub(super) fn governed(scopes: &[ToolScope], path: &str) -> Governed {
    let extension = path.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    scopes
        .iter()
        .filter(|scope| {
            path.starts_with(&scope.dir)
                && (scope.extensions.is_empty() || scope.extensions.contains(&extension))
        })
        .fold(Governed::default(), |governed, scope| Governed {
            features: governed.features | scope.features,
            tools: governed.tools | (1 << scope.tool),
        })
}

/// Stable identity of the detected scopes. Stored commit tallies were split by
/// it, so a change must measure those commits again.
pub(super) fn fingerprint(scopes: &[ToolScope]) -> String {
    let mut digest = Sha256::new();
    for scope in scopes {
        digest.update(scope.dir.as_bytes());
        digest.update([0, scope.tool as u8, scope.features, 0]);
        digest.update(scope.extensions.join(",").as_bytes());
        digest.update([0]);
    }
    crate::hex::encode(&digest.finalize()[..8])
}

/// NUL-separated file paths at `history_ref`, or `None` when the listing is too
/// large to read. Tooling and range detection share one listing.
pub(super) fn listing(
    root: &Path,
    history_ref: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match run_bounded_git_with_limit(
        root,
        &["ls-tree", "-r", "-z", "--name-only", history_ref],
        None,
        LISTING_OUTPUT_LIMIT,
    ) {
        Ok(out) if out.success => Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned())),
        Ok(_) => Err("git ls-tree exited unsuccessfully".into()),
        Err(WorkingTreeDiffError::GitOutputLimit) => Ok(None),
        Err(error) => Err(format!("git ls-tree failed: {error}").into()),
    }
}

/// Formatter and linter scopes at `history_ref`. Without a listing only the
/// gofmt convention remains; missing configuration never fails a mine.
pub(super) fn detect(
    root: &Path,
    history_ref: &str,
    listing: Option<&str>,
) -> Result<Vec<ToolScope>, Box<dyn std::error::Error>> {
    let mut scopes = vec![scope("", GOFMT, GO, FORMATTED)];
    let Some(listing) = listing else {
        return Ok(scopes);
    };
    let mut content_paths = Vec::new();
    for path in listing.split('\0').filter(|path| !path.is_empty()) {
        let (dir, name) = match path.rsplit_once('/') {
            Some((dir, name)) => (&path[..dir.len() + 1], name),
            None => ("", path),
        };
        match name {
            "rustfmt.toml" | ".rustfmt.toml" => scopes.push(scope(dir, RUSTFMT, RUST, FORMATTED)),
            "biome.json" | "biome.jsonc" => scopes.push(scope(dir, BIOME, SCRIPT, FORMATTED)),
            "dprint.json" | ".dprint.json" => scopes.push(scope(dir, DPRINT, SCRIPT, FORMATTED)),
            ".clang-format" => scopes.push(scope(dir, CLANG_FORMAT, C_FAMILY, FORMATTED)),
            ".editorconfig" => scopes.push(scope(dir, EDITORCONFIG, ANY, INDENT)),
            "ruff.toml" | ".ruff.toml" => scopes.push(scope(dir, RUFF, PYTHON, FORMATTED)),
            name if name.starts_with(".prettierrc") || name.starts_with("prettier.config.") => {
                scopes.push(scope(dir, PRETTIER, SCRIPT, FORMATTED));
            }
            name if name.starts_with(".eslintrc") || name.starts_with("eslint.config.") => {
                scopes.push(scope(dir, ESLINT, SCRIPT, DECLARATION));
            }
            _ if wants_contents(path, name) && !path.contains('\n') => content_paths.push(path),
            _ => {}
        }
    }
    content_paths.truncate(MAX_CONTENT_READS);
    for (path, text) in read_snapshot_files(root, history_ref, &content_paths)? {
        scopes_from_contents(&path, &text, &mut scopes);
    }
    scopes.sort();
    scopes.dedup();
    Ok(scopes)
}

fn scope(dir: &str, tool: usize, extensions: &'static [&'static str], features: u8) -> ToolScope {
    ToolScope {
        dir: dir.to_string(),
        tool,
        extensions,
        features,
    }
}

fn wants_contents(path: &str, name: &str) -> bool {
    matches!(
        name,
        "pyproject.toml"
            | "package.json"
            | ".pre-commit-config.yaml"
            | "Makefile"
            | "justfile"
            | "Justfile"
            | ".gitlab-ci.yml"
            | "Taskfile.yml"
            | "lefthook.yml"
    ) || path.starts_with(".github/workflows/")
        || path.starts_with(".circleci/")
        || (name.starts_with("azure-pipelines")
            && (name.ends_with(".yml") || name.ends_with(".yaml")))
}

fn scopes_from_contents(path: &str, text: &str, scopes: &mut Vec<ToolScope>) {
    let (dir, name) = match path.rsplit_once('/') {
        Some((dir, name)) => (&path[..dir.len() + 1], name),
        None => ("", path),
    };
    match name {
        "pyproject.toml" => {
            if text.contains("[tool.black") {
                scopes.push(scope(dir, BLACK, PYTHON, FORMATTED));
            }
            if text.contains("[tool.ruff") {
                scopes.push(scope(dir, RUFF, PYTHON, FORMATTED));
            }
        }
        "package.json" => {
            for (dependency, tool, features) in [
                ("\"prettier\"", PRETTIER, FORMATTED),
                ("\"@biomejs/biome\"", BIOME, FORMATTED),
                ("\"dprint\"", DPRINT, FORMATTED),
                ("\"eslint\"", ESLINT, DECLARATION),
            ] {
                if text.contains(dependency) {
                    scopes.push(scope(dir, tool, SCRIPT, features));
                }
            }
            push_invocations(dir, text, scopes);
        }
        // CI and task files run their tools over the whole repository.
        _ => push_invocations("", text, scopes),
    }
}

fn push_invocations(dir: &str, text: &str, scopes: &mut Vec<ToolScope>) {
    for (needles, tool, extensions, features) in INVOCATIONS {
        if needles.iter().any(|needle| text.contains(needle)) {
            scopes.push(scope(dir, tool, extensions, features));
        }
    }
}

/// Blob contents at `history_ref` through one `git cat-file --batch`. Missing
/// objects are skipped; output over the limit reads nothing, so only the
/// file-name scopes remain.
pub(super) fn read_snapshot_files(
    root: &Path,
    history_ref: &str,
    paths: &[&str],
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let input: String = paths
        .iter()
        .map(|path| format!("{history_ref}:{path}\n"))
        .collect();
    let out = match run_bounded_git_with_limit(
        root,
        &["cat-file", "--batch"],
        Some(input.as_bytes()),
        CONTENT_OUTPUT_LIMIT,
    ) {
        Ok(out) if out.success => out.stdout,
        Ok(_) => return Err("git cat-file exited unsuccessfully".into()),
        Err(WorkingTreeDiffError::GitOutputLimit) => return Ok(Vec::new()),
        Err(error) => return Err(format!("git cat-file failed: {error}").into()),
    };
    Ok(parse_cat_file_batch(&out, paths))
}

/// `<oid> <type> <size>\n<bytes>\n` per found object, `<name> missing\n` otherwise,
/// in request order.
fn parse_cat_file_batch(mut out: &[u8], paths: &[&str]) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for path in paths {
        let Some(end) = out.iter().position(|&byte| byte == b'\n') else {
            break;
        };
        let header = String::from_utf8_lossy(&out[..end]).into_owned();
        out = &out[end + 1..];
        let mut fields = header.split(' ');
        let (Some(_), Some(kind), Some(size)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(size) = size.parse::<usize>() else {
            break;
        };
        if out.len() < size {
            break;
        }
        if kind == "blob" {
            files.push((
                path.to_string(),
                String::from_utf8_lossy(&out[..size]).into_owned(),
            ));
        }
        out = &out[(size + 1).min(out.len())..];
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(root: &Path, args: &[&str]) {
        let mut command = Command::new("git");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        let out = command
            .arg("-C")
            .arg(root)
            .args(["-c", "commit.gpgsign=false", "-c", "core.hooksPath="])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", root.join(".unused-global-git-config"))
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    }

    fn names(scopes: &[ToolScope], path: &str) -> Vec<&'static str> {
        let governed = governed(scopes, path);
        (0..TOOLS.len())
            .filter(|bit| governed.tools & (1 << bit) != 0)
            .map(|bit| TOOLS[bit])
            .collect()
    }

    #[test]
    fn detects_config_dependency_and_ci_scopes_at_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (path, text) in [
            ("engine/rustfmt.toml", "max_width = 100\n"),
            (
                "web/package.json",
                "{\"devDependencies\": {\"prettier\": \"3\"}}\n",
            ),
            ("tools/pyproject.toml", "[tool.ruff]\nline-length = 88\n"),
            (
                ".github/workflows/ci.yml",
                "steps:\n  - run: cargo fmt --check\n",
            ),
            ("api/.eslintrc.json", "{}\n"),
        ] {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        git(root, &["init", "-q"]);
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.name=A",
                "-c",
                "user.email=a@x",
                "commit",
                "-qm",
                "init",
            ],
        );
        // Uncommitted configuration is not part of the mined snapshot.
        std::fs::write(root.join("biome.json"), "{}\n").unwrap();

        let listing = listing(root, "HEAD").unwrap();
        let scopes = detect(root, "HEAD", listing.as_deref()).unwrap();
        assert_eq!(names(&scopes, "engine/src/lib.rs"), ["rustfmt"]);
        assert_eq!(
            names(&scopes, "cli/main.rs"),
            ["rustfmt"],
            "CI formats all Rust"
        );
        assert_eq!(names(&scopes, "web/src/app.ts"), ["prettier"]);
        assert!(
            names(&scopes, "cli/app.ts").is_empty(),
            "prettier stays in web/"
        );
        assert_eq!(names(&scopes, "tools/run.py"), ["ruff"]);
        assert!(names(&scopes, "scripts/run.py").is_empty());
        assert_eq!(names(&scopes, "api/handler.ts"), ["eslint"]);
        assert_eq!(governed(&scopes, "api/handler.ts").features, DECLARATION);
        assert_eq!(names(&scopes, "svc/main.go"), ["gofmt"]);

        let mut changed = scopes.clone();
        changed.push(scope("", BIOME, SCRIPT, FORMATTED));
        assert_ne!(fingerprint(&scopes), fingerprint(&changed));
        assert_eq!(
            fingerprint(&scopes),
            fingerprint(&detect(root, "HEAD", listing.as_deref()).unwrap())
        );
    }

    #[test]
    fn cat_file_batch_skips_missing_objects() {
        let out = b"abc blob 5\nhello\nHEAD:gone missing\ndef blob 3\nbye\n";
        assert_eq!(
            parse_cat_file_batch(out, &["a.txt", "gone", "b.txt"]),
            vec![
                ("a.txt".to_string(), "hello".to_string()),
                ("b.txt".to_string(), "bye".to_string()),
            ]
        );
    }
}
