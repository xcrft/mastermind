//! Import observations from the complete committed source, limited to added rows.

use super::{imported_library, language};
use std::collections::BTreeSet;
use std::path::Path;
use tree_sitter::{Node, Parser};

pub(super) fn from_source(
    path: &str,
    source: &str,
    added_rows: &BTreeSet<usize>,
) -> Option<BTreeSet<String>> {
    if source.len() as u64 > crate::indexer::MAX_INDEXABLE_FILE_SIZE {
        return None;
    }
    let language_name = language(path)?;
    let extractor = crate::indexer::extractor_for_path(Path::new(path))?;
    let mut parser = Parser::new();
    parser.set_language(&extractor.language()).ok()?;
    let tree = parser.parse(source, None)?;
    if tree.root_node().has_error() {
        return None;
    }
    let mut libraries = BTreeSet::new();
    let mut nodes = vec![tree.root_node()];
    while let Some(node) = nodes.pop() {
        // A recovered parse cannot turn quoted or malformed input into an import.
        if node.is_error() || node.is_missing() {
            continue;
        }
        let intersects = added_rows
            .range(node.start_position().row..=node.end_position().row)
            .next()
            .is_some();
        if intersects && !node.has_error() {
            match (language_name, node.kind()) {
                ("Python", "import_statement") => {
                    let mut cursor = node.walk();
                    for child in node.named_children(&mut cursor) {
                        let name = if child.kind() == "aliased_import" {
                            child.child_by_field_name("name")
                        } else if child.kind() == "dotted_name" {
                            Some(child)
                        } else {
                            None
                        };
                        if let Some(name) =
                            name.and_then(|name| name.utf8_text(source.as_bytes()).ok())
                        {
                            insert(&mut libraries, "Python", &format!("import {name}"));
                        }
                    }
                }
                ("Python", "import_from_statement") => {
                    if let Some(module) = node
                        .child_by_field_name("module_name")
                        .and_then(|name| name.utf8_text(source.as_bytes()).ok())
                    {
                        insert(&mut libraries, "Python", &format!("from {module} import x"));
                    }
                }
                ("Rust", "use_declaration") => {
                    if let Some(argument) = node.child_by_field_name("argument") {
                        rust_roots(argument, source, &mut libraries);
                    }
                }
                ("TypeScript" | "JavaScript", "import_statement" | "export_statement") => {
                    if let Some(specifier) = node.child_by_field_name("source") {
                        script_specifier(specifier, source, language_name, &mut libraries);
                    }
                }
                ("TypeScript" | "JavaScript", "call_expression") => {
                    if node
                        .child_by_field_name("function")
                        .is_some_and(|function| {
                            function.kind() == "identifier"
                                && function.utf8_text(source.as_bytes()) == Ok("require")
                        })
                    {
                        if let Some(specifier) = node
                            .child_by_field_name("arguments")
                            .and_then(|arguments| arguments.named_child(0))
                        {
                            script_specifier(specifier, source, language_name, &mut libraries);
                        }
                    }
                }
                ("Go", "import_spec") => {
                    if let Some(specifier) = node
                        .child_by_field_name("path")
                        .and_then(|path| path.utf8_text(source.as_bytes()).ok())
                    {
                        let specifier = specifier.trim_matches(['"', '`']);
                        insert(&mut libraries, "Go", &format!("\"{specifier}\""));
                    }
                }
                _ => {}
            }
        }
        let mut cursor = node.walk();
        nodes.extend(node.named_children(&mut cursor));
    }
    Some(libraries)
}

fn insert(libraries: &mut BTreeSet<String>, language: &str, statement: &str) {
    if let Some(library) = imported_library(language, statement) {
        libraries.insert(library);
    }
}

fn script_specifier(
    node: Node<'_>,
    source: &str,
    language: &str,
    libraries: &mut BTreeSet<String>,
) {
    if node.kind() == "string" {
        if let Ok(specifier) = node.utf8_text(source.as_bytes()) {
            insert(libraries, language, &format!("import x from {specifier}"));
        }
    }
}

