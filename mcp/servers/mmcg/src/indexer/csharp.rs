//! C# extractor — namespaces, classes/structs/records/interfaces, methods,
//! properties, using directives, calls, attributes.

use super::common::{
    line_of, node_text, push_call_with_type, push_def_with_decorators, push_import,
};
use super::LanguageExtractor;
use crate::store::PendingFile;
use tree_sitter::{Node, Tree};

pub struct CsharpExtractor;

impl LanguageExtractor for CsharpExtractor {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_c_sharp::LANGUAGE.into()
    }

    fn name(&self) -> &'static str {
        "csharp"
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
            // Transparent containers — walk their children with the same parent context.
            // C# 10 file-scoped namespaces (`namespace Foo;`) have their siblings as
            // module-level, so we keep recursing past them.
            "namespace_declaration" => {
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, parent_index, module_index);
                } else {
                    walk(child, source, pending, parent_index, module_index);
                }
            }
            "file_scoped_namespace_declaration" => {
                walk(child, source, pending, parent_index, module_index);
            }
            "class_declaration" | "struct_declaration" | "record_declaration" => {
                let kind = if child.kind() == "struct_declaration" {
                    "struct"
                } else {
                    "class"
                };
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let decorators = merge_decorators(
                    collect_attributes(&child, source),
                    collect_modifiers(&child, source),
                );
                let idx = push_def_with_decorators(
                    pending,
                    name,
                    kind,
                    &child,
                    signature,
                    parent_index,
                    decorators,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "interface_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let decorators = collect_attributes(&child, source);
                let idx = push_def_with_decorators(
                    pending,
                    name,
                    "interface",
                    &child,
                    signature,
                    parent_index,
                    decorators,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "enum_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                push_def_with_decorators(
                    pending,
                    name,
                    "enum",
                    &child,
                    signature,
                    parent_index,
                    collect_attributes(&child, source),
                );
            }
            "method_declaration" | "local_function_statement" => {
                let kind = method_kind_for(parent_index, pending);
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let decorators = collect_attributes(&child, source);
                let idx = push_def_with_decorators(
                    pending,
                    name,
                    kind,
                    &child,
                    signature,
                    parent_index,
                    decorators,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "constructor_declaration" => {
                let name = name_field(&child, source).unwrap_or("<ctor>").to_string();
                let signature = signature_until_body(&child, source);
                let decorators = collect_attributes(&child, source);
                let idx = push_def_with_decorators(
                    pending,
                    name,
                    "method",
                    &child,
                    signature,
                    parent_index,
                    decorators,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "property_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = property_signature(&child, source);
                push_def_with_decorators(
                    pending,
                    name,
                    "property",
                    &child,
                    signature,
                    parent_index,
                    collect_attributes(&child, source),
                );
                // Don't recurse into accessor bodies for symbols, but do walk for calls.
                if let Some(acc) = child.child_by_field_name("accessors") {
                    walk(acc, source, pending, parent_index, module_index);
                }
            }
            "using_directive" => {
                collect_using(&child, source, pending, module_index);
            }
            "invocation_expression" => {
                if let Some(fn_node) = child.child_by_field_name("function") {
                    if let Some((name, path, to_type)) = leaf_and_path(&fn_node, source) {
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
            "object_creation_expression" => {
                if let Some(t) = child.child_by_field_name("type") {
                    if let Some((name, path, to_type)) = leaf_and_path(&t, source) {
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
            _ => walk(child, source, pending, parent_index, module_index),
        }
    }
}

fn name_field<'a>(node: &Node, source: &'a [u8]) -> Option<&'a str> {
    node.child_by_field_name("name")
        .and_then(|n| node_text(&n, source))
}

fn method_kind_for(parent_index: Option<usize>, pending: &PendingFile) -> &'static str {
    match parent_index {
        Some(p)
            if matches!(
                pending.symbols[p].kind.as_str(),
                "class" | "struct" | "interface" | "record"
            ) =>
        {
            "method"
        }
        _ => "function",
    }
}

/// Extract `(leaf, full_text, to_type)` for an invocation target or
/// object-creation type. Mirrors the TS extractor's resolution rules:
/// - `Foo()` → ("Foo", "Foo", None)
/// - `obj.Bar()` → ("Bar", "obj.Bar", None) when receiver is lowercase
/// - `Type.Bar()` → ("Bar", "Type.Bar", Some("Type")) when receiver is uppercase
fn leaf_and_path(node: &Node, source: &[u8]) -> Option<(String, Option<String>, Option<String>)> {
    let full = node_text(node, source).map(String::from);
    match node.kind() {
        "identifier" => {
            let n = node_text(node, source)?.to_string();
            Some((n, full, None))
        }
        "generic_name" => {
            // Foo<T> — pull the leading identifier
            let n = node
                .child_by_field_name("name")
                .and_then(|c| node_text(&c, source))
                .or_else(|| first_identifier_text(node, source))?
                .to_string();
            Some((n, full, None))
        }
        "member_access_expression" => {
            let leaf = node
                .child_by_field_name("name")
                .and_then(|n| leaf_of_name(&n, source))?;
            let to_type = type_prefix_from_expression(node, source);
            Some((leaf, full, to_type))
        }
        "qualified_name" => {
            // `System.Console` used as a type or static call target
            let leaf = node
                .child_by_field_name("name")
                .and_then(|n| leaf_of_name(&n, source))?;
            let qualifier = node
                .child_by_field_name("qualifier")
                .and_then(|n| node_text(&n, source))
                .and_then(|s| s.rsplit('.').next().map(str::to_string));
            let to_type = qualifier.filter(|s| starts_uppercase(s));
            Some((leaf, full, to_type))
        }
        "type" => {
            // Drill into the inner type expression.
            let inner = first_named_child(node)?;
            leaf_and_path(&inner, source)
        }
        _ => {
            // Best-effort fallback: drill into the first named child.
            let inner = first_named_child(node)?;
            leaf_and_path(&inner, source)
        }
    }
}

fn leaf_of_name(node: &Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, source).map(String::from),
        "generic_name" => node
            .child_by_field_name("name")
            .and_then(|c| node_text(&c, source))
            .or_else(|| first_identifier_text(node, source))
            .map(String::from),
        _ => node_text(node, source).map(String::from),
    }
}

/// For `X.Y.Method` or `Type.Method` extract `Y`/`Type` as the receiver, mirroring
/// the TS extractor's heuristic — uppercase receiver = type/namespace.
fn type_prefix_from_expression(member: &Node, source: &[u8]) -> Option<String> {
    let expr = member.child_by_field_name("expression")?;
    let candidate = match expr.kind() {
        "identifier" => node_text(&expr, source).map(String::from),
        "generic_name" => expr
            .child_by_field_name("name")
            .and_then(|n| node_text(&n, source))
            .map(String::from),
        "member_access_expression" => expr
            .child_by_field_name("name")
            .and_then(|n| leaf_of_name(&n, source)),
        "qualified_name" => expr
            .child_by_field_name("name")
            .and_then(|n| leaf_of_name(&n, source)),
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

fn first_identifier_text<'a>(node: &Node, source: &'a [u8]) -> Option<&'a str> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return node_text(&child, source);
        }
    }
    None
}

