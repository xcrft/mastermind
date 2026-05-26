//! Java extractor — classes/interfaces/enums/records, methods, constructors,
//! annotations (captured as decorators), imports.

use super::common::{
    line_of, node_text, push_call_with_type, push_def_with_decorators, push_import,
};
use super::LanguageExtractor;
use crate::store::PendingFile;
use tree_sitter::{Node, Tree};

pub struct JavaExtractor;

impl LanguageExtractor for JavaExtractor {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_java::LANGUAGE.into()
    }

    fn name(&self) -> &'static str {
        "java"
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
            "class_declaration" | "record_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let decorators = collect_annotations(&child, source);
                let idx = push_def_with_decorators(
                    pending,
                    name,
                    "class",
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
                let decorators = collect_annotations(&child, source);
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
                let decorators = collect_annotations(&child, source);
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
            "method_declaration" | "compact_constructor_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let decorators = collect_annotations(&child, source);
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
            "constructor_declaration" => {
                let name = name_field(&child, source).unwrap_or("<ctor>").to_string();
                let signature = signature_until_body(&child, source);
                let decorators = collect_annotations(&child, source);
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
            "import_declaration" => {
                collect_import(&child, source, pending, module_index);
            }
            "method_invocation" => {
                let name = child
                    .child_by_field_name("name")
                    .and_then(|n| node_text(&n, source))
                    .map(String::from);
                let object = child
                    .child_by_field_name("object")
                    .and_then(|n| node_text(&n, source))
                    .map(String::from);
                if let Some(n) = name {
                    let full = match object.as_deref() {
                        Some(o) => Some(format!("{o}.{n}")),
                        None => Some(n.clone()),
                    };
                    // Uppercase receiver = type/Class.staticMethod
                    let to_type = object
                        .as_deref()
                        .and_then(|o| o.rsplit('.').next().map(str::to_string))
                        .filter(|s| starts_uppercase(s));
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
                // `new Foo(...)` → call to "Foo".
                if let Some(t) = child.child_by_field_name("type") {
                    let name = node_text(&t, source).and_then(|s| {
                        // Strip generic suffix `Foo<T>` and qualifier `pkg.Foo`.
                        let base = s.split('<').next().unwrap_or(s);
                        base.rsplit('.').next().map(str::to_string)
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

fn starts_uppercase(s: &str) -> bool {
    s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

/// `import java.util.List;` → name=`List`, path=`java.util.List`.
/// `import static java.lang.Math.PI;` → name=`PI`, path=`java.lang.Math.PI`.
/// `import java.util.*;` → name=`*`, path=`java.util::*`.
fn collect_import(decl: &Node, source: &[u8], pending: &mut PendingFile, module_index: usize) {
    let line = line_of(decl);
    let Some(text) = node_text(decl, source) else {
        return;
    };
    let raw = text.trim().trim_end_matches(';').trim();
    let after_import = raw.strip_prefix("import").unwrap_or(raw).trim_start();
    let stripped = after_import
        .strip_prefix("static ")
        .map(str::trim_start)
        .unwrap_or(after_import);
    if stripped.is_empty() {
        return;
    }
    let (leaf, to_path) = if stripped.ends_with(".*") {
        let pkg = stripped.trim_end_matches(".*");
        ("*".to_string(), Some(format!("{pkg}::*")))
    } else {
        let leaf = stripped.rsplit('.').next().unwrap_or(stripped).to_string();
        (leaf, Some(stripped.to_string()))
    };
    push_import(pending, module_index, leaf, to_path, line);
}

/// Capture all `@Annotation` / `@Marker` names attached to a declaration.
/// Stripped to leaf name (`@org.junit.Test` → `Test`). Returned in the
/// `,Name1,Name2,` format used by `mmcg_unreferenced` filtering.
fn collect_annotations(decl: &Node, source: &[u8]) -> Option<String> {
    let mut names: Vec<String> = Vec::new();
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        if !matches!(child.kind(), "modifiers") {
            continue;
        }
        let mut mc = child.walk();
        for m in child.children(&mut mc) {
            match m.kind() {
                "annotation" | "marker_annotation" => {
                    if let Some(name_node) = m.child_by_field_name("name") {
                        let raw = node_text(&name_node, source).unwrap_or("").trim();
                        if !raw.is_empty() {
                            let leaf = raw.rsplit('.').next().unwrap_or(raw);
                            names.push(leaf.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if names.is_empty() {
        None
    } else {
        Some(format!(",{},", names.join(",")))
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
        dir.push(format!("mmcg-java-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn extracts_class_method_and_constructor() {
        let path = write_tmp(
            "Foo.java",
            "package app;\n\
             public class Foo {\n\
                 public Foo() {}\n\
                 public int bar(int x) { return x + 1; }\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &JavaExtractor).unwrap();
        let names: Vec<&str> = pending.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"bar"));
        // Constructor name = class name
        let constructors: usize = pending
            .symbols
            .iter()
            .filter(|s| s.kind == "method" && s.name == "Foo")
            .count();
        assert!(constructors >= 1, "expected at least one Foo constructor");
    }

    #[test]
    fn extracts_imports() {
        let path = write_tmp(
            "Imports.java",
            "package app;\n\
             import java.util.List;\n\
             import java.util.*;\n\
             import static java.lang.Math.PI;\n\
             class X {}\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &JavaExtractor).unwrap();
        let imports: Vec<(&str, Option<&str>)> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "imports")
            .map(|e| (e.to_name.as_str(), e.to_path.as_deref()))
            .collect();
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "List" && *p == Some("java.util.List")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "*" && *p == Some("java.util::*")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "PI" && *p == Some("java.lang.Math.PI")));
    }

    #[test]
    fn captures_annotations_into_decorators() {
        let path = write_tmp(
            "Tests.java",
            "package app;\n\
             import org.junit.jupiter.api.Test;\n\
             public class FooTests {\n\
                 @Test public void shouldDoX() {}\n\
                 @Override public String toString() { return \"\"; }\n\
                 @org.junit.jupiter.api.ParameterizedTest public void params(int n) {}\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &JavaExtractor).unwrap();
        let by_name: std::collections::HashMap<&str, &Option<String>> = pending
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), &s.decorators))
            .collect();
        assert_eq!(by_name["shouldDoX"].as_deref(), Some(",Test,"));
        assert_eq!(by_name["toString"].as_deref(), Some(",Override,"));
        assert_eq!(by_name["params"].as_deref(), Some(",ParameterizedTest,"));
    }

    #[test]
    fn extracts_calls_and_new() {
        let path = write_tmp(
            "Calls.java",
            "package app;\n\
             public class Main {\n\
                 public void run() {\n\
                     System.out.println(\"hi\");\n\
                     var list = new java.util.ArrayList<Integer>();\n\
                     helper();\n\
                 }\n\
                 void helper() {}\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &JavaExtractor).unwrap();
        let calls: Vec<&str> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| e.to_name.as_str())
            .collect();
        assert!(calls.contains(&"println"));
        assert!(calls.contains(&"helper"));
        // `new java.util.ArrayList<Integer>()` → call to "ArrayList"
        assert!(calls.contains(&"ArrayList"));
    }
}