fn rust_roots(node: Node<'_>, source: &str, libraries: &mut BTreeSet<String>) {
    match node.kind() {
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                rust_roots(child, source, libraries);
            }
        }
        "scoped_use_list" | "use_as_clause" => {
            if let Some(path) = node.child_by_field_name("path") {
                rust_roots(path, source, libraries);
            }
        }
        "use_wildcard" => {
            if let Some(path) = node.named_child(0) {
                rust_roots(path, source, libraries);
            }
        }
        "identifier" | "scoped_identifier" => {
            if let Ok(path) = node.utf8_text(source.as_bytes()) {
                insert(libraries, "Rust", &format!("use {path};"));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(path: &str, source: &str) -> BTreeSet<String> {
        from_source(path, source, &(0..source.lines().count()).collect()).unwrap()
    }

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn python_imports_exclude_comments_strings_relative_and_standard_modules() {
        let source = "\"\"\"\nfrom the example import nothing\nimport imagined\n\"\"\"\n# import phantom\nimport numpy as np, pytest\nfrom pandas.core import DataFrame\nfrom .local import value\nimport tomllib, ssl, types, zlib, lzma, contextvars, binascii\n";
        assert_eq!(
            all("sample.py", source),
            names(&["Python:numpy", "Python:pytest", "Python:pandas"])
        );
        assert!(from_source("sample.py", source, &BTreeSet::from([1, 2, 4]))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn script_imports_require_added_syntax_and_support_multiline_statements() {
        let source = "/*\n * unknown from \"confirmed zero\"\n * import x from 'imagined';\n */\nimport {\n  parse\n} from 'zod';\nexport { value } from '@scope/pkg/subpath';\nconst fs = require('fs-extra');\nconst literal = \"require('phantom')\";\nimport 'node:test';\nimport 'worker_threads';\nimport './local';\n";
        assert_eq!(
            all("sample.ts", source),
            names(&[
                "TypeScript:zod",
                "TypeScript:@scope/pkg",
                "TypeScript:fs-extra"
            ])
        );
        assert_eq!(
            from_source("sample.ts", source, &BTreeSet::from([5])).unwrap(),
            names(&["TypeScript:zod"])
        );
        assert!(from_source("sample.ts", source, &BTreeSet::from([1, 2, 9]))
            .unwrap()
            .is_empty());
        assert_eq!(
            all(
                "sample.js",
                "import 'react';\nmodule.exports = require('lodash/fp');\n"
            ),
            names(&["JavaScript:react", "JavaScript:lodash"])
        );
    }

    #[test]
    fn rust_and_go_import_groups_aliases_and_strings_are_distinguished() {
        let rust = "//! use phantom::Value;\nuse {serde::Serialize, tokio::{self, task}};\nuse regex as re;\nuse tracing::*;\nuse std::collections::HashMap;\nuse crate::local;\nfn example() { let _ = \"use imaginary::Thing;\"; }\n";
        assert_eq!(
            all("sample.rs", rust),
            names(&["Rust:serde", "Rust:tokio", "Rust:regex", "Rust:tracing"])
        );
        let go = "package main\nimport (\n  \"fmt\"\n  alias \"github.com/acme/lib/subpackage\"\n  . `golang.org/x/tools`\n)\n// import \"imagined.example/lib\"\n";
        assert_eq!(
            all("sample.go", go),
            names(&["Go:github.com/acme/lib", "Go:golang.org/x/tools"])
        );
    }

    #[test]
    fn unsupported_oversized_and_malformed_source_is_unknown() {
        assert!(from_source("sample.txt", "import numpy", &BTreeSet::from([0])).is_none());
        assert!(from_source("sample.py", "import (", &BTreeSet::from([0])).is_none());
        let large = " ".repeat(crate::indexer::MAX_INDEXABLE_FILE_SIZE as usize + 1);
        assert!(from_source("sample.py", &large, &BTreeSet::from([0])).is_none());
    }
}
