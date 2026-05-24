//! TypeScript / TSX extractor — declarations, methods, classes, interfaces, calls, imports.

use super::common::{line_of, node_text, push_call_with_type, push_def, push_import};
use super::LanguageExtractor;
use crate::store::PendingFile;
use tree_sitter::{Node, Tree};

pub struct TypescriptExtractor {
    is_tsx: bool,
}

impl TypescriptExtractor {
    pub fn new(is_tsx: bool) -> Self {
        Self { is_tsx }
    }
}

impl LanguageExtractor for TypescriptExtractor {
    fn language(&self) -> tree_sitter::Language {
        if self.is_tsx {
            tree_sitter_typescript::language_tsx()
        } else {
            tree_sitter_typescript::language_typescript()
        }
    }

    fn name(&self) -> &'static str {
        if self.is_tsx {
            "tsx"
        } else {
            "typescript"
        }
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

/// The walk logic is shared between TypeScript and JavaScript — both grammars
/// use the same node kinds for the constructs we care about. TS-only nodes
/// (`interface_declaration`, etc.) just never fire in JS files.
pub(super) fn walk(
    node: Node,
    source: &[u8],
    pending: &mut PendingFile,
    parent_index: Option<usize>,
    module_index: usize,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declaration" | "generator_function_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let idx = push_def(pending, name, "function", &child, signature, parent_index);
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "method_definition" => {
                let kind = match parent_index {
                    Some(p)
                        if matches!(pending.symbols[p].kind.as_str(), "class" | "interface") =>
                    {
                        "method"
                    }
                    _ => "function",
                };
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let idx = push_def(pending, name, kind, &child, signature, parent_index);
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "class_declaration" | "abstract_class_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let idx = push_def(pending, name, "class", &child, signature, parent_index);
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "interface_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let idx = push_def(pending, name, "interface", &child, signature, parent_index);
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "call_expression" => {
                if let Some((name, path, to_type)) = call_target_with_type(&child, source) {
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
            "new_expression" => {
                if let Some(c) = child.child_by_field_name("constructor") {
                    if let Some((name, path, to_type)) = leaf_and_path(&c, source) {
                        push_call_with_type(
                            pending,
                            parent_index.unwrap_or(module_index),
                            name,
                            path,
                            to_type,
                            line_of(&child),
                        );
                    }
                }
                walk(child, source, pending, parent_index, module_index);
            }
            "import_statement" => {
                collect_import_names(&child, source, pending, module_index);
            }
            _ => walk(child, source, pending, parent_index, module_index),
        }
    }
}

fn name_field<'a>(node: &Node, source: &'a [u8]) -> Option<&'a str> {
    node.child_by_field_name("name")
        .and_then(|n| node_text(&n, source))
}

fn call_target_with_type(
    call_node: &Node,
    source: &[u8],
) -> Option<(String, Option<String>, Option<String>)> {
    let fn_node = call_node.child_by_field_name("function")?;
    leaf_and_path(&fn_node, source)
}

/// Returns (leaf_name, full_path_in_source, to_type).
/// For `obj.foo` returns ("foo", Some("obj.foo"), None) — lowercase receiver.
/// For `JSON.parse` or `Class.method` returns ("parse"/"method", path, Some("JSON"/"Class")).
/// Heuristic: receiver identifier starting with uppercase letter is treated as type/namespace.
fn leaf_and_path(node: &Node, source: &[u8]) -> Option<(String, Option<String>, Option<String>)> {
    let full = node_text(node, source).map(String::from);
    match node.kind() {
        "identifier" | "type_identifier" => {
            let n = node_text(node, source)?.to_string();
            Some((n, full, None))
        }
        "member_expression" => {
            let leaf = node
                .child_by_field_name("property")
                .and_then(|n| node_text(&n, source))?
                .to_string();
            let to_type = type_prefix_from_object(node, source);
            Some((leaf, full, to_type))
        }
        _ => None,
    }
}

