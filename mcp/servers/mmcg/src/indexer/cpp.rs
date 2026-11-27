//! C / C++ extractor — best-effort syntactic surface scan.
//!
//! # Precision disclaimer (read before trusting query results)
//!
//! Unlike the Python/TS/Rust/C#/Go/Java/PHP extractors, C/C++ via tree-sitter
//! alone is fundamentally lower precision: semantics depend on the
//! **preprocessor** and **template instantiation**, which tree-sitter doesn't
//! run. Known false-positive / false-negative classes:
//!
//! - **Macros are invisible.** `TEST(SuiteName, TestName) { ... }` (gtest)
//!   parses as a call to `TEST`, not a function definition — so the test body
//!   is unindexed and the synthetic function is absent from `mmcg_search`.
//!   Calls inside `ASSERT_EQ(...)` or other macro args may or may not be
//!   visible depending on the macro shape.
//! - **Template instantiations** aren't tracked. `foo<int>(x)` records a call
//!   to `foo`, but `vector<T>::push_back(x)` records `push_back` with whatever
//!   the receiver looks like syntactically.
//! - **Header / source split.** `void Foo::bar()` in `.cpp` and `void bar();`
//!   in `.h` produce two symbol rows with the same name. No partial-class-style
//!   collapse — they're genuinely different declarations.
//! - **ADL / overload resolution** isn't done. `swap(a, b)` records a call to
//!   `swap`; resolving it to `std::swap`, a member, or a namespace-qualified
//!   version needs semantic analysis we don't do.
//! - **Namespaces** are symbols that own their members, so a qualified name
//!   such as `billing::tax::rate` resolves. An anonymous namespace is not.
//! - **`#include`** is captured as an `imports` edge with the header name, but
//!   the *contents* it brings in are NOT followed.
//!
//! For high-precision C++ structural analysis use `clangd` (semantic, slow,
//! large) or `ctags` (fast, similar precision tradeoffs to this extractor).

