//! Rust extractor — functions, structs, enums, traits, impls (with methods),
//! calls, macro invocations, and use declarations.

use super::common::{
    line_of, node_text, push_call, push_call_with_type, push_def, push_def_with_decorators,
    push_import,
};
use super::LanguageExtractor;
use crate::store::PendingFile;
use tree_sitter::{Node, Tree};

pub struct RustExtractor;

impl LanguageExtractor for RustExtractor {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn name(&self) -> &'static str {
        "rust"
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
    // Attributes are preceding siblings of the item they decorate. Accumulate;
    // on the next def-item attach and clear; on any other node clear (stray
    // attributes don't carry to non-adjacent items).
    let mut pending_attrs: Vec<String> = Vec::new();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "attribute_item" | "inner_attribute_item" => {
                if let Some(n) = extract_attribute_name(&child, source) {
                    pending_attrs.push(n);
                }
                continue;
            }
            "function_item" => {
                let kind = match parent_index {
                    Some(p) if matches!(pending.symbols[p].kind.as_str(), "impl" | "trait") => {
                        "method"
                    }
                    _ => "function",
                };
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_for_function(&child, source);
                let attrs = take_attrs(&mut pending_attrs);
                let idx = push_def_or_decorated(
                    pending,
                    name,
                    kind,
                    &child,
                    signature,
                    parent_index,
                    attrs,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "struct_item" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let sig = signature_until_body_or_semi(&child, source);
                let attrs = take_attrs(&mut pending_attrs);
                push_def_or_decorated(pending, name, "struct", &child, sig, parent_index, attrs);
            }
            "enum_item" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let sig = signature_until_body_or_semi(&child, source);
                let attrs = take_attrs(&mut pending_attrs);
                push_def_or_decorated(pending, name, "enum", &child, sig, parent_index, attrs);
            }
            "trait_item" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let sig = signature_until_body_or_semi(&child, source);
                let attrs = take_attrs(&mut pending_attrs);
                let idx =
                    push_def_or_decorated(pending, name, "trait", &child, sig, parent_index, attrs);
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "impl_item" => {
                // The impl block becomes a symbol named after its target type;
                // methods inside parent to this impl symbol.
                let target_name =
                    impl_target_name(&child, source).unwrap_or_else(|| "<impl>".to_string());
                let sig = signature_until_body_or_semi(&child, source);
                let attrs = take_attrs(&mut pending_attrs);
                let idx = push_def_or_decorated(
                    pending,
                    target_name,
                    "impl",
                    &child,
                    sig,
                    parent_index,
                    attrs,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "mod_item" => {
                // `mod foo { ... }` — container symbol.
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let sig = signature_until_body_or_semi(&child, source);
                let attrs = take_attrs(&mut pending_attrs);
                let idx =
                    push_def_or_decorated(pending, name, "mod", &child, sig, parent_index, attrs);
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "call_expression" => {
                pending_attrs.clear();
                if let Some((name, path, type_prefix)) = call_target_with_type(&child, source) {
                    push_call_with_type(
                        pending,
                        parent_index.unwrap_or(module_index),
                        name,
                        path,
                        type_prefix,
                        line_of(&child),
                    );
                }
                walk(child, source, pending, parent_index, module_index);
            }
            "macro_invocation" => {
                if let Some(macro_node) = child.child_by_field_name("macro") {
                    if let Some(name) = rightmost_identifier(&macro_node, source) {
                        let full = node_text(&macro_node, source).map(|t| format!("{t}!"));
                        push_call(
                            pending,
                            parent_index.unwrap_or(module_index),
                            name,
                            full,
                            line_of(&child),
                        );
                    }
                }
                walk(child, source, pending, parent_index, module_index);
            }
            "use_declaration" => {
                if let Some(arg) = child.child_by_field_name("argument") {
                    collect_use_names(&arg, source, pending, module_index, line_of(&child), None);
                }
            }
            _ => walk(child, source, pending, parent_index, module_index),
        }
    }
}

/// Take accumulated attributes, convert to comma-delimited decorator format.
fn take_attrs(attrs: &mut Vec<String>) -> Option<String> {
    if attrs.is_empty() {
        None
    } else {
        let formatted = format!(",{},", attrs.join(","));
        attrs.clear();
        Some(formatted)
    }
}

/// Picks `push_def` or `push_def_with_decorators` based on attrs presence.
fn push_def_or_decorated(
    pending: &mut PendingFile,
    name: String,
    kind: &str,
    node: &Node,
    signature: Option<String>,
    parent_index: Option<usize>,
    decorators: Option<String>,
) -> usize {
    if decorators.is_some() {
        push_def_with_decorators(
            pending,
            name,
            kind,
            node,
            signature,
            parent_index,
            decorators,
        )
    } else {
        push_def(pending, name, kind, node, signature, parent_index)
    }
}

/// Attribute name from `#[name]`, `#[name::sub]`, or `#[name(args)]` — the path
/// part before any `(`, e.g. "test", "tokio::main", "derive", "cfg".
fn extract_attribute_name(attr_item: &Node, source: &[u8]) -> Option<String> {
    let text = node_text(attr_item, source)?;
    // e.g. "#[test]", "#[tokio::main]", "#[derive(Debug)]", "#[cfg(test)]".
    let inner = text
        .trim_start_matches('#')
        .trim_start_matches("![")
        .trim_start_matches('[');
    let inner = inner.trim_end_matches(']');
    // Cut at first '(', whitespace, or '='.
    let cut = inner
        .find(['(', ' ', '\t', '=', '\n'])
        .unwrap_or(inner.len());
    let name = inner[..cut].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn name_field<'a>(node: &Node, source: &'a [u8]) -> Option<&'a str> {
    node.child_by_field_name("name")
        .and_then(|n| node_text(&n, source))
}