/// Walk the `object` side of a member_expression to find a capital-letter
/// receiver — that's the type/namespace by convention. `JSON.parse` → "JSON".
/// `pkg.Cls.method` → "Cls". `foo.bar.method` → None.
fn type_prefix_from_object(member: &Node, source: &[u8]) -> Option<String> {
    let obj = member.child_by_field_name("object")?;
    let candidate = match obj.kind() {
        "identifier" | "type_identifier" => node_text(&obj, source).map(String::from),
        "member_expression" => obj
            .child_by_field_name("property")
            .and_then(|n| node_text(&n, source))
            .map(String::from),
        _ => None,
    }?;
    if starts_uppercase(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn starts_uppercase(s: &str) -> bool {
    s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

fn collect_import_names(
    import_stmt: &Node,
    source: &[u8],
    pending: &mut PendingFile,
    module_index: usize,
) {
    let line = line_of(import_stmt);

    // Module source: `'bar'` in `import { foo } from 'bar'` — strip quotes.
    let module_source = import_stmt
        .child_by_field_name("source")
        .and_then(|n| node_text(&n, source))
        .map(|s| s.trim_matches(|c| c == '"' || c == '\'').to_string());

    let clause = match import_stmt.child_by_field_name("import_clause") {
        Some(c) => c,
        None => match find_child_of_kind(import_stmt, "import_clause") {
            Some(c) => c,
            None => return,
        },
    };

    let mut cursor = clause.walk();
    for child in clause.children(&mut cursor) {
        match child.kind() {
            // `import foo from 'bar';` — default import
            "identifier" => {
                if let Some(name) = node_text(&child, source) {
                    let path = module_source.as_ref().map(|m| format!("{m}::default"));
                    push_import(pending, module_index, name.to_string(), path, line);
                }
            }
            // `import * as foo from 'bar';`
            "namespace_import" => {
                let mut nc = child.walk();
                for n in child.children(&mut nc) {
                    if n.kind() == "identifier" {
                        if let Some(name) = node_text(&n, source) {
                            let path = module_source.as_ref().map(|m| format!("{m}::*"));
                            push_import(pending, module_index, name.to_string(), path, line);
                        }
                    }
                }
            }
            // `import { foo, bar as baz } from 'qux';`
            "named_imports" => {
                let mut nc = child.walk();
                for spec in child.children(&mut nc) {
                    if spec.kind() == "import_specifier" {
                        let orig = spec
                            .child_by_field_name("name")
                            .and_then(|n| node_text(&n, source));
                        let alias = spec
                            .child_by_field_name("alias")
                            .and_then(|n| node_text(&n, source));
                        let binding = alias.or(orig);
                        if let (Some(name), Some(orig_name)) = (binding, orig) {
                            let path = module_source.as_ref().map(|m| format!("{m}::{orig_name}"));
                            push_import(pending, module_index, name.to_string(), path, line);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// The `for` form is needed here — `.find()` triggers a borrow-lifetime issue
// because the `cursor` local would be dropped before the returned `Node<'tree>`.
#[allow(clippy::manual_find)]
fn find_child_of_kind<'tree>(node: &Node<'tree>, target_kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == target_kind {
            return Some(child);
        }
    }
    None
}

fn signature_until_body(node: &Node, source: &[u8]) -> Option<String> {
    let body = node.child_by_field_name("body")?;
    let header_end = body.start_byte();
    let start = node.start_byte();
    if header_end <= start {
        return None;
    }
    let text = std::str::from_utf8(&source[start..header_end]).ok()?;
    let trimmed = text
        .trim_end_matches(|c: char| c == '{' || c.is_whitespace())
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
        dir.push(format!("mmcg-ts-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn extracts_function_and_class_method() {
        let path = write_tmp(
            "functions.ts",
            "function hello(x: number): string { return String(x); }\n\
             class Foo { bar(): void { this.baz(); } baz() {} }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &TypescriptExtractor::new(false)).unwrap();
        let names: Vec<&str> = pending.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello"));
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"bar"));
        assert!(names.contains(&"baz"));
        let kinds: Vec<&str> = pending.symbols.iter().map(|s| s.kind.as_str()).collect();
        assert!(kinds.contains(&"class"));
        assert!(kinds.contains(&"method"));
    }

    #[test]
    fn extracts_imports() {
        let path = write_tmp(
            "imports.ts",
            "import foo from 'a';\n\
             import { bar, baz as qux } from 'b';\n\
             import * as ns from 'c';\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &TypescriptExtractor::new(false)).unwrap();
        let imports: Vec<&str> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "imports")
            .map(|e| e.to_name.as_str())
            .collect();
        assert!(imports.contains(&"foo"));
        assert!(imports.contains(&"bar"));
        assert!(imports.contains(&"qux"));
        assert!(imports.contains(&"ns"));
    }

    #[test]
    fn extracts_calls_and_new() {
        let path = write_tmp(
            "calls.ts",
            "function main() {\n\
                console.log('hi');\n\
                const x = new Foo();\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &TypescriptExtractor::new(false)).unwrap();
        let calls: Vec<&str> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| e.to_name.as_str())
            .collect();
        // member call → property name
        assert!(calls.contains(&"log"));
        // `new Foo()` → "Foo"
        assert!(calls.contains(&"Foo"));
    }
}
