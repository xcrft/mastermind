//! PHP extractor — namespaces, classes/interfaces/traits/enums, methods,
//! functions, calls, PHP 8 attributes (`#[Test]`), `use` directives.

use super::common::{
    line_of, node_text, push_call_with_type, push_def_with_decorators, push_import,
};
use super::LanguageExtractor;
use crate::store::PendingFile;
use tree_sitter::{Node, Tree};

pub struct PhpExtractor;

impl LanguageExtractor for PhpExtractor {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_php::LANGUAGE_PHP.into()
    }

    fn name(&self) -> &'static str {
        "php"
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
            // Namespaces are transparent — walk bodies with same parent context.
            // Either `namespace X { ... }` (with body) or `namespace X;`
            // (siblings become module-level).
            "namespace_definition" => {
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, parent_index, module_index);
                }
            }
            "class_declaration" | "trait_declaration" => {
                let kind = if child.kind() == "trait_declaration" {
                    "trait"
                } else {
                    "class"
                };
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
                let decorators = collect_attributes(&child, source);
                let idx = push_def_with_decorators(
                    pending,
                    name,
                    "enum",
                    &child,
                    signature,
                    parent_index,
                    decorators,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "method_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
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
            "function_definition" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let decorators = collect_attributes(&child, source);
                let idx = push_def_with_decorators(
                    pending,
                    name,
                    "function",
                    &child,
                    signature,
                    parent_index,
                    decorators,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "namespace_use_declaration" => {
                collect_use(&child, source, pending, module_index);
            }
            "function_call_expression" => {
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
            "member_call_expression" => {
                // `$obj->method(...)`
                let name = child
                    .child_by_field_name("name")
                    .and_then(|n| node_text(&n, source))
                    .map(String::from);
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
                walk(child, source, pending, parent_index, module_index);
            }
            "scoped_call_expression" => {
                // `Foo::bar(...)` / `static::bar(...)` / `self::bar(...)`
                let name = child
                    .child_by_field_name("name")
                    .and_then(|n| node_text(&n, source))
                    .map(String::from);
                let scope = child
                    .child_by_field_name("scope")
                    .and_then(|n| node_text(&n, source))
                    .map(String::from);
                if let Some(n) = name {
                    let full = match scope.as_deref() {
                        Some(s) => Some(format!("{s}::{n}")),
                        None => Some(n.clone()),
                    };
                    let to_type = scope.filter(|s| starts_uppercase(s));
                    push_call_with_type(
                        pending,
                        parent_index.unwrap_or(module_index),
                        n,
                        full,
                        to_type,
                        line_of(&child),
                    );
                }
                walk(child, source, pending, parent_index, module_index);
            }
            "object_creation_expression" => {
                // `new Foo(...)` — first child after `new` is the class name expr.
                let mut oc = child.walk();
                for sub in child.children(&mut oc) {
                    let leaf = match sub.kind() {
                        "name" => node_text(&sub, source).map(String::from),
                        "qualified_name" => node_text(&sub, source)
                            .and_then(|s| s.rsplit('\\').next().map(str::to_string)),
                        _ => None,
                    };
                    if let Some(n) = leaf {
                        push_call_with_type(
                            pending,
                            parent_index.unwrap_or(module_index),
                            n.clone(),
                            Some(n),
                            None,
                            line_of(&child),
                        );
                        break;
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

fn starts_uppercase(s: &str) -> bool {
    s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

/// `use Foo\Bar;` / `use Foo\Bar as Baz;` / `use Foo\{A, B as C};`.
fn collect_use(decl: &Node, source: &[u8], pending: &mut PendingFile, module_index: usize) {
    let line = line_of(decl);
    let Some(text) = node_text(decl, source) else {
        return;
    };
    let raw = text.trim().trim_end_matches(';').trim();
    let after_use = raw.strip_prefix("use").unwrap_or(raw).trim_start();
    // Skip optional `function ` / `const `.
    let body = after_use
        .strip_prefix("function ")
        .or_else(|| after_use.strip_prefix("const "))
        .unwrap_or(after_use);

    // Grouped form: `Foo\Bar\{A, B as C}`.
    if let Some(brace) = body.find('{') {
        let prefix = body[..brace].trim().trim_end_matches('\\').trim();
        let inside = body[brace + 1..].trim_end_matches('}').trim();
        for item in inside.split(',') {
            emit_use_item(item.trim(), Some(prefix), pending, module_index, line);
        }
    } else {
        emit_use_item(body, None, pending, module_index, line);
    }
}

fn emit_use_item(
    item: &str,
    prefix: Option<&str>,
    pending: &mut PendingFile,
    module_index: usize,
    line: u32,
) {
    if item.is_empty() {
        return;
    }
    let (raw_path, alias) = if let Some((p, a)) = item.split_once(" as ") {
        (p.trim(), Some(a.trim()))
    } else {
        (item, None)
    };
    let full_path = match prefix {
        Some(p) => format!("{p}\\{raw_path}"),
        None => raw_path.to_string(),
    };
    let leaf = alias.map(str::to_string).unwrap_or_else(|| {
        full_path
            .rsplit('\\')
            .next()
            .unwrap_or(&full_path)
            .to_string()
    });
    push_import(pending, module_index, leaf, Some(full_path), line);
}

fn leaf_and_path(node: &Node, source: &[u8]) -> Option<(String, Option<String>, Option<String>)> {
    let full = node_text(node, source).map(String::from);
    match node.kind() {
        "name" => {
            let n = node_text(node, source)?.to_string();
            Some((n, full, None))
        }
        "qualified_name" => {
            let raw = node_text(node, source)?;
            let leaf = raw.rsplit('\\').next().unwrap_or(raw).to_string();
            let to_type = raw
                .rsplit_once('\\')
                .map(|(prefix, _)| prefix.rsplit('\\').next().unwrap_or(prefix).to_string())
                .filter(|s| starts_uppercase(s));
            Some((leaf, full, to_type))
        }
        _ => None,
    }
}

/// Collect PHP 8 attribute names from `#[Foo]` / `#[Foo, Bar(args)]` /
/// `#[\\Ns\\Foo]`. `attribute_list` → `attribute_group` → `attribute` nodes,
/// each with a name child.
fn collect_attributes(decl: &Node, source: &[u8]) -> Option<String> {
    let attrs = decl.child_by_field_name("attributes")?;
    let mut names: Vec<String> = Vec::new();
    collect_attr_names(&attrs, source, &mut names);
    if names.is_empty() {
        None
    } else {
        Some(format!(",{},", names.join(",")))
    }
}

fn collect_attr_names(node: &Node, source: &[u8], out: &mut Vec<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "attribute_group" => collect_attr_names(&child, source, out),
            "attribute" => {
                // First named child is the name (qualified_name or name).
                if let Some(name_node) = first_named_child(&child) {
                    let raw = node_text(&name_node, source).unwrap_or("").trim();
                    let leaf = raw.rsplit('\\').next().unwrap_or(raw);
                    let cleaned = leaf.trim_start_matches('\\');
                    if !cleaned.is_empty() {
                        out.push(cleaned.to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

// Same lifetime constraint as `find_child_of_kind` in typescript.rs: `.find()`
// would drop the cursor before the borrow ends.
#[allow(clippy::manual_find)]
fn first_named_child<'tree>(node: &Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
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
        dir.push(format!("mmcg-php-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn extracts_classes_traits_and_methods() {
        let path = write_tmp(
            "Foo.php",
            "<?php\nnamespace App;\n\
             class Foo {\n\
                 public function bar(int $x): int { return $x + 1; }\n\
             }\n\
             interface IRunner { public function run(): void; }\n\
             trait Helper { public function helped(): void {} }\n\
             function topLevel() {}\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &PhpExtractor).unwrap();
        let by_name: std::collections::HashMap<&str, &str> = pending
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind.as_str()))
            .collect();
        assert_eq!(by_name["Foo"], "class");
        assert_eq!(by_name["IRunner"], "interface");
        assert_eq!(by_name["Helper"], "trait");
        assert_eq!(by_name["bar"], "method");
        assert_eq!(by_name["topLevel"], "function");
    }

    #[test]
    fn extracts_use_declarations() {
        let path = write_tmp(
            "Usings.php",
            "<?php\n\
             use App\\Foo;\n\
             use App\\Bar as Baz;\n\
             use App\\Sub\\{A, B as C};\n\
             class X {}\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &PhpExtractor).unwrap();
        let imports: Vec<(&str, Option<&str>)> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "imports")
            .map(|e| (e.to_name.as_str(), e.to_path.as_deref()))
            .collect();
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "Foo" && *p == Some("App\\Foo")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "Baz" && *p == Some("App\\Bar")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "A" && *p == Some("App\\Sub\\A")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "C" && *p == Some("App\\Sub\\B")));
    }

    #[test]
    fn captures_php8_attributes() {
        let path = write_tmp(
            "Attrs.php",
            "<?php\n\
             use PHPUnit\\Framework\\Attributes\\Test;\n\
             class FooTests {\n\
                 #[Test] public function shouldDoX(): void {}\n\
                 #[\\Symfony\\Component\\Routing\\Attribute\\Route('/api')] public function endpoint(): void {}\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &PhpExtractor).unwrap();
        let by_name: std::collections::HashMap<&str, &Option<String>> = pending
            .symbols
            .iter()
            .filter(|s| s.kind == "method")
            .map(|s| (s.name.as_str(), &s.decorators))
            .collect();
        assert_eq!(by_name["shouldDoX"].as_deref(), Some(",Test,"));
        assert_eq!(by_name["endpoint"].as_deref(), Some(",Route,"));
    }

    #[test]
    fn extracts_calls_static_and_new() {
        let path = write_tmp(
            "Calls.php",
            "<?php\n\
             class Main {\n\
                 public function run(): void {\n\
                     Logger::info('hi');\n\
                     $x = new Foo();\n\
                     $x->bar();\n\
                     helper();\n\
                 }\n\
             }\n\
             function helper(): void {}\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &PhpExtractor).unwrap();
        let calls: Vec<(&str, Option<&str>)> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| (e.to_name.as_str(), e.to_type.as_deref()))
            .collect();
        assert!(calls
            .iter()
            .any(|(n, t)| *n == "info" && *t == Some("Logger")));
        assert!(calls.iter().any(|(n, _)| *n == "Foo"));
        assert!(calls.iter().any(|(n, _)| *n == "bar"));
        assert!(calls.iter().any(|(n, _)| *n == "helper"));
    }
}