use super::common::{
    line_of, name_field, node_text, push_call_with_type, push_def_with_decorators, push_import,
    signature_until_body, starts_uppercase,
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
            // `namespace billing { }` and `namespace billing::tax { }` own
            // their members. An anonymous namespace only limits linkage, so
            // its members keep the enclosing parent.
            "namespace_definition" => {
                let body = child.child_by_field_name("body");
                match child
                    .child_by_field_name("name")
                    .and_then(|name| node_text(&name, source))
                {
                    Some(name) => {
                        let name = name.to_string();
                        let idx = push_def_with_decorators(
                            pending,
                            name.clone(),
                            "namespace",
                            &child,
                            Some(format!("namespace {name}")),
                            parent_index,
                            None,
                        );
                        if let Some(body) = body {
                            walk(body, source, pending, Some(idx), module_index);
                        }
                    }
                    None => {
                        if let Some(body) = body {
                            walk(body, source, pending, parent_index, module_index);
                        }
                    }
                }
            }
            // `namespace fs = std::filesystem;` brings a namespace in under a
            // local name, like `using namespace`.
            "namespace_alias_definition" => {
                emit_namespace_alias(&child, source, pending, module_index);
            }
            // `using Id = std::uint64_t;` and `typedef struct Node *NodeRef;`
            "alias_declaration" => {
                if let Some(name) = name_field(&child, source) {
                    push_def_with_decorators(
                        pending,
                        name.to_string(),
                        "type",
                        &child,
                        declaration_signature(&child, source),
                        parent_index,
                        None,
                    );
                }
            }
            "type_definition" => {
                push_typedef_names(&child, source, pending, parent_index);
                // `typedef struct Node { ... } Node;` also defines the struct.
                walk(child, source, pending, parent_index, module_index);
            }
            // Templates wrap a single declaration — walk through with same
            // parent. The wrapped declaration captures its own symbol; no
            // separate symbol for the `template<>` itself.
            "template_declaration" => {
                walk(child, source, pending, parent_index, module_index);
            }
            "class_specifier" | "struct_specifier" | "union_specifier" => {
                let name = name_field(&child, source).map(String::from);
                let body = child.child_by_field_name("body");
                // Skip forward declarations (`class Foo;`, no body) — they
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
            // `function_definition` covers top-level fns, class methods (nested
            // in a class_specifier body), and out-of-class method definitions
            // like `void Foo::bar() {}`.
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
            // Header declarations use `declaration` at file scope and
            // `field_declaration` inside a class body. Only accept declarators
            // that actually contain a function declarator so ordinary fields
            // and variables do not become callable symbols.
            "declaration" | "field_declaration" => {
                let declaration_scope = parent_index
                    .and_then(|index| pending.symbols.get(index))
                    .map(|symbol| symbol.kind.as_str());
                if parent_index != Some(module_index)
                    && !matches!(
                        declaration_scope,
                        Some("class" | "struct" | "union" | "namespace")
                    )
                {
                    // A direct initializer such as `Iterator end, it(path)`
                    // can look like a function declarator without semantic
                    // type information. Declarations inside function bodies
                    // are therefore traversed for calls, but never promoted
                    // to header symbols.
                    walk(child, source, pending, parent_index, module_index);
                    continue;
                }

                let mut cursor = child.walk();
                let declarators: Vec<_> = child
                    .children_by_field_name("declarator", &mut cursor)
                    .collect();
                if declarators.is_empty() {
                    walk(child, source, pending, parent_index, module_index);
                    continue;
                }

                let mut emitted = false;
                for declarator in declarators {
                    if !contains_kind(declarator, "function_declarator") {
                        continue;
                    }

                    let (name, receiver) = extract_declarator_name(&declarator, source);
                    if name == "<anon>" {
                        continue;
                    }
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
                    push_def_with_decorators(
                        pending,
                        name,
                        kind,
                        &child,
                        declaration_signature(&child, source),
                        parent_index,
                        None,
                    );
                    emitted = true;
                }

                if !emitted {
                    walk(child, source, pending, parent_index, module_index);
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
            // Scope chain scope::scope::name — final `name` is the method, the
            // rightmost scope is the receiver type.
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

fn contains_kind(node: Node<'_>, expected: &str) -> bool {
    if node.kind() == expected {
        return true;
    }
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .any(|child| contains_kind(child, expected));
    found
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
            // `foo<int>(...)` — pull `foo`.
            let name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(&n, source))?
                .to_string();
            Some((name, full, None))
        }
        _ => None,
    }
}

/// One `type` symbol per typedef name. `typedef struct Node { } Node;` names
/// the struct it defines, which is already a symbol, so that name is skipped.
fn push_typedef_names(
    definition: &Node,
    source: &[u8],
    pending: &mut PendingFile,
    parent_index: Option<usize>,
) {
    let tag = definition
        .child_by_field_name("type")
        .and_then(|tag| tag.child_by_field_name("body").and(Some(tag)))
        .and_then(|tag| name_field(&tag, source));
    let mut cursor = definition.walk();
    let declarators: Vec<_> = definition
        .children_by_field_name("declarator", &mut cursor)
        .collect();
    for declarator in declarators {
        let Some(name) = typedef_name(declarator, source) else {
            continue;
        };
        if Some(name) == tag {
            continue;
        }
        push_def_with_decorators(
            pending,
            name.to_string(),
            "type",
            definition,
            declaration_signature(definition, source),
            parent_index,
            None,
        );
    }
}

/// The declared name inside a typedef declarator: `NodeRef` in `*NodeRef`,
/// `Handler` in `(*Handler)(int)`, `Row` in `Row[4]`.
fn typedef_name<'a>(declarator: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    if declarator.kind() == "type_identifier" {
        return node_text(&declarator, source);
    }
    if let Some(inner) = declarator.child_by_field_name("declarator") {
        return typedef_name(inner, source);
    }
    let mut cursor = declarator.walk();
    let found = declarator
        .named_children(&mut cursor)
        .find_map(|child| typedef_name(child, source));
    found
}

