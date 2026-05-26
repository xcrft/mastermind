//! Go extractor — functions, methods, types (struct/interface), imports, calls.

use super::common::{
    line_of, node_text, push_call_with_type, push_def, push_def_with_decorators, push_import,
};
use super::LanguageExtractor;
use crate::store::PendingFile;
use tree_sitter::{Node, Tree};

pub struct GoExtractor;

impl LanguageExtractor for GoExtractor {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn name(&self) -> &'static str {
        "go"
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
            "function_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                let idx = push_def(pending, name, "function", &child, signature, parent_index);
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "method_declaration" => {
                let name = name_field(&child, source).unwrap_or("<anon>").to_string();
                let signature = signature_until_body(&child, source);
                // Go methods are top-level (not nested in a type body), so parent_index
                // remains the module — but we capture the receiver type as the parent
                // semantically through the symbol's own naming. For mmcg's purposes,
                // emitting `kind=method` plus the literal receiver in the signature is
                // enough; callers query by leaf name `Method` and match the receiver
                // via `to_type` on the call site.
                let idx = push_def(pending, name, "method", &child, signature, parent_index);
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, source, pending, Some(idx), module_index);
                }
            }
            "type_declaration" => {
                // `type Foo struct {...}` / `type Foo interface {...}` / `type Foo Other`.
                // Wraps one or more `type_spec` nodes.
                let mut tc = child.walk();
                for spec in child.children(&mut tc) {
                    if spec.kind() != "type_spec" {
                        continue;
                    }
                    let name = spec
                        .child_by_field_name("name")
                        .and_then(|n| node_text(&n, source))
                        .unwrap_or("<anon>")
                        .to_string();
                    let type_node = spec.child_by_field_name("type");
                    let kind = match type_node.map(|n| n.kind()) {
                        Some("struct_type") => "struct",
                        Some("interface_type") => "interface",
                        _ => "type",
                    };
                    let signature = node_text(&spec, source).map(|s| s.trim().to_string());
                    let idx = push_def(pending, name, kind, &spec, signature, parent_index);
                    if let Some(t) = type_node {
                        walk(t, source, pending, Some(idx), module_index);
                    }
                }
            }
            "import_declaration" => {
                collect_imports(&child, source, pending, module_index);
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
            "composite_literal" => {
                // `Foo{...}` — treat as construction call to type Foo.
                if let Some(t) = child.child_by_field_name("type") {
                    if let Some((name, path, _)) = leaf_and_path(&t, source) {
                        push_call_with_type(
                            pending,
                            parent_index.unwrap_or(module_index),
                            name,
                            path,
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

/// `import "fmt"` / `import f "fmt"` / `import ( a "x"; b "y" )`.
fn collect_imports(decl: &Node, source: &[u8], pending: &mut PendingFile, module_index: usize) {
    let line = line_of(decl);
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        match child.kind() {
            "import_spec" => emit_import_spec(&child, source, pending, module_index, line),
            "import_spec_list" => {
                let mut lc = child.walk();
                for spec in child.children(&mut lc) {
                    if spec.kind() == "import_spec" {
                        emit_import_spec(&spec, source, pending, module_index, line);
                    }
                }
            }
            _ => {}
        }
    }
}

fn emit_import_spec(
    spec: &Node,
    source: &[u8],
    pending: &mut PendingFile,
    module_index: usize,
    line: u32,
) {
    let path_lit = spec
        .child_by_field_name("path")
        .and_then(|n| node_text(&n, source))
        .map(|s| s.trim_matches('"').to_string());
    let Some(path) = path_lit else { return };
    if path.is_empty() {
        return;
    }
    // Leaf: explicit alias if present, else last `/` segment of the path.
    let leaf = spec
        .child_by_field_name("name")
        .and_then(|n| node_text(&n, source))
        .filter(|n| *n != "_" && *n != ".")
        .map(str::to_string)
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path).to_string());
    let to_path = Some(format!("{path}::*"));
    push_import(pending, module_index, leaf, to_path, line);
}

/// `(leaf, full_text, to_type)` for a call target. `pkg.Func` → ("Func", "pkg.Func", Some("pkg"))
/// when receiver starts uppercase; otherwise to_type=None. Plain identifier → just the name.
fn leaf_and_path(node: &Node, source: &[u8]) -> Option<(String, Option<String>, Option<String>)> {
    let full = node_text(node, source).map(String::from);
    match node.kind() {
        "identifier" | "type_identifier" => {
            let n = node_text(node, source)?.to_string();
            Some((n, full, None))
        }
        "selector_expression" => {
            let field = node
                .child_by_field_name("field")
                .and_then(|n| node_text(&n, source))?
                .to_string();
            let operand = node
                .child_by_field_name("operand")
                .and_then(|n| node_text(&n, source))
                .and_then(|s| s.rsplit('.').next().map(str::to_string));
            // Uppercase receiver = exported type / package — useful as to_type hint.
            let to_type = operand.filter(|s| starts_uppercase(s));
            Some((field, full, to_type))
        }
        "qualified_type" => {
            // `pkg.TypeName` used as constructor in composite_literal.
            let name = node
                .child_by_field_name("name")
                .and_then(|n| node_text(&n, source))?
                .to_string();
            let package = node
                .child_by_field_name("package")
                .and_then(|n| node_text(&n, source))
                .map(String::from);
            let to_type = package.filter(|s| starts_uppercase(s));
            Some((name, full, to_type))
        }
        _ => None,
    }
}

fn starts_uppercase(s: &str) -> bool {
    s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
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

// Currently unused — Go has no decorator/attribute syntax. Kept for future
// build-tag / directive capture (`//go:build`, `//go:generate`).
#[allow(dead_code)]
fn push_def_with_build_tags(
    pending: &mut PendingFile,
    name: String,
    kind: &str,
    node: &Node,
    signature: Option<String>,
    parent_index: Option<usize>,
    tags: Option<String>,
) -> usize {
    push_def_with_decorators(pending, name, kind, node, signature, parent_index, tags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parse_one;
    use std::env;
    use std::path::PathBuf;

    fn write_tmp(name: &str, content: &str) -> PathBuf {
        let mut dir = env::temp_dir();
        dir.push(format!("mmcg-go-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn extracts_functions_methods_and_types() {
        let path = write_tmp(
            "main.go",
            "package main\n\
             type Server struct { addr string }\n\
             type Handler interface { Serve() }\n\
             func New(addr string) *Server { return &Server{addr: addr} }\n\
             func (s *Server) Run() error { return nil }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &GoExtractor).unwrap();
        let names: Vec<&str> = pending.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Server"));
        assert!(names.contains(&"Handler"));
        assert!(names.contains(&"New"));
        assert!(names.contains(&"Run"));
        let kinds: std::collections::HashMap<&str, &str> = pending
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind.as_str()))
            .collect();
        assert_eq!(kinds["Server"], "struct");
        assert_eq!(kinds["Handler"], "interface");
        assert_eq!(kinds["Run"], "method");
        assert_eq!(kinds["New"], "function");
    }

    #[test]
    fn extracts_imports_with_alias_and_grouped() {
        let path = write_tmp(
            "imports.go",
            "package main\n\
             import \"fmt\"\n\
             import f \"strings\"\n\
             import (\n\
                 \"net/http\"\n\
                 j \"encoding/json\"\n\
                 _ \"side/effect\"\n\
                 . \"dot/import\"\n\
             )\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &GoExtractor).unwrap();
        let imports: Vec<(&str, Option<&str>)> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "imports")
            .map(|e| (e.to_name.as_str(), e.to_path.as_deref()))
            .collect();
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "fmt" && *p == Some("fmt::*")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "f" && *p == Some("strings::*")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "http" && *p == Some("net/http::*")));
        assert!(imports
            .iter()
            .any(|(n, p)| *n == "j" && *p == Some("encoding/json::*")));
        // _ and . aliases fall back to path leaf
        assert!(imports.iter().any(|(n, _)| *n == "effect"));
        assert!(imports.iter().any(|(n, _)| *n == "import"));
    }

    #[test]
    fn extracts_calls_and_composite_literals() {
        let path = write_tmp(
            "calls.go",
            "package main\n\
             import \"fmt\"\n\
             func main() {\n\
                 fmt.Println(\"hi\")\n\
                 s := Server{addr: \":8080\"}\n\
                 s.Run()\n\
                 _ = s\n\
             }\n",
        );
        let root = path.parent().unwrap();
        let pending = parse_one(&path, root, &GoExtractor).unwrap();
        let calls: Vec<(&str, Option<&str>)> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| (e.to_name.as_str(), e.to_type.as_deref()))
            .collect();
        // Member call captured with receiver as to_type when uppercase
        assert!(calls
            .iter()
            .any(|(n, t)| *n == "Println" && (*t == Some("fmt") || t.is_none())));
        // Composite literal Server{...} → call to "Server"
        assert!(calls.iter().any(|(n, _)| *n == "Server"));
        // s.Run() → call to "Run" (s is lowercase so to_type=None)
        assert!(calls.iter().any(|(n, _)| *n == "Run"));
    }
}
