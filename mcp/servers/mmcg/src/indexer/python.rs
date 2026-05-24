//! Python extractor — functions, classes, methods, calls, imports.

use super::common::{
    line_of, node_text, push_call_with_type, push_def, push_def_with_decorators, push_import,
};
use super::LanguageExtractor;
use crate::store::PendingFile;
use tree_sitter::{Node, Tree};

pub struct PythonExtractor;

impl LanguageExtractor for PythonExtractor {
    fn language(&self) -> tree_sitter::Language {
        // tree-sitter 0.23+ exposes grammars as `LANGUAGE` (LanguageFn const) — convert via .into().
        tree_sitter_python::LANGUAGE.into()
    }

    fn name(&self) -> &'static str {
        "python"
    }

    fn extract(&self, tree: &Tree, source: &[u8], pending: &mut PendingFile, module_index: usize) {
        walk(
            tree.root_node(),
            source,
            pending,
            Some(module_index),
            module_index,
        );
    }
}

fn walk(
    node: Node,
    source: &[u8],
    pending: &mut PendingFile,
    parent_index: Option<usize>,
    module_index: usize,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" | "async_function_definition" => {
                handle_def(&child, source, pending, parent_index, module_index, None);
            }
            "class_definition" => {
                handle_def(&child, source, pending, parent_index, module_index, None);
            }
            "decorated_definition" => {
                // Collect decorator names AND walk decorator subtrees to preserve
                // call edges from decorator arguments (e.g. `@app.route("/api")`
                // is a call to `app.route` from module scope). Then find the
                // inner def and handle with decorators attached.
                let mut names: Vec<String> = Vec::new();
                let mut inner: Option<Node> = None;
                let mut dc = child.walk();
                for sub in child.children(&mut dc) {
                    match sub.kind() {
                        "decorator" => {
                            if let Some(n) = extract_decorator_name(&sub, source) {
                                names.push(n);
                            }
                            // Walk decorator subtree — captures the call edges
                            // for `@app.route("/api")` etc. at module scope.
                            walk(sub, source, pending, parent_index, module_index);
                        }
                        "function_definition"
                        | "async_function_definition"
                        | "class_definition" => {
                            inner = Some(sub);
                        }
                        _ => {}
                    }
                }
                if let Some(def_node) = inner {
                    let decorators = if names.is_empty() {
                        None
                    } else {
                        Some(format!(",{},", names.join(",")))
                    };
                    handle_def(
                        &def_node,
                        source,
                        pending,
                        parent_index,
                        module_index,
                        decorators,
                    );
                }
            }
            "call" => {
                if let Some((name, path, to_type)) = call_target(&child, source) {
                    push_call_with_type(
                        pending,
                        parent_index.unwrap_or(module_index),
                        name,
                        path,
                        to_type,
                        line_of(&child),
                    );
                }
                walk(child, source, pending, parent_index, module_index);
            }
            "import_statement" => {
                handle_import(&child, source, pending, module_index);
            }
            "import_from_statement" => {
                handle_import_from(&child, source, pending, module_index);
            }
            _ => walk(child, source, pending, parent_index, module_index),
        }
    }
}

/// Push a def (function/method/class) and recurse into its body. Shared between
/// the decorated and undecorated paths.
fn handle_def(
    def_node: &Node,
    source: &[u8],
    pending: &mut PendingFile,
    parent_index: Option<usize>,
    module_index: usize,
    decorators: Option<String>,
) {
    let kind = if def_node.kind() == "class_definition" {
        "class"
    } else if matches!(parent_index, Some(idx) if pending.symbols[idx].kind == "class") {
        "method"
    } else {
        "function"
    };
    let name = def_node
        .child_by_field_name("name")
        .and_then(|n| node_text(&n, source))
        .unwrap_or("<anon>")
        .to_string();
    let signature = extract_signature(def_node, source);
    let idx = if decorators.is_some() {
        push_def_with_decorators(
            pending,
            name,
            kind,
            def_node,
            signature,
            parent_index,
            decorators,
        )
    } else {
        push_def(pending, name, kind, def_node, signature, parent_index)
    };
    if let Some(body) = def_node.child_by_field_name("body") {
        walk(body, source, pending, Some(idx), module_index);
    }
}

/// Extract the dotted name of a Python decorator. Returns the function name only
/// (not arguments). Handles:
///   `@property`              → "property"
///   `@pytest.fixture`        → "pytest.fixture"
///   `@app.route("/api")`     → "app.route"
///   `@router.get("/x")`      → "router.get"
fn extract_decorator_name(decorator_node: &Node, source: &[u8]) -> Option<String> {
    let mut cursor = decorator_node.walk();
    for child in decorator_node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "attribute" => return node_text(&child, source).map(String::from),
            "call" => {
                let func = child.child_by_field_name("function")?;
                return node_text(&func, source).map(String::from);
            }
            _ => continue,
        }
    }
    None
}