fn first_named_child<'tree>(node: &Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let mut found = None;
    for child in node.children(&mut cursor) {
        if child.is_named() {
            found = Some(child);
            break;
        }
    }
    found
}

/// `using System.Collections.Generic;` → import name "Generic",
/// path "System.Collections.Generic::*". `using static System.Math;` and
/// `using Alias = System.IO;` strip their prefixes the same way.
fn collect_using(directive: &Node, source: &[u8], pending: &mut PendingFile, module_index: usize) {
    let line = line_of(directive);
    let Some(text) = node_text(directive, source) else {
        return;
    };
    let raw = text.trim().trim_end_matches(';').trim();
    // Strip leading `using` (and optional `static`).
    let after_using = raw.strip_prefix("using").unwrap_or(raw).trim_start();
    let after_static = after_using
        .strip_prefix("static ")
        .map(str::trim_start)
        .unwrap_or(after_using);
    // Alias form: `Alias = Real.Namespace` — take the right side as the imported path.
    let path = if let Some((_, rhs)) = after_static.split_once('=') {
        rhs.trim()
    } else {
        after_static
    };
    if path.is_empty() {
        return;
    }
    let leaf = path.rsplit('.').next().unwrap_or(path).to_string();
    let to_path = Some(format!("{path}::*"));
    push_import(pending, module_index, leaf, to_path, line);
}