/// `impl Foo { ... }` → "Foo". `impl Trait for Foo { ... }` → "Foo" (the type,
/// not the trait).
fn impl_target_name(impl_node: &Node, source: &[u8]) -> Option<String> {
    let type_node = impl_node.child_by_field_name("type")?;
    rightmost_identifier(&type_node, source)
}

/// Strip path segments and generics — `foo::Bar::Baz<T>` → "Baz".
fn rightmost_identifier(node: &Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" | "type_identifier" => node_text(node, source).map(String::from),
        "scoped_identifier" | "scoped_type_identifier" => node
            .child_by_field_name("name")
            .and_then(|n| node_text(&n, source))
            .map(String::from),
        "generic_type" => node
            .child_by_field_name("type")
            .and_then(|n| rightmost_identifier(&n, source)),
        "field_expression" => node
            .child_by_field_name("field")
            .and_then(|n| node_text(&n, source))
            .map(String::from),
        _ => {
            // Last-ditch: walk children, return the rightmost identifier's text.
            let mut last: Option<String> = None;
            let mut c = node.walk();
            for ch in node.children(&mut c) {
                if let Some(name) = rightmost_identifier(&ch, source) {
                    last = Some(name);
                }
            }
            last
        }
    }
}

/// Returns (leaf_name, full_path, type_prefix).
/// - `SessionStore::new()` → ("new", Some("SessionStore::new"), Some("SessionStore"))
/// - `foo::bar::Baz::new()` → ("new", Some("foo::bar::Baz::new"), Some("Baz"))
/// - `obj.method()` (field_expression) → ("method", Some("obj.method"), None)  — value receiver, not a type
/// - `foo()` → ("foo", Some("foo"), None)
fn call_target_with_type(
    call_node: &Node,
    source: &[u8],
) -> Option<(String, Option<String>, Option<String>)> {
    let fn_node = call_node.child_by_field_name("function")?;
    let leaf = rightmost_identifier(&fn_node, source)?;
    let path = node_text(&fn_node, source).map(String::from);
    // Type prefix only for scoped_identifier (Type::method). field_expression
    // (obj.method) has a value receiver, not a type — skip.
    let type_prefix = if fn_node.kind() == "scoped_identifier" {
        fn_node
            .child_by_field_name("path")
            .and_then(|p| rightmost_identifier(&p, source))
    } else {
        None
    };
    Some((leaf, path, type_prefix))
}

