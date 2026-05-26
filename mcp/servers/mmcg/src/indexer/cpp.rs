//! C / C++ extractor — best-effort syntactic surface scan.
//!
//! # Precision disclaimer (read before trusting query results)
//!
//! Unlike the Python/TS/Rust/C#/Go/Java/PHP extractors, C/C++ via tree-sitter
//! alone is fundamentally lower precision because the language's semantics
//! depend on the **preprocessor** and **template instantiation** which
//! tree-sitter does not run. Known false-positive / false-negative classes:
//!
//! - **Macros are invisible.** `TEST(SuiteName, TestName) { ... }` (gtest) is
//!   parsed as a call to `TEST`, not as a function definition — so the test
//!   body is unindexed and the synthetic function doesn't appear in
//!   `mmcg_search`. Similarly any call inside `ASSERT_EQ(...)` or other macro
//!   arguments may or may not be visible depending on the macro shape.
//! - **Template instantiations** are not tracked. `foo<int>(x)` records a call
//!   to `foo`, but `vector<T>::push_back(x)` records `push_back` with whatever
//!   the receiver looks like syntactically.
//! - **Header / source split.** `void Foo::bar()` defined in `.cpp` and
//!   declared `void bar();` in `.h` produce two symbol rows with the same name.
//!   No partial-class-style collapse — they're genuinely different declarations.
//! - **ADL / overload resolution** is not performed. `swap(a, b)` records a
//!   call to `swap`; whether that resolves to `std::swap`, a member, or a
//!   namespace-qualified version requires semantic analysis we don't do.
//! - **`#include`** is captured as an `imports` edge with the header name, but
//!   the *contents* brought in by the include are NOT followed.
//!
//! For high-precision C++ structural analysis use `clangd` (semantic, slow,
//! large) or `ctags` (fast, similar precision tradeoffs to this extractor).

use super::common::{
    line_of, node_text, push_call_with_type, push_def_with_decorators, push_import,
};
use super::LanguageExtractor;
use crate::store::PendingFile;
use tree_sitter::{Node, Tree};

pub struct CppExtractor;

