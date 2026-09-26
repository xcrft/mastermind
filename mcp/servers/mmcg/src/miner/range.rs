//! The author's range: which languages, areas and libraries their commits
//! touch, how often, and how recently. Counts are commits, not lines, and
//! recency is relative to the newest mined commit so a profile renders the
//! same way on every run. Range says where someone has worked, not how well.

use super::stats::{bump, cget};
use super::store::{CommitEvidence, Counts};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Commits newer than this many days before the newest mined commit are recent.
const RECENT_DAYS: i64 = 183;
const TOP_LANGUAGES: usize = 8;
const TOP_AREAS: usize = 8;
const TOP_LIBRARIES: usize = 12;

/// Directory names that group components rather than name one, so the area is
/// the next level down (`packages/billing`, not `packages`).
const CONTAINERS: [&str; 17] = [
    "src",
    "lib",
    "libs",
    "app",
    "apps",
    "packages",
    "services",
    "crates",
    "backend",
    "frontend",
    "cmd",
    "internal",
    "pkg",
    "modules",
    "components",
    "projects",
    "tools",
];

/// Python standard-library modules; importing them says nothing about range.
const PYTHON_STDLIB: [&str; 79] = [
    "__future__",
    "abc",
    "argparse",
    "array",
    "ast",
    "asyncio",
    "atexit",
    "base64",
    "bisect",
    "calendar",
    "codecs",
    "collections",
    "concurrent",
    "configparser",
    "contextlib",
    "copy",
    "csv",
    "ctypes",
    "dataclasses",
    "datetime",
    "decimal",
    "difflib",
    "email",
    "enum",
    "errno",
    "fnmatch",
    "fractions",
    "functools",
    "getpass",
    "glob",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "locale",
    "logging",
    "math",
    "multiprocessing",
    "operator",
    "os",
    "pathlib",
    "pickle",
    "platform",
    "pprint",
    "queue",
    "random",
    "re",
    "secrets",
    "shlex",
    "shutil",
    "signal",
    "socket",
    "sqlite3",
    "statistics",
    "string",
    "struct",
    "subprocess",
    "sys",
    "tempfile",
    "textwrap",
    "threading",
    "time",
    "traceback",
    "typing",
    "unittest",
    "urllib",
    "uuid",
    "warnings",
    "weakref",
    "xml",
    "zipfile",
];

/// Node.js core modules; importing them says nothing about range.
const NODE_CORE: [&str; 16] = [
    "assert",
    "buffer",
    "child_process",
    "crypto",
    "events",
    "fs",
    "http",
    "https",
    "net",
    "os",
    "path",
    "process",
    "stream",
    "timers",
    "url",
    "util",
];

/// Manifests read to learn a repository's own package names.
const MAX_MANIFESTS: usize = 256;

/// Packages and modules a repository defines itself: Cargo package names and
/// Rust module files, `package.json` names, Python modules and the directories
/// holding them, and Go modules. Imports of them are the repository's own
/// code, not libraries; a local module named like a library hides it.
#[derive(Debug, Default)]
pub(super) struct FirstParty {
    rust: BTreeSet<String>,
    python: BTreeSet<String>,
    script: BTreeSet<String>,
    go_modules: Vec<String>,
}

