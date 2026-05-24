//! Python extractor — functions, classes, methods, calls, imports.

use super::common::{line_of, node_text, push_call_with_type, push_def, push_import};
use super::LanguageExtractor;
use crate::store::PendingFile;
use tree_sitter::{Node, Tree};

pub struct PythonExtractor;

impl LanguageExtractor for PythonExtractor {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_python::language()
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
                let kind = match parent_index {
                    Some(idx) if pending.symbols[idx].kind == "class" => "method",
                    _ => "function",
                };
                let name = child
                    .child_by_field_name("name")
                    .and_then(|n| node_text(&n, source))
                    .unwrap_or("<anon>")
                    .to_string();
                let signature = extract_signature(&child, source);
                let idx = push_def(pending, name, kind, &child, signature, parent_index);
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "class_definition" => {
                let name = child
                    .child_by_field_name("name")
                    .and_then(|n| node_text(&n, source))
                    .unwrap_or("<anon>")
                    .to_string();
                let signature = extract_signature(&child, source);
                let idx = push_def(pending, name, "class", &child, signature, parent_index);
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
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