fn emit_namespace_alias(
    decl: &Node,
    source: &[u8],
    pending: &mut PendingFile,
    module_index: usize,
) {
    let Some(alias) = name_field(decl, source) else {
        return;
    };
    let mut cursor = decl.walk();
    let target = decl
        .named_children(&mut cursor)
        .filter(|child| {
            matches!(
                child.kind(),
                "namespace_identifier" | "nested_namespace_specifier"
            )
        })
        .last()
        .filter(|target| {
            target.start_byte()
                > decl
                    .child_by_field_name("name")
                    .map_or(0, |n| n.start_byte())
        })
        .and_then(|target| node_text(&target, source));
    if let Some(target) = target {
        push_import(
            pending,
            module_index,
            alias.to_string(),
            Some(format!("{target}::*")),
            line_of(decl),
        );
    }
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

/// `using std::vector;` → leaf `vector`, path `std::vector`. `using namespace
/// std;` introduces every name in `std` — emit one edge with name `*` so
/// `imported_by std` can find it.
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

fn declaration_signature(node: &Node, source: &[u8]) -> Option<String> {
    let text = node_text(node, source)?;
    let trimmed = text.trim().trim_end_matches(';').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::common;
    use crate::indexer::parse_one;

    #[test]
    fn extracts_class_and_method_definitions() {
        let path = common::write_tmp(
            "cpp",
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
        // Out-of-class `app::Foo::baz` recognized as method via receiver.
        assert_eq!(by_name["baz"], "method");
    }

    #[test]
    fn extracts_function_and_method_declarations_from_headers() {
        let path = common::write_tmp(
            "cpp",
            "Provider.h",
            "class Provider {\n\
             public:\n\
                 virtual void DoWork(int value) = 0;\n\
                 int Ready() const;\n\
             };\n\
             void FreeFunction(int value);\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CppExtractor).unwrap();
        let by_name: std::collections::HashMap<&str, &str> = pending
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.kind.as_str()))
            .collect();

        assert_eq!(by_name["DoWork"], "method");
        assert_eq!(by_name["Ready"], "method");
        assert_eq!(by_name["FreeFunction"], "function");
    }

    #[test]
    fn extracts_every_function_declarator_from_a_single_declaration() {
        let path = common::write_tmp(
            "cpp",
            "MultipleDeclarators.h",
            "class Provider {\n\
             public:\n\
                 void Alpha(), Beta();\n\
             };\n\
             void First(), Second();\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CppExtractor).unwrap();
        let by_name: std::collections::HashMap<&str, &str> = pending
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.kind.as_str()))
            .collect();

        assert_eq!(by_name["Alpha"], "method");
        assert_eq!(by_name["Beta"], "method");
        assert_eq!(by_name["First"], "function");
        assert_eq!(by_name["Second"], "function");
    }

    #[test]
    fn does_not_treat_local_direct_initializers_as_function_declarations() {
        let path = common::write_tmp(
            "cpp",
            "LocalInitializers.cpp",
            "void Run(Path run_dir, Pointer pin) {\n\
                 for (DirectoryIterator end, it(run_dir); it != end; ++it) {}\n\
                 Byte *first(pin), *last(pin + 1);\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CppExtractor).unwrap();
        let names: Vec<&str> = pending
            .symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect();

        assert!(names.contains(&"Run"));
        assert!(!names.contains(&"it"));
        assert!(!names.contains(&"first"));
        assert!(!names.contains(&"last"));
    }

    #[test]
    fn extracts_struct_union_enum() {
        let path = common::write_tmp(
            "cpp",
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
    fn namespaces_own_their_members_and_aliases_become_types() {
        let path = common::write_tmp(
            "cpp",
            "ledger.hpp",
            "#pragma once\n\
             #include <cstdint>\n\
             namespace fs = std::filesystem;\n\
             namespace billing {\n\
             using Cents = std::int64_t;\n\
             typedef struct Entry { Cents amount; } Entry;\n\
             typedef void (*Listener)(const Entry&);\n\
             namespace tax {\n\
             Cents rate(const char* region);\n\
             }\n\
             class Ledger {\n\
             public:\n\
             \x20 using Row = Entry;\n\
             \x20 void post(const Entry& entry);\n\
             };\n\
             namespace {\n\
             Cents round(Cents value) { return value; }\n\
             }\n\
             }\n\
             namespace billing::export_ {\n\
             void write(const fs::path& target);\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &CppExtractor).unwrap();
        let find = |name: &str, kind: &str| {
            pending
                .symbols
                .iter()
                .find(|s| s.name == name && s.kind == kind)
                .unwrap_or_else(|| panic!("no {kind} {name} in {:#?}", pending.symbols))
        };
        let parent = |symbol: &crate::store::PendingSymbol| {
            pending.symbols[symbol.parent_index.unwrap()].name.as_str()
        };

        let billing = find("billing", "namespace");
        assert_eq!((billing.line_start, billing.line_end), (4, 19));
        assert_eq!(parent(find("tax", "namespace")), "billing");
        assert_eq!(parent(find("rate", "function")), "tax");
        assert_eq!(parent(find("Ledger", "class")), "billing");
        assert_eq!(parent(find("post", "method")), "Ledger");
        assert_eq!(parent(find("round", "function")), "billing");
        assert_eq!(parent(find("write", "function")), "billing::export_");
        assert_eq!(
            pending
                .symbols
                .iter()
                .filter(|s| s.kind == "namespace")
                .count(),
            3,
            "the anonymous namespace is not a symbol"
        );

        let cents = find("Cents", "type");
        assert_eq!(
            cents.signature.as_deref(),
            Some("using Cents = std::int64_t")
        );
        assert_eq!(parent(cents), "billing");
        assert_eq!(parent(find("Listener", "type")), "billing");
        assert_eq!(parent(find("Row", "type")), "Ledger");
        find("Entry", "struct");
        assert!(
            !pending
                .symbols
                .iter()
                .any(|s| s.name == "Entry" && s.kind == "type"),
            "a typedef of the struct it defines adds no second row"
        );

        assert!(pending.edges.iter().any(|e| e.kind == "imports"
            && e.to_name == "fs"
            && e.to_path.as_deref() == Some("std::filesystem::*")));
    }

    #[test]
    fn extracts_includes_and_using() {
        let path = common::write_tmp(
            "cpp",
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
        let path = common::write_tmp(
            "cpp",
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