impl FirstParty {
    pub(super) fn from_snapshot(
        root: &Path,
        history_ref: &str,
        listing: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut first_party = FirstParty::default();
        let Some(listing) = listing else {
            return Ok(first_party);
        };
        let mut manifests = Vec::new();
        for path in listing.split('\0').filter(|path| !path.is_empty()) {
            let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
            let parent = dir.rsplit('/').next().filter(|parent| !parent.is_empty());
            match name {
                "Cargo.toml" | "package.json" | "go.mod" if !path.contains('\n') => {
                    manifests.push(path)
                }
                "mod.rs" => first_party.rust.extend(parent.map(str::to_string)),
                _ if name.ends_with(".rs") => {
                    first_party
                        .rust
                        .insert(name.trim_end_matches(".rs").to_string());
                }
                _ if name.ends_with(".py") => {
                    let module = name.trim_end_matches(".py");
                    if !module.starts_with("__") {
                        first_party.python.insert(module.to_string());
                    }
                    first_party.python.extend(parent.map(str::to_string));
                }
                _ => {}
            }
        }
        manifests.truncate(MAX_MANIFESTS);
        for (path, text) in super::tooling::read_snapshot_files(root, history_ref, &manifests)? {
            if path.ends_with("Cargo.toml") {
                if let Some(name) = cargo_package_name(&text) {
                    first_party.rust.insert(name.replace('-', "_"));
                }
            } else if path.ends_with("package.json") {
                let name = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|manifest| Some(manifest.get("name")?.as_str()?.to_string()));
                first_party.script.extend(name);
            } else if let Some(module) = text
                .lines()
                .find_map(|line| line.trim().strip_prefix("module "))
            {
                first_party.go_modules.push(module.trim().to_string());
            }
        }
        Ok(first_party)
    }

    /// Every input to import classification participates in cache validity.
    /// Ordered sets make equivalent package listings share a fingerprint.
    pub(super) fn fingerprint(&self) -> String {
        let go_modules: BTreeSet<_> = self.go_modules.iter().collect();
        let context = serde_json::to_vec(&(&self.rust, &self.python, &self.script, go_modules))
            .expect("sets of strings serialize");
        let mut digest = Sha256::new();
        digest.update(b"mastermind-first-party-v1\0");
        digest.update(context);
        crate::hex::encode(&digest.finalize())
    }

    /// Whether a `Language:name` library is the repository's own code.
    pub(super) fn contains(&self, library: &str) -> bool {
        let Some((language, name)) = library.split_once(':') else {
            return false;
        };
        match language {
            "Rust" => self.rust.contains(name),
            "Python" => self.python.contains(name),
            "TypeScript" | "JavaScript" => self.script.contains(name),
            "Go" => {
                name.contains("/internal/")
                    || self
                        .go_modules
                        .iter()
                        .any(|module| name == module || name.starts_with(&format!("{module}/")))
            }
            _ => false,
        }
    }
}

/// `name` from the `[package]` table of a Cargo manifest.
fn cargo_package_name(manifest: &str) -> Option<&str> {
    let mut in_package = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package {
            let value = line
                .strip_prefix("name")?
                .trim_start()
                .strip_prefix('=')?
                .trim();
            return value.strip_prefix('"')?.split('"').next();
        }
    }
    None
}

/// Display language of a path, broader than the style detectors: infrastructure
/// and query languages are part of someone's range.
pub(super) fn language(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name == "Dockerfile" || name.starts_with("Dockerfile.") {
        return Some("Docker");
    }
    let extension = name.rsplit_once('.')?.1;
    Some(match extension {
        "rs" => "Rust",
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        "cs" => "C#",
        "php" => "PHP",
        "rb" => "Ruby",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "C++",
        "scala" => "Scala",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "sql" => "SQL",
        "tf" | "hcl" => "HCL",
        "sh" | "bash" | "zsh" => "Shell",
        "ps1" | "psm1" => "PowerShell",
        "proto" => "Protobuf",
        "graphql" | "gql" => "GraphQL",
        _ => return None,
    })
}

/// Names that reach `style.md` and agents: letters, digits and the path and
/// package punctuation that library and directory names use, nothing that can
/// open markup.
fn inert(name: &str) -> bool {
    name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || "@/._+:- ".contains(c))
}

/// The component a path belongs to within its repository.
pub(super) fn area(path: &str) -> String {
    let mut parts = path.split('/');
    let first = parts.next().unwrap_or(path);
    let second = parts.next();
    let area = match (second, parts.next()) {
        (None, _) => return "(root)".to_string(),
        (Some(second), Some(_)) if CONTAINERS.contains(&first) => format!("{first}/{second}"),
        _ => first.to_string(),
    };
    if inert(&area) {
        area
    } else {
        "(other)".to_string()
    }
}

/// The library an added import line names, tagged with its language. Relative
/// imports, the Rust standard library and common Python standard modules are
/// not range, and a name with unexpected characters is dropped.
pub(super) fn imported_library(language: &str, line: &str) -> Option<String> {
    library_name(language, line).filter(|name| inert(name))
}