/// Walk a use-tree (argument of use_declaration), emitting an import edge per
/// imported-into-scope leaf. `prefix` is the path so far when recursing into a
/// `scoped_use_list` (e.g. `foo::{bar, baz}` → prefix is `foo`).
fn collect_use_names(
    node: &Node,
    source: &[u8],
    pending: &mut PendingFile,
    module_index: usize,
    line: u32,
    prefix: Option<&str>,
) {
    match node.kind() {
        "identifier" | "type_identifier" => {
            if let Some(name) = node_text(node, source) {
                let path = compose_scoped(prefix, name);
                push_import(pending, module_index, name.to_string(), Some(path), line);
            }
        }
        "scoped_identifier" => {
            // use foo::bar — imports "bar", path is the full scoped expression.
            let full = node_text(node, source).map(String::from);
            let leaf_path = full.as_deref().unwrap_or("");
            let combined = if let Some(p) = prefix {
                format!("{p}::{leaf_path}")
            } else {
                leaf_path.to_string()
            };
            if let Some(n) = node.child_by_field_name("name") {
                if let Some(name) = node_text(&n, source) {
                    push_import(
                        pending,
                        module_index,
                        name.to_string(),
                        Some(combined),
                        line,
                    );
                }
            }
        }
        "use_as_clause" => {
            // use foo::bar as baz — name="baz", path="foo::bar" (prefix prepended).
            let alias = node
                .child_by_field_name("alias")
                .and_then(|a| node_text(&a, source));
            let inner_path = node
                .child_by_field_name("path")
                .and_then(|p| node_text(&p, source))
                .map(String::from);

            if let Some(name) = alias {
                let path = match (prefix, inner_path) {
                    (Some(p), Some(i)) => Some(format!("{p}::{i}")),
                    (Some(p), None) => Some(p.to_string()),
                    (None, Some(i)) => Some(i),
                    (None, None) => None,
                };
                push_import(pending, module_index, name.to_string(), path, line);
            }
        }
        "use_list" | "scoped_use_list" => {
            // { a, b, c::d } — recurse into each entry. For scoped_use_list the
            // path child is the prefix, list holds entries.
            let new_prefix = if node.kind() == "scoped_use_list" {
                let local = node
                    .child_by_field_name("path")
                    .and_then(|p| node_text(&p, source));
                match (prefix, local) {
                    (Some(p), Some(l)) => Some(format!("{p}::{l}")),
                    (Some(p), None) => Some(p.to_string()),
                    (None, Some(l)) => Some(l.to_string()),
                    (None, None) => None,
                }
            } else {
                prefix.map(String::from)
            };

            let entries = if node.kind() == "scoped_use_list" {
                node.child_by_field_name("list")
            } else {
                Some(*node)
            };
            if let Some(list) = entries {
                let mut c = list.walk();
                for ch in list.children(&mut c) {
                    collect_use_names(
                        &ch,
                        source,
                        pending,
                        module_index,
                        line,
                        new_prefix.as_deref(),
                    );
                }
            }
        }
        "use_wildcard" => {
            let path = prefix
                .map(|p| format!("{p}::*"))
                .or_else(|| Some("*".to_string()));
            push_import(pending, module_index, "*".to_string(), path, line);
        }
        _ => {
            // Permissive: try the last identifier we can find.
            if let Some(name) = rightmost_identifier(node, source) {
                let path = compose_scoped(prefix, &name);
                push_import(pending, module_index, name, Some(path), line);
            }
        }
    }
}

fn compose_scoped(prefix: Option<&str>, leaf: &str) -> String {
    match prefix {
        Some(p) => format!("{p}::{leaf}"),
        None => leaf.to_string(),
    }
}

fn signature_for_function(node: &Node, source: &[u8]) -> Option<String> {
    // fn item: stop before the body { ... }.
    signature_until_body_or_semi(node, source)
}

fn signature_until_body_or_semi(node: &Node, source: &[u8]) -> Option<String> {
    // Take everything before the body or end of declaration.
    let cut = node
        .child_by_field_name("body")
        .map(|n| n.start_byte())
        .unwrap_or_else(|| node.end_byte());
    let start = node.start_byte();
    if cut <= start {
        return None;
    }
    let text = std::str::from_utf8(&source[start..cut]).ok()?;
    let trimmed = text
        .trim_end_matches(['{', ';', ' ', '\t', '\n', '\r'])
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
        dir.push(format!("mmcg-rs-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn extracts_function_and_struct() {
        let path = write_tmp(
            "lib.rs",
            "pub fn hello(x: i32) -> String { x.to_string() }\n\
             pub struct Foo { pub bar: i32 }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &RustExtractor).unwrap();
        let names: Vec<&str> = pending.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello"));
        assert!(names.contains(&"Foo"));
    }

    #[test]
    fn extracts_impl_methods() {
        let path = write_tmp(
            "impl.rs",
            "struct Foo;\n\
             impl Foo {\n\
                 fn new() -> Self { Foo }\n\
                 fn bar(&self) { self.baz(); }\n\
                 fn baz(&self) {}\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &RustExtractor).unwrap();
        let methods: Vec<&str> = pending
            .symbols
            .iter()
            .filter(|s| s.kind == "method")
            .map(|s| s.name.as_str())
            .collect();
        assert!(methods.contains(&"new"));
        assert!(methods.contains(&"bar"));
        assert!(methods.contains(&"baz"));

        let impl_sym = pending.symbols.iter().find(|s| s.kind == "impl").unwrap();
        assert_eq!(impl_sym.name, "Foo");
    }

    #[test]
    fn extracts_macro_invocation_as_call() {
        let path = write_tmp("m.rs", "fn main() { println!(\"hi\"); vec![1, 2, 3]; }\n");
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &RustExtractor).unwrap();
        let calls: Vec<&str> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| e.to_name.as_str())
            .collect();
        assert!(calls.contains(&"println"));
        assert!(calls.contains(&"vec"));
    }

    #[test]
    fn extracts_use_declarations() {
        let path = write_tmp(
            "u.rs",
            "use std::path::PathBuf;\n\
             use std::collections::{HashMap, BTreeMap};\n\
             use serde::Serialize as Ser;\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &RustExtractor).unwrap();
        let imports: Vec<&str> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "imports")
            .map(|e| e.to_name.as_str())
            .collect();
        assert!(imports.contains(&"PathBuf"));
        assert!(imports.contains(&"HashMap"));
        assert!(imports.contains(&"BTreeMap"));
        assert!(imports.contains(&"Ser"));
    }
}