/// Collect attribute names from a declaration's `attribute_list` children.
/// Returns `,Name1,Name2,` formatted so `unreferenced` can match individual
/// names with `LIKE '%,Name1,%'`. Mirrors the Python decorator + Rust attribute
/// schemes for cross-language consistency.
fn collect_attributes(decl: &Node, source: &[u8]) -> Option<String> {
    let mut names: Vec<String> = Vec::new();
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        if child.kind() != "attribute_list" {
            continue;
        }
        let mut ac = child.walk();
        for attr in child.children(&mut ac) {
            if attr.kind() != "attribute" {
                continue;
            }
            if let Some(name_node) = attr.child_by_field_name("name") {
                let raw = node_text(&name_node, source).unwrap_or("").trim();
                if raw.is_empty() {
                    continue;
                }
                // For qualified names like `System.Web.HttpGet` keep only the leaf.
                let leaf = raw.rsplit('.').next().unwrap_or(raw);
                // Drop trailing `Attribute` suffix — C# convention allows `[Test]` for
                // `TestAttribute`; both should match the same filter rule.
                let stripped = leaf.strip_suffix("Attribute").unwrap_or(leaf);
                if !stripped.is_empty() {
                    names.push(stripped.to_string());
                }
            }
        }
    }
    if names.is_empty() {
        None
    } else {
        Some(format!(",{},", names.join(",")))
    }
}

/// Collect modifier keywords (`partial`, `abstract`, `sealed`, `static`) from
/// a declaration. Stored alongside attributes in the decorators column so
/// queries can filter on them (e.g. partial-class collapse in `mmcg_search`).
fn collect_modifiers(decl: &Node, source: &[u8]) -> Option<String> {
    const TRACKED: &[&str] = &["partial", "abstract", "sealed", "static"];
    let mut names: Vec<String> = Vec::new();
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        if child.kind() != "modifier" {
            continue;
        }
        let text = node_text(&child, source).unwrap_or("").trim();
        if TRACKED.contains(&text) {
            names.push(text.to_string());
        }
    }
    if names.is_empty() {
        None
    } else {
        Some(format!(",{},", names.join(",")))
    }
}