fn library_name(language: &str, line: &str) -> Option<String> {
    let line = line.trim_start();
    let library = match language {
        "Rust" => {
            let path = line
                .strip_prefix("pub use ")
                .or(line.strip_prefix("use "))?;
            let root = path.split([':', ';', '{', ' ']).next()?;
            (!matches!(root, "crate" | "self" | "super" | "std" | "core" | "alloc"))
                .then_some(root)?
        }
        "Python" => {
            let module = line
                .strip_prefix("from ")
                .or(line.strip_prefix("import "))?
                .split([' ', ',', '.'])
                .next()?;
            (!module.is_empty() && !PYTHON_STDLIB.contains(&module)).then_some(module)?
        }
        "TypeScript" | "JavaScript" => {
            let quoted = line
                .rsplit_once(" from ")
                .map(|(_, source)| source)
                .or_else(|| line.strip_prefix("import "))
                .or_else(|| line.split_once("require(").map(|(_, source)| source))?
                .trim_start();
            let quote = quoted.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
            let specifier = quoted[1..].split(quote).next()?;
            if specifier.is_empty()
                || specifier.starts_with(['.', '/'])
                || specifier.starts_with("node:")
                || NODE_CORE.contains(&specifier.split('/').next().unwrap_or(specifier))
            {
                return None;
            }
            let mut segments = specifier.split('/');
            let head = segments.next()?;
            // `@/…` and `~/…` are tsconfig path aliases into the repository itself.
            if head == "@" || head.starts_with('~') {
                return None;
            }
            if head.starts_with('@') {
                return Some(format!("{language}:{head}/{}", segments.next()?));
            }
            head
        }
        "Go" => {
            let specifier = line.strip_prefix("import ").unwrap_or(line).trim();
            let specifier = specifier.strip_prefix('"')?.split('"').next()?;
            let segments: Vec<&str> = specifier.split('/').collect();
            if !segments[0].contains('.') {
                return None;
            }
            return Some(format!(
                "{language}:{}",
                segments[..segments.len().min(3)].join("/")
            ));
        }
        _ => return None,
    };
    Some(format!("{language}:{library}"))
}

/// Range tallies of one commit.
pub(super) fn tally(
    languages: &BTreeSet<&'static str>,
    areas: &BTreeSet<String>,
    libraries: &BTreeSet<String>,
    c: &mut Counts,
) {
    for language in languages {
        bump(c, &format!("range.lang.{language}"), 1);
    }
    for area in areas {
        bump(c, &format!("range.area.{area}"), 1);
    }
    for library in libraries {
        bump(c, &format!("range.lib.{library}"), 1);
    }
}

/// The `## Range` bullets, or nothing when no commit touched a known language.
pub(super) fn render(commits: &[CommitEvidence]) -> String {
    let newest = commits
        .iter()
        .filter_map(|commit| day_number(&commit.authored_at))
        .max();
    let mut entries: BTreeMap<String, (usize, usize, &str)> = BTreeMap::new();
    for commit in commits {
        let day = day_number(&commit.authored_at);
        let recent =
            matches!((day, newest), (Some(day), Some(newest)) if newest - day <= RECENT_DAYS);
        for key in commit.counts.keys().filter(|key| key.starts_with("range.")) {
            if cget(&commit.counts, key) == 0 {
                continue;
            }
            let entry = entries.entry(key.clone()).or_insert((0, 0, ""));
            entry.0 += 1;
            entry.1 += usize::from(recent);
            if commit.authored_at.as_str() > entry.2 {
                entry.2 = &commit.authored_at;
            }
        }
    }
    let section = |prefix: &str, top: usize| -> Vec<String> {
        let mut rows: Vec<(&str, &(usize, usize, &str))> = entries
            .iter()
            .filter_map(|(key, entry)| Some((key.strip_prefix(prefix)?, entry)))
            .collect();
        rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(b.0)));
        rows.iter()
            .take(top)
            .map(|(name, (commits, recent, last))| {
                let last = last.get(..7).unwrap_or(last);
                format!("{name} ({commits}; {recent} recent; last {last})")
            })
            .collect()
    };
    let languages = section("range.lang.", TOP_LANGUAGES);
    if languages.is_empty() {
        return String::new();
    }
    let mut out = format!("- **Languages:** {}.\n", languages.join(", "));
    let areas = section("range.area.", TOP_AREAS);
    if !areas.is_empty() {
        out.push_str(&format!("- **Areas:** {}.\n", areas.join(", ")));
    }
    let libraries = section("range.lib.", TOP_LIBRARIES);
    if !libraries.is_empty() {
        out.push_str(&format!(
            "- **Libraries in added imports** (sampled diffs only): {}.\n",
            libraries.join(", ")
        ));
    }
    out
}