impl LanguageExtractor for CppExtractor {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_cpp::LANGUAGE.into()
    }

    fn name(&self) -> &'static str {
        "cpp"
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
            // Transparent — `namespace foo { ... }`.
            "namespace_definition" => {
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, parent_index, module_index);
                }
            }
            // Templates wrap a single declaration — walk through with same parent.
            // The wrapped declaration captures its own symbol; we don't push a
            // separate symbol for the `template<>` itself.
            "template_declaration" => {
                walk(child, source, pending, parent_index, module_index);
            }
            "class_specifier" | "struct_specifier" | "union_specifier" => {
                let name = name_field(&child, source).map(String::from);
                let body = child.child_by_field_name("body");
                // Forward declarations (`class Foo;` with no body) skip — they
                // surface as duplicates of the real definition.
                if body.is_none() {
                    walk(child, source, pending, parent_index, module_index);
                    continue;
                }
                let kind = match child.kind() {
                    "struct_specifier" => "struct",
                    "union_specifier" => "union",
                    _ => "class",
                };
                let name = name.unwrap_or_else(|| "<anon>".to_string());
                let signature = signature_until_body(&child, source);
                let idx = push_def_with_decorators(
                    pending,
                    name,
                    kind,
                    &child,
                    signature,
                    parent_index,
                    None,
                );
                walk(body.unwrap(), source, pending, Some(idx), module_index);
            }
            "enum_specifier" => {
                let name = name_field(&child, source).map(String::from);
                if name.is_none() {
                    continue;
                }
                let signature = signature_until_body(&child, source);
                push_def_with_decorators(
                    pending,
                    name.unwrap(),
                    "enum",
                    &child,
                    signature,
                    parent_index,
                    None,
                );
            }
            // `function_definition` covers both top-level fns and class methods
            // (when nested inside a class_specifier body) AND out-of-class method
            // definitions like `void Foo::bar() {}`.
            "function_definition" => {
                let (name, receiver) = extract_function_name(&child, source);
                let kind = if receiver.is_some()
                    || matches!(
                        parent_index
                            .and_then(|p| pending.symbols.get(p))
                            .map(|s| s.kind.as_str()),
                        Some("class" | "struct" | "union")
                    ) {
                    "method"
                } else {
                    "function"
                };
                let signature = signature_until_body(&child, source);
                let idx = push_def_with_decorators(
                    pending,
                    name,
                    kind,
                    &child,
                    signature,
                    parent_index,
                    None,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "preproc_include" => {
                emit_include(&child, source, pending, module_index);
            }
            "using_declaration" => {
                emit_using(&child, source, pending, module_index);
            }
            "call_expression" => {
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
            "new_expression" => {
                if let Some(t) = child.child_by_field_name("type") {
                    let name = node_text(&t, source).and_then(|s| {
                        let base = s.split('<').next().unwrap_or(s);
                        base.rsplit("::").next().map(str::to_string)
                    });
                    if let Some(n) = name {
                        push_call_with_type(
                            pending,
                            parent_index.unwrap_or(module_index),
                            n.clone(),
                            Some(n),
                            None,
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

/// Returns `(method_name, optional_receiver_type)` for a function_definition.
/// `void foo() {}` → ("foo", None)
/// `void Foo::bar() {}` → ("bar", Some("Foo"))
/// `Foo::Foo() {}` (constructor) → ("Foo", Some("Foo"))
fn extract_function_name(fn_def: &Node, source: &[u8]) -> (String, Option<String>) {
    let Some(declarator) = fn_def.child_by_field_name("declarator") else {
        return ("<anon>".to_string(), None);
    };
    extract_declarator_name(&declarator, source)
}

fn extract_declarator_name(node: &Node, source: &[u8]) -> (String, Option<String>) {
    match node.kind() {
        "function_declarator" => {
            if let Some(inner) = node.child_by_field_name("declarator") {
                return extract_declarator_name(&inner, source);
            }
            ("<anon>".to_string(), None)
        }
        "identifier" | "field_identifier" => (
            node_text(node, source).unwrap_or("<anon>").to_string(),
            None,
        ),
        "qualified_identifier" => {
            // Walk the scope chain: scope::scope::name. Final `name` is the
            // method, the rightmost scope is the receiver type.
            let name = node
                .child_by_field_name("name")
                .map(|n| match n.kind() {
                    "qualified_identifier" | "destructor_name" | "operator_name" => {
                        extract_declarator_name(&n, source).0
                    }
                    _ => node_text(&n, source).unwrap_or("<anon>").to_string(),
                })
                .unwrap_or_else(|| "<anon>".to_string());
            let scope = node
                .child_by_field_name("scope")
                .and_then(|s| node_text(&s, source))
                .map(|s| s.rsplit("::").next().unwrap_or(s).to_string());
            (name, scope)
        }
        "operator_name" | "destructor_name" => (
            node_text(node, source).unwrap_or("<anon>").to_string(),
            None,
        ),
        "reference_declarator" | "pointer_declarator" => {
            // Wrappers around the real declarator.
            let mut cursor = node.walk();
            for c in node.named_children(&mut cursor) {
                let (name, recv) = extract_declarator_name(&c, source);
                if name != "<anon>" {
                    return (name, recv);
                }
            }
            ("<anon>".to_string(), None)
        }
        _ => {
            // Best-effort: scan for first usable identifier.
            let mut cursor = node.walk();
            for c in node.named_children(&mut cursor) {
                let (name, recv) = extract_declarator_name(&c, source);
                if name != "<anon>" {
                    return (name, recv);
                }
            }
            ("<anon>".to_string(), None)
        }
    }
}

fn leaf_and_path(node: &Node, source: &[u8]) -> Option<(String, Option<String>, Option<String>)> {
    let full = node_text(node, source).map(String::from);
    match node.kind() {
        "identifier" | "type_identifier" => {
            let n = node_text(node, source)?.to_string();
            Some((n, full, None))
        }
        "field_expression" => {
            let field = node
                .child_by_field_name("field")
                .and_then(|n| node_text(&n, source))?
                .to_string();
            Some((field, full, None))
        }
        "qualified_identifier" => {
            let name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(&n, source))?
                .to_string();
            let scope = node
                .child_by_field_name("scope")
                .and_then(|s| node_text(&s, source))
                .map(|s| s.rsplit("::").next().unwrap_or(s).to_string());
            let to_type = scope.filter(|s| starts_uppercase(s));
            Some((name, full, to_type))
        }
        "template_function" => {
            // `foo<int>(...)` — pull just `foo`.
            let name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(&n, source))?
                .to_string();
            Some((name, full, None))
        }
        _ => None,
    }
}

fn starts_uppercase(s: &str) -> bool {
    s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

/// `#include "foo.h"` / `#include <vector>` → import edge with the header name.
fn emit_include(decl: &Node, source: &[u8], pending: &mut PendingFile, module_index: usize) {
    let line = line_of(decl);
    let Some(path_node) = decl.child_by_field_name("path") else {
        return;
    };
    let raw = node_text(&path_node, source).unwrap_or("").trim();
    if raw.is_empty() {
        return;
    }
    let cleaned = raw
        .trim_start_matches(['<', '"'])
        .trim_end_matches(['>', '"']);
    // Leaf = filename without dir prefix.
    let leaf = cleaned.rsplit('/').next().unwrap_or(cleaned).to_string();
    push_import(
        pending,
        module_index,
        leaf,
        Some(format!("{cleaned}::*")),
        line,
    );
}

/// `using std::vector;` → import edge with leaf = `vector`, path = `std::vector`.
/// `using namespace std;` is harder (introduces every name in `std`) — we emit
/// one edge with name `*` so `imported_by std` can find it.
fn emit_using(decl: &Node, source: &[u8], pending: &mut PendingFile, module_index: usize) {
    let line = line_of(decl);
    let Some(text) = node_text(decl, source) else {
        return;
    };
    let raw = text.trim().trim_end_matches(';').trim();
    let after_using = raw.strip_prefix("using").unwrap_or(raw).trim_start();
    if let Some(ns) = after_using.strip_prefix("namespace ") {
        let ns = ns.trim();
        push_import(
            pending,
            module_index,
            "*".to_string(),
            Some(format!("{ns}::*")),
            line,
        );
        return;
    }
    if after_using.is_empty() {
        return;
    }
    let path = after_using.trim();
    let leaf = path.rsplit("::").next().unwrap_or(path).to_string();
    push_import(pending, module_index, leaf, Some(path.to_string()), line);
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
        dir.push(format!("mmcg-cpp-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn extracts_class_and_method_definitions() {
        let path = write_tmp(
            "Foo.cpp",
            "namespace app {\n\
                 class Foo {\n\
                 public:\n\
                     int bar(int x) { return x + 1; }\n\
                 };\n\
             }\n\
             int app::Foo::baz() { return 0; }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CppExtractor).unwrap();
        let by_name: std::collections::HashMap<&str, &str> = pending
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind.as_str()))
            .collect();
        assert_eq!(by_name["Foo"], "class");
        assert_eq!(by_name["bar"], "method");
        // Out-of-class definition `app::Foo::baz` recognized as method via receiver.
        assert_eq!(by_name["baz"], "method");
    }

    #[test]
    fn extracts_struct_union_enum() {
        let path = write_tmp(
            "Types.c",
            "struct Point { int x; int y; };\n\
             union Value { int i; float f; };\n\
             enum Color { RED, GREEN, BLUE };\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CppExtractor).unwrap();
        let by_name: std::collections::HashMap<&str, &str> = pending
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind.as_str()))
            .collect();
        assert_eq!(by_name["Point"], "struct");
        assert_eq!(by_name["Value"], "union");
        assert_eq!(by_name["Color"], "enum");
    }

    #[test]
    fn extracts_includes_and_using() {
        let path = write_tmp(
            "Includes.cpp",
            "#include <vector>\n\
             #include \"local.h\"\n\
             #include \"sub/dir/nested.h\"\n\
             using std::vector;\n\
             using namespace app;\n\
             int main() { return 0; }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CppExtractor).unwrap();
        let imports: Vec<(&str, Option<&str>)> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "imports")
            .map(|e| (e.to_name.as_str(), e.to_path.as_deref()))
            .collect();
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "vector" && *p == Some("vector::*")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "local.h" && *p == Some("local.h::*")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "nested.h" && *p == Some("sub/dir/nested.h::*")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "vector" && *p == Some("std::vector")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "*" && *p == Some("app::*")));
    }

    #[test]
    fn extracts_calls_and_new() {
        let path = write_tmp(
            "Calls.cpp",
            "#include <iostream>\n\
             struct Foo {};\n\
             void helper() {}\n\
             int main() {\n\
                 std::cout << \"hi\";\n\
                 helper();\n\
                 auto* f = new Foo();\n\
                 return 0;\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CppExtractor).unwrap();
        let calls: Vec<&str> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| e.to_name.as_str())
            .collect();
        assert!(calls.contains(&"helper"));
        assert!(calls.contains(&"Foo"));
    }
}