/// Merge two `,name,` formatted decorator strings into one. Used to combine
/// attributes (`[Fact]`) and modifiers (`partial`) on the same declaration.
fn merge_decorators(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (None, None) => None,
        (Some(x), None) | (None, Some(x)) => Some(x),
        (Some(x), Some(y)) => {
            // Both are `,name,` formatted — strip the trailing comma of `x` and the
            // leading comma of `y` before joining.
            let trimmed_y = y.trim_start_matches(',');
            Some(format!("{x}{trimmed_y}"))
        }
    }
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
        .trim_end_matches(|c: char| c == '{' || c == '=' || c == '>' || c.is_whitespace())
        .to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn property_signature(node: &Node, source: &[u8]) -> Option<String> {
    let start = node.start_byte();
    let end = node
        .child_by_field_name("accessors")
        .map(|n| n.start_byte())
        .or_else(|| node.child_by_field_name("value").map(|n| n.start_byte()))
        .unwrap_or_else(|| node.end_byte());
    if end <= start {
        return None;
    }
    let text = std::str::from_utf8(&source[start..end]).ok()?;
    let trimmed = text
        .trim_end_matches(|c: char| c == '{' || c == '=' || c == '>' || c.is_whitespace())
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
        dir.push(format!("mmcg-cs-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn extracts_class_method_and_property() {
        let path = write_tmp(
            "Foo.cs",
            "namespace App {\n\
                 public class Foo {\n\
                     public string Name { get; set; }\n\
                     public int Bar(int x) { return x + 1; }\n\
                 }\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CsharpExtractor).unwrap();
        let names: Vec<&str> = pending.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"), "missing class Foo");
        assert!(names.contains(&"Bar"), "missing method Bar");
        assert!(names.contains(&"Name"), "missing property Name");
        let kinds: Vec<&str> = pending.symbols.iter().map(|s| s.kind.as_str()).collect();
        assert!(kinds.contains(&"class"));
        assert!(kinds.contains(&"method"));
        assert!(kinds.contains(&"property"));
    }

    #[test]
    fn extracts_file_scoped_namespace() {
        let path = write_tmp(
            "Scoped.cs",
            "namespace App.Sub;\n\
             public class Service { public void Run() {} }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CsharpExtractor).unwrap();
        let names: Vec<&str> = pending.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Service"));
        assert!(names.contains(&"Run"));
    }

    #[test]
    fn extracts_using_directives() {
        let path = write_tmp(
            "Usings.cs",
            "using System;\n\
             using System.Collections.Generic;\n\
             using static System.Math;\n\
             using Coll = System.Collections;\n\
             class X {}\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CsharpExtractor).unwrap();
        let imports: Vec<&str> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "imports")
            .map(|e| e.to_name.as_str())
            .collect();
        assert!(imports.contains(&"System"));
        assert!(imports.contains(&"Generic"));
        assert!(imports.contains(&"Math"));
        assert!(imports.contains(&"Collections"));
    }

    #[test]
    fn extracts_calls_and_new() {
        let path = write_tmp(
            "Calls.cs",
            "using System;\n\
             class Main {\n\
                 public void Run() {\n\
                     Console.WriteLine(\"hi\");\n\
                     var list = new System.Collections.Generic.List<int>();\n\
                     Helper();\n\
                 }\n\
                 void Helper() {}\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CsharpExtractor).unwrap();
        let calls: Vec<&str> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| e.to_name.as_str())
            .collect();
        assert!(calls.contains(&"WriteLine"), "missing WriteLine call");
        assert!(calls.contains(&"Helper"), "missing Helper call");
        assert!(calls.contains(&"List"), "missing List from new");

        // WriteLine should carry Console as the type prefix
        let writeline_to_type = pending
            .edges
            .iter()
            .find(|e| e.kind == "calls" && e.to_name == "WriteLine")
            .and_then(|e| e.to_type.as_deref());
        assert_eq!(writeline_to_type, Some("Console"));
    }

    #[test]
    fn captures_partial_modifier() {
        let path = write_tmp(
            "Partial.cs",
            "namespace App;\n\
             public partial class User { public string Name { get; set; } = \"\"; }\n\
             public sealed class Admin {}\n\
             public class Normal {}\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CsharpExtractor).unwrap();
        let by_name: std::collections::HashMap<&str, &Option<String>> = pending
            .symbols
            .iter()
            .filter(|s| s.kind == "class")
            .map(|s| (s.name.as_str(), &s.decorators))
            .collect();
        assert_eq!(by_name["User"].as_deref(), Some(",partial,"));
        assert_eq!(by_name["Admin"].as_deref(), Some(",sealed,"));
        assert_eq!(by_name.get("Normal").and_then(|d| d.as_deref()), None);
    }

    #[test]
    fn captures_attributes_into_decorators() {
        let path = write_tmp(
            "Tests.cs",
            "using Xunit;\n\
             public class FooTests {\n\
                 [Fact] public void ShouldDoX() {}\n\
                 [Theory, InlineData(1)] public void ShouldDoY(int n) {}\n\
                 [HttpGet(\"/api/foo\")] public void Endpoint() {}\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CsharpExtractor).unwrap();
        let by_name: std::collections::HashMap<&str, &Option<String>> = pending
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), &s.decorators))
            .collect();
        assert_eq!(by_name["ShouldDoX"].as_deref(), Some(",Fact,"));
        assert!(by_name["ShouldDoY"]
            .as_deref()
            .map(|s| s.contains(",Theory,"))
            .unwrap_or(false));
        assert_eq!(by_name["Endpoint"].as_deref(), Some(",HttpGet,"));
    }
}