/// Libraries that appear in the same commits as any of `areas`, most frequent
/// first: the author's associations for the code being changed. Co-occurrence
/// is counted in commits and says nothing about causality.
pub(super) fn associations(
    commits: &[CommitEvidence],
    areas: &BTreeSet<String>,
    top: usize,
) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for commit in commits {
        let touches = commit.counts.keys().any(|key| {
            key.strip_prefix("range.area.")
                .is_some_and(|area| areas.contains(area))
        });
        if !touches {
            continue;
        }
        for name in commit
            .counts
            .keys()
            .filter_map(|key| key.strip_prefix("range.lib."))
        {
            *counts.entry(name).or_insert(0) += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(name, commits)| (name.to_string(), commits))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(top);
    ranked
}

/// Days since 1970-01-01 for a `YYYY-MM-DD` date, without a date library.
fn day_number(date: &str) -> Option<i64> {
    let mut parts = date.get(..10)?.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    // Days-from-civil (Howard Hinnant), valid for the proleptic Gregorian calendar.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_map_to_languages_and_component_areas() {
        assert_eq!(language("infra/main.tf"), Some("HCL"));
        assert_eq!(language("deploy/Dockerfile"), Some("Docker"));
        assert_eq!(language("README.md"), None);
        assert_eq!(area("mcp/src/server.rs"), "mcp");
        assert_eq!(area("packages/billing/src/index.ts"), "packages/billing");
        assert_eq!(area("src/lib.rs"), "src");
        assert_eq!(area("Cargo.toml"), "(root)");
        assert_eq!(area("*weird*/x.rs"), "(other)");
    }

    #[test]
    fn imports_name_external_libraries_only() {
        let cases = [
            ("Rust", "use tokio::sync::mpsc;", Some("Rust:tokio")),
            ("Rust", "use crate::store::Counts;", None),
            ("Rust", "use std::path::Path;", None),
            (
                "Python",
                "from fastapi import APIRouter",
                Some("Python:fastapi"),
            ),
            ("Python", "import numpy as np", Some("Python:numpy")),
            ("Python", "from .models import Item", None),
            ("Python", "import json", None),
            (
                "TypeScript",
                "import { z } from 'zod';",
                Some("TypeScript:zod"),
            ),
            (
                "TypeScript",
                "import { Hono } from \"@hono/node-server/serve\";",
                Some("TypeScript:@hono/node-server"),
            ),
            ("TypeScript", "import { x } from './local';", None),
            (
                "JavaScript",
                "const fs = require('fs-extra');",
                Some("JavaScript:fs-extra"),
            ),
            (
                "Go",
                "\"github.com/spf13/cobra/doc\"",
                Some("Go:github.com/spf13/cobra"),
            ),
            ("Go", "\"fmt\"", None),
            ("TypeScript", "import x from '<b>bold</b>';", None),
        ];
        for (language, line, expected) in cases {
            assert_eq!(
                imported_library(language, line).as_deref(),
                expected,
                "{language}: {line}"
            );
        }
    }

    #[test]
    fn first_party_comes_from_manifests_not_directory_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (path, text) in [
            (
                "crates/core/Cargo.toml",
                "[package]\nname = \"edge-ai-core\"\n[dependencies]\nname = \"x\"\n",
            ),
            ("apps/web/package.json", "{\"name\": \"@lakehub/web\"}\n"),
            ("backend/shared/backend_shared/__init__.py", ""),
            ("crates/core/src/domain.rs", "pub struct Id;\n"),
            ("crates/core/src/store/mod.rs", "pub mod sql;\n"),
            ("app/types/models.py", "X = 1\n"),
            ("go.mod", "module sallyport.app\n\ngo 1.23\n"),
            (".claude/skills/fastapi/SKILL.md", "not a package\n"),
        ] {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["-c", "user.name=A", "-c", "user.email=a@x"])
                .args(["-c", "commit.gpgsign=false", "-c", "core.hooksPath="])
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", root.join(".unused"))
                .status()
                .unwrap();
            assert!(status.success());
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        let listing = super::super::tooling::listing(root, "HEAD").unwrap();
        let first_party = FirstParty::from_snapshot(root, "HEAD", listing.as_deref()).unwrap();

        assert!(first_party.contains("Rust:edge_ai_core"));
        assert!(first_party.contains("TypeScript:@lakehub/web"));
        assert!(first_party.contains("Python:backend_shared"));
        assert!(first_party.contains("Rust:domain"), "a crate-local module");
        assert!(first_party.contains("Rust:store"));
        assert!(first_party.contains("Python:types"));
        assert!(first_party.contains("Python:models"));
        assert!(first_party.contains("Go:sallyport.app/internal/auth"));
        assert!(first_party.contains("Go:sallyport.app/api"));
        assert!(first_party.contains("Go:example.com/tool/internal/x"));
        assert!(
            !first_party.contains("Python:fastapi"),
            "a directory name is not a package"
        );
        assert!(!first_party.contains("Rust:tokio"));
        assert!(!first_party.contains("Go:github.com/stretchr/testify"));
        assert_eq!(
            imported_library("Python", "from __future__ import annotations"),
            None
        );
        assert_eq!(
            imported_library("TypeScript", "import assert from 'node:assert';"),
            None
        );
        assert_eq!(
            imported_library("JavaScript", "const fs = require('fs');"),
            None
        );
        assert_eq!(
            imported_library("TypeScript", "import { Button } from '@/components/ui';"),
            None
        );
    }

    #[test]
    fn associations_rank_what_co_occurs_with_the_changed_areas() {
        let commit = |keys: &[&str]| CommitEvidence {
            sha: String::new(),
            authored_at: "2026-09-01".to_string(),
            counts: keys.iter().map(|key| (key.to_string(), 1)).collect(),
        };
        let commits = [
            commit(&[
                "range.area.edge-ai/mcp",
                "range.lib.Rust:tokio",
                "range.lang.Rust",
            ]),
            commit(&["range.area.edge-ai/mcp", "range.lib.Rust:tokio"]),
            commit(&["range.area.edge-ai/web", "range.lib.TypeScript:react"]),
        ];
        let areas = BTreeSet::from(["edge-ai/mcp".to_string()]);
        assert_eq!(
            associations(&commits, &areas, 5),
            [("Rust:tokio".to_string(), 2)]
        );
        assert!(associations(&commits, &BTreeSet::new(), 5).is_empty());
    }

    #[test]
    fn range_counts_commits_and_recency_from_the_newest_commit() {
        let commit = |date: &str, keys: &[&str]| CommitEvidence {
            sha: date.to_string(),
            authored_at: date.to_string(),
            counts: keys.iter().map(|key| (key.to_string(), 1)).collect(),
        };
        let commits = [
            commit("2026-09-01", &["range.lang.Rust", "range.area.edge-ai/mcp"]),
            commit("2026-08-01", &["range.lang.Rust", "range.lib.Rust:tokio"]),
            commit(
                "2025-01-01",
                &["range.lang.Python", "range.area.edge-ai/mcp"],
            ),
        ];
        let rendered = render(&commits);
        assert!(
            rendered.contains("Rust (2; 2 recent; last 2026-09)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("Python (1; 0 recent; last 2025-01)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("edge-ai/mcp (2; 1 recent; last 2026-09)"),
            "{rendered}"
        );
        assert!(rendered.contains("Rust:tokio (1; 1 recent"), "{rendered}");
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(
            day_number("2026-03-01").unwrap() - day_number("2026-02-28").unwrap(),
            1
        );
        assert!(render(&[]).is_empty());
    }
}