/// Returns (leaf, full_path, to_type).
/// `foo()` → ("foo", "foo", None)
/// `obj.foo()` → ("foo", "obj.foo", None) — lowercase receiver, not a class
/// `Class.foo()` → ("foo", "Class.foo", Some("Class")) — uppercase receiver, treat as class
/// `pkg.Cls.foo()` → ("foo", "pkg.Cls.foo", Some("Cls")) — find rightmost-capitalized receiver
fn call_target(
    call_node: &Node,
    source: &[u8],
) -> Option<(String, Option<String>, Option<String>)> {
    let fn_node = call_node.child_by_field_name("function")?;
    let full = node_text(&fn_node, source).map(String::from);
    match fn_node.kind() {
        "identifier" => {
            let n = node_text(&fn_node, source)?.to_string();
            Some((n, full, None))
        }
        "attribute" => {
            let leaf = fn_node
                .child_by_field_name("attribute")
                .and_then(|n| node_text(&n, source))?
                .to_string();
            let to_type = type_prefix_from_object(&fn_node, source);
            Some((leaf, full, to_type))
        }
        _ => None,
    }
}

/// For Python `attribute` node `obj.foo`, find the receiver. If the immediate
/// receiver is uppercase-starting, treat as a class. For nested `pkg.Cls.foo`,
/// the receiver is `pkg.Cls` (another attribute node) — recurse to find the
/// rightmost identifier and check its case.
fn type_prefix_from_object(attribute: &Node, source: &[u8]) -> Option<String> {
    let obj = attribute.child_by_field_name("object")?;
    let candidate = match obj.kind() {
        "identifier" => node_text(&obj, source).map(String::from),
        "attribute" => obj
            .child_by_field_name("attribute")
            .and_then(|n| node_text(&n, source))
            .map(String::from),
        _ => None,
    }?;
    if candidate
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        Some(candidate)
    } else {
        None
    }
}

/// `import foo.bar` → name=bar, path=foo.bar
/// `import foo.bar as baz` → name=baz, path=foo.bar
fn handle_import(node: &Node, source: &[u8], pending: &mut PendingFile, module_index: usize) {
    let line = line_of(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" => {
                let path = node_text(&child, source).map(String::from);
                let name = rightmost_identifier(&child, source).unwrap_or_default();
                if !name.is_empty() {
                    push_import(pending, module_index, name, path, line);
                }
            }
            "aliased_import" => {
                let alias = child
                    .child_by_field_name("alias")
                    .and_then(|n| node_text(&n, source))
                    .map(String::from);
                let orig_path = child
                    .child_by_field_name("name")
                    .and_then(|n| node_text(&n, source))
                    .map(String::from);
                if let Some(a) = alias {
                    push_import(pending, module_index, a, orig_path, line);
                }
            }
            _ => {}
        }
    }
}

/// `from foo.bar import baz` → name=baz, path=foo.bar.baz
/// `from foo.bar import baz as qux` → name=qux, path=foo.bar.baz
fn handle_import_from(node: &Node, source: &[u8], pending: &mut PendingFile, module_index: usize) {
    let line = line_of(node);

    // The module name appears in field "module_name" in tree-sitter-python.
    let module = node
        .child_by_field_name("module_name")
        .and_then(|n| node_text(&n, source))
        .map(String::from);

    // Walk children; the "name" field appears multiple times for each imported item.
    let mut cursor = node.walk();
    for (i, child) in node.children(&mut cursor).enumerate() {
        let field = node.field_name_for_child(i as u32);
        if field != Some("name") {
            continue;
        }
        match child.kind() {
            "identifier" => {
                let n = node_text(&child, source).unwrap_or("").to_string();
                let path = compose_dotted_path(module.as_deref(), Some(n.as_str()));
                if !n.is_empty() {
                    push_import(pending, module_index, n, path, line);
                }
            }
            "dotted_name" => {
                let leaf = rightmost_identifier(&child, source).unwrap_or_default();
                let leaf_text = node_text(&child, source).map(String::from);
                let path = match (&module, &leaf_text) {
                    (Some(m), Some(l)) => Some(format!("{m}.{l}")),
                    (Some(m), None) => Some(m.clone()),
                    (None, Some(l)) => Some(l.clone()),
                    _ => None,
                };
                if !leaf.is_empty() {
                    push_import(pending, module_index, leaf, path, line);
                }
            }
            "aliased_import" => {
                let alias = child
                    .child_by_field_name("alias")
                    .and_then(|n| node_text(&n, source))
                    .map(String::from);
                let orig = child
                    .child_by_field_name("name")
                    .and_then(|n| node_text(&n, source));
                if let Some(a) = alias {
                    let path = compose_dotted_path(module.as_deref(), orig);
                    push_import(pending, module_index, a, path, line);
                }
            }
            _ => {}
        }
    }
}

