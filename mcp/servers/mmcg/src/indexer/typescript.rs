//! TypeScript / TSX extractor — declarations, methods, classes, interfaces, calls, imports.

use super::common::{
    line_of, node_text, push_call_with_type, push_def, push_import, signature_until_body,
};
use super::{DocumentationTextBuilder, LanguageExtractor, RawConceptDocumentation};
use crate::store::PendingFile;
use std::collections::HashMap;
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
        // tree-sitter 0.23+ exposes grammars as `LANGUAGE_*` (LanguageFn consts) — .into() converts.
        if self.is_tsx {
            tree_sitter_typescript::LANGUAGE_TSX.into()
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
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

/// Walk logic shared between TypeScript and JavaScript — both grammars use the
/// same node kinds for the constructs we care about. TS-only nodes
/// (`interface_declaration`, etc.) never fire in JS files.
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
            "variable_declarator" => {
                match declare_function_value(&child, source, pending, parent_index) {
                    Some(idx) => {
                        if let Some(value) = child.child_by_field_name("value") {
                            walk(value, source, pending, Some(idx), module_index);
                        }
                    }
                    None => walk(child, source, pending, parent_index, module_index),
                }
            }
            "jsx_opening_element" | "jsx_self_closing_element" => {
                if let Some((name, path, to_type)) = jsx_component_target(&child, source) {
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

fn jsx_component_target(
    element: &Node,
    source: &[u8],
) -> Option<(String, Option<String>, Option<String>)> {
    let name_node = element.child_by_field_name("name")?;
    let (name, path, to_type) = leaf_and_path(&name_node, source)?;
    if starts_uppercase(&name) {
        Some((name, path, to_type))
    } else {
        None
    }
}

fn declare_function_value(
    declarator: &Node,
    source: &[u8],
    pending: &mut PendingFile,
    parent_index: Option<usize>,
) -> Option<usize> {
    let name_node = declarator.child_by_field_name("name")?;
    let name = node_text(&name_node, source)?.to_string();
    let value = declarator.child_by_field_name("value")?;
    let body_owner = function_body_owner(&value)?;
    let signature = declarator_signature(&name_node, &body_owner, source);
    Some(push_def(
        pending,
        name,
        "function",
        declarator,
        signature,
        parent_index,
    ))
}

fn function_body_owner<'tree>(value: &Node<'tree>) -> Option<Node<'tree>> {
    match value.kind() {
        "arrow_function" | "function_expression" | "function" => Some(*value),
        "call_expression" => {
            let args = value.child_by_field_name("arguments")?;
            let mut cursor = args.walk();
            for arg in args.children(&mut cursor) {
                if matches!(
                    arg.kind(),
                    "arrow_function" | "function_expression" | "function"
                ) {
                    return Some(arg);
                }
            }
            None
        }
        _ => None,
    }
}

fn declarator_signature(name_node: &Node, body_owner: &Node, source: &[u8]) -> Option<String> {
    let name = node_text(name_node, source)?;
    let params = body_owner
        .child_by_field_name("parameters")
        .and_then(|n| node_text(&n, source))
        .unwrap_or("()");
    Some(format!("{name}{params}"))
}

/// Returns (leaf_name, full_path_in_source, to_type).
/// `obj.foo` → ("foo", Some("obj.foo"), None) — lowercase receiver.
/// `JSON.parse` / `Class.method` → ("parse"/"method", path, Some("JSON"/"Class")).
/// Heuristic: uppercase-starting receiver identifier = type/namespace.
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

/// Walk the `object` side of a member_expression for a capital-letter receiver —
/// the type/namespace by convention. `JSON.parse` → "JSON".
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

    // Module source: `'bar'` in `import { foo } from 'bar'`, quotes stripped.
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

// `for` is needed here — `.find()` hits a borrow-lifetime issue: the `cursor`
// local would drop before the returned `Node<'tree>`.
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

fn documentation_nodes_adjacent(source: &[u8], left: &Node, right: &Node) -> bool {
    if left.end_byte() > right.start_byte()
        || right.start_position().row > left.end_position().row.saturating_add(1)
    {
        return false;
    }
    let gap = &source[left.end_byte()..right.start_byte()];
    if !gap.iter().all(u8::is_ascii_whitespace) {
        return false;
    }
    let mut newlines = 0usize;
    let mut index = 0usize;
    while index < gap.len() {
        if gap[index] == b'\r' {
            newlines += 1;
            index += usize::from(gap.get(index + 1) == Some(&b'\n'));
        } else if gap[index] == b'\n' {
            newlines += 1;
        }
        index += 1;
    }
    newlines <= 1
}

fn documentation_comment_starts_line(source: &[u8], node: &Node) -> bool {
    let line_start = source[..node.start_byte()]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    source[line_start..node.start_byte()]
        .iter()
        .all(u8::is_ascii_whitespace)
}

fn is_jsdoc(node: &Node, source: &[u8]) -> bool {
    node.kind() == "comment" && node_text(node, source).is_some_and(|text| text.starts_with("/**"))
}

fn is_file_header_jsdoc(node: &Node, source: &[u8]) -> bool {
    let Some(text) = node_text(node, source) else {
        return false;
    };
    let lower = text.to_ascii_lowercase();
    lower.contains("@file")
        || lower.contains("@fileoverview")
        || lower.contains("@license")
        || lower.contains("@copyright")
        || lower.contains("copyright")
        || lower.contains("licensed under")
        || lower.contains("spdx-license-identifier")
}

fn push_jsdoc(builder: &mut DocumentationTextBuilder, node: &Node, source: &[u8]) {
    let Some(text) = node_text(node, source) else {
        return;
    };
    let body = text
        .strip_prefix("/**")
        .unwrap_or_default()
        .strip_suffix("*/")
        .unwrap_or_default();
    for line in body.lines() {
        let line = line.trim_start();
        let line = line.strip_prefix('*').unwrap_or(line);
        builder.push_line(line.strip_prefix(' ').unwrap_or(line).trim_end());
    }
}

fn owned_symbol_nodes<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    match node.kind() {
        "export_statement" => node
            .child_by_field_name("declaration")
            .map(owned_symbol_nodes)
            .unwrap_or_default(),
        "function_declaration"
        | "generator_function_declaration"
        | "class_declaration"
        | "abstract_class_declaration"
        | "interface_declaration"
        | "method_definition" => vec![node],
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .filter(|child| {
                    child.kind() == "variable_declarator"
                        && child
                            .child_by_field_name("value")
                            .as_ref()
                            .and_then(function_body_owner)
                            .is_some()
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn symbol_index_for_declaration(
    node: &Node,
    source: &[u8],
    symbols: &HashMap<(u32, &str), usize>,
) -> Option<usize> {
    let name = node
        .child_by_field_name("name")
        .and_then(|name| node_text(&name, source))
        .unwrap_or("<anon>");
    let line = node.start_position().row as u32 + 1;
    symbols.get(&(line, name)).copied()
}

fn walk_concept_documentation(
    node: Node,
    source: &[u8],
    symbols: &HashMap<(u32, &str), usize>,
    output: &mut Vec<RawConceptDocumentation>,
) {
    let mut cursor = node.walk();
    let children = node.named_children(&mut cursor).collect::<Vec<_>>();
    for (index, wrapper) in children.iter().enumerate() {
        let owners = owned_symbol_nodes(*wrapper);
        if owners.is_empty() || index == 0 {
            continue;
        }
        let comment = children[index - 1];
        if !is_jsdoc(&comment, source)
            || (node.kind() == "program" && is_file_header_jsdoc(&comment, source))
            || !documentation_comment_starts_line(source, &comment)
            || !documentation_nodes_adjacent(source, &comment, wrapper)
        {
            continue;
        }
        for owner in owners {
            let Some(symbol_index) = symbol_index_for_declaration(&owner, source, symbols) else {
                continue;
            };
            let mut builder = DocumentationTextBuilder::default();
            push_jsdoc(&mut builder, &comment, source);
            if let Some(candidate) = builder.finish(symbol_index) {
                output.push(candidate);
            }
        }
    }
    for child in children {
        walk_concept_documentation(child, source, symbols, output);
    }
}

pub(super) fn collect_concept_documentation(
    tree: &Tree,
    source: &[u8],
    pending: &PendingFile,
) -> Vec<RawConceptDocumentation> {
    let mut output = Vec::new();
    let mut symbols = HashMap::with_capacity(pending.symbols.len());
    for (index, symbol) in pending.symbols.iter().enumerate() {
        if symbol.kind != "module" {
            symbols
                .entry((symbol.line_start, symbol.name.as_str()))
                .or_insert(index);
        }
    }
    walk_concept_documentation(tree.root_node(), source, &symbols, &mut output);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::common;
    use crate::indexer::parse_one;

    #[test]
    fn extracts_function_and_class_method() {
        let path = common::write_tmp(
            "ts",
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
        let path = common::write_tmp(
            "ts",
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
    fn extracts_variable_declared_functions_with_parameter_signatures() {
        let path = common::write_tmp(
            "ts",
            "declared.tsx",
            "export const Card = ({ title }: Props) => <div />;\n\
             const Plain = function (a: number) { return a; };\n\
             export const Wrapped = memo((props: Props) => <div />);\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &TypescriptExtractor::new(true)).unwrap();
        let found: Vec<(&str, Option<&str>)> = pending
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.signature.as_deref()))
            .collect();
        assert!(found.contains(&("Card", Some("Card({ title }: Props)"))));
        assert!(found.contains(&("Plain", Some("Plain(a: number)"))));
        assert!(found.contains(&("Wrapped", Some("Wrapped(props: Props)"))));
    }

    #[test]
    fn jsx_usage_becomes_a_call_edge_and_host_elements_do_not() {
        let path = common::write_tmp(
            "ts",
            "usage.tsx",
            "export function Screen() {\n\
                return <section><Button /><Select.Option /><div /></section>;\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &TypescriptExtractor::new(true)).unwrap();
        let calls: Vec<&str> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| e.to_name.as_str())
            .collect();
        assert!(calls.contains(&"Button"));
        assert!(calls.contains(&"Option"));
        assert!(!calls.contains(&"section"));
        assert!(!calls.contains(&"div"));
        assert_eq!(calls.iter().filter(|n| **n == "Button").count(), 1);
    }

    #[test]
    fn extracts_calls_and_new() {
        let path = common::write_tmp(
            "ts",
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
        // new Foo() → "Foo"
        assert!(calls.contains(&"Foo"));
    }
}
