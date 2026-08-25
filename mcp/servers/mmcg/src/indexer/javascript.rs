//! JavaScript / JSX extractor — shares the walker with TypeScript: the grammars
//! use the same node kinds for what we care about (functions, classes, methods,
//! calls, imports). TS-only node kinds don't appear in JS files.

use super::typescript;
use super::{LanguageExtractor, RawConceptDocumentation};
use crate::store::PendingFile;
use tree_sitter::Tree;

pub struct JavascriptExtractor;

impl LanguageExtractor for JavascriptExtractor {
    fn language(&self) -> tree_sitter::Language {
        // tree-sitter 0.25+ exposes grammar as `LANGUAGE` (LanguageFn const) — .into() converts.
        tree_sitter_javascript::LANGUAGE.into()
    }

    fn name(&self) -> &'static str {
        "javascript"
    }

    fn extract(&self, tree: &Tree, source: &[u8], pending: &mut PendingFile, module_index: usize) {
        typescript::walk(
            tree.root_node(),
            source,
            pending,
            Some(module_index),
            module_index,
        );
    }
}

pub(super) fn collect_concept_documentation(
    tree: &Tree,
    source: &[u8],
    pending: &PendingFile,
) -> Vec<RawConceptDocumentation> {
    typescript::collect_concept_documentation(tree, source, pending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::common;
    use crate::indexer::parse_one;

    #[test]
    fn extracts_function_and_class() {
        let path = common::write_tmp(
            "js",
            "lib.js",
            "function hello(x) { return String(x); }\n\
             class Foo {\n  bar() { this.baz(); }\n  baz() {}\n}\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &JavascriptExtractor).unwrap();
        let names: Vec<&str> = pending.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello"));
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"bar"));
        assert!(names.contains(&"baz"));
    }

    #[test]
    fn extracts_imports_with_paths() {
        let path = common::write_tmp(
            "js",
            "imp.js",
            "import foo from 'pkg-a';\n\
             import { bar, baz as qux } from './local';\n\
             import * as ns from 'pkg-b';\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &JavascriptExtractor).unwrap();
        let imports: Vec<(&str, Option<&str>)> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "imports")
            .map(|e| (e.to_name.as_str(), e.to_path.as_deref()))
            .collect();

        assert!(imports.contains(&("foo", Some("pkg-a::default"))));
        assert!(imports.contains(&("bar", Some("./local::bar"))));
        assert!(imports.contains(&("qux", Some("./local::baz"))));
        assert!(imports.contains(&("ns", Some("pkg-b::*"))));
    }

    #[test]
    fn extracts_calls() {
        let path = common::write_tmp(
            "js",
            "calls.js",
            "function main() {\n  console.log('hi');\n  const x = new Foo();\n}\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &JavascriptExtractor).unwrap();
        let calls: Vec<(&str, Option<&str>)> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| (e.to_name.as_str(), e.to_path.as_deref()))
            .collect();
        assert!(calls.contains(&("log", Some("console.log"))));
        assert!(calls.contains(&("Foo", Some("Foo"))));
    }
}