fn compose_dotted_path(module: Option<&str>, leaf: Option<&str>) -> Option<String> {
    match (module, leaf) {
        (Some(m), Some(l)) => Some(format!("{m}.{l}")),
        (Some(m), None) => Some(m.to_string()),
        (None, Some(l)) => Some(l.to_string()),
        (None, None) => None,
    }
}

fn rightmost_identifier(node: &Node, source: &[u8]) -> Option<String> {
    let mut last: Option<String> = None;
    let mut c = node.walk();
    for ch in node.children(&mut c) {
        if ch.kind() == "identifier" {
            last = node_text(&ch, source).map(String::from);
        }
    }
    last
}

fn extract_signature(node: &Node, source: &[u8]) -> Option<String> {
    let body = node.child_by_field_name("body")?;
    let header_end = body.start_byte();
    let start = node.start_byte();
    if header_end <= start {
        return None;
    }
    let text = std::str::from_utf8(&source[start..header_end]).ok()?;
    let trimmed = text
        .trim_end_matches(|c: char| c == ':' || c.is_whitespace())
        .to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parse_one;
    use std::env;
    use std::path::PathBuf;

    fn write_tmp(name: &str, content: &str) -> PathBuf {
        let mut dir = env::temp_dir();
        dir.push(format!("mmcg-py-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn extracts_function_with_module_symbol() {
        let path = write_tmp("f1.py", "def hello(x: int) -> str:\n    return str(x)\n");
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &PythonExtractor).unwrap();
        assert_eq!(pending.symbols[0].kind, "module");
        assert_eq!(pending.symbols[1].name, "hello");
        assert_eq!(pending.symbols[1].kind, "function");
    }

    #[test]
    fn extracts_class_with_methods() {
        let path = write_tmp(
            "f2.py",
            "class Foo:\n    def bar(self):\n        self.baz()\n    def baz(self):\n        pass\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &PythonExtractor).unwrap();
        let kinds: Vec<&str> = pending.symbols.iter().map(|s| s.kind.as_str()).collect();
        assert!(kinds.contains(&"class"));
        assert_eq!(kinds.iter().filter(|k| **k == "method").count(), 2);
    }

    #[test]
    fn imports_capture_fully_qualified_path() {
        let path = write_tmp(
            "f3.py",
            "import os\n\
             import pathlib as pl\n\
             from collections import defaultdict\n\
             from collections.abc import Iterable as Iter\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &PythonExtractor).unwrap();

        let imports: Vec<(&str, Option<&str>)> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "imports")
            .map(|e| (e.to_name.as_str(), e.to_path.as_deref()))
            .collect();

        // import os → ("os", "os")
        assert!(imports.contains(&("os", Some("os"))));
        // import pathlib as pl → ("pl", "pathlib")
        assert!(imports.contains(&("pl", Some("pathlib"))));
        // from collections import defaultdict → ("defaultdict", "collections.defaultdict")
        assert!(imports.contains(&("defaultdict", Some("collections.defaultdict"))));
        // from collections.abc import Iterable as Iter → ("Iter", "collections.abc.Iterable")
        assert!(imports.contains(&("Iter", Some("collections.abc.Iterable"))));
    }

    #[test]
    fn extracts_decorators_into_symbol_field() {
        let path = write_tmp(
            "f5.py",
            "import pytest\n\
             from fastapi import APIRouter\n\
             router = APIRouter()\n\
             \n\
             @pytest.fixture\n\
             def db():\n\
                 return None\n\
             \n\
             @router.get('/api/x')\n\
             async def get_x():\n\
                 return {}\n\
             \n\
             @property\n\
             def simple(self):\n\
                 return 1\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &PythonExtractor).unwrap();

        // db should have decorators=,pytest.fixture,
        let db = pending
            .symbols
            .iter()
            .find(|s| s.name == "db")
            .expect("db symbol");
        assert_eq!(db.decorators.as_deref(), Some(",pytest.fixture,"));

        // get_x should have decorators=,router.get,
        let get_x = pending
            .symbols
            .iter()
            .find(|s| s.name == "get_x")
            .expect("get_x symbol");
        assert_eq!(get_x.decorators.as_deref(), Some(",router.get,"));

        // simple should have decorators=,property,
        let simple = pending
            .symbols
            .iter()
            .find(|s| s.name == "simple")
            .expect("simple symbol");
        assert_eq!(simple.decorators.as_deref(), Some(",property,"));
    }

    #[test]
    fn calls_capture_full_attribute_path() {
        let path = write_tmp(
            "f4.py",
            "def main():\n    logger.info('hi')\n    pkg.mod.helper()\n    print('there')\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &PythonExtractor).unwrap();
        let calls: Vec<(&str, Option<&str>)> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| (e.to_name.as_str(), e.to_path.as_deref()))
            .collect();
        assert!(calls.contains(&("info", Some("logger.info"))));
        assert!(calls.contains(&("helper", Some("pkg.mod.helper"))));
        assert!(calls.contains(&("print", Some("print"))));
    }
}
