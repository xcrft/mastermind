//! Vue single-file components — the file is the component.
//!
//! A `.vue` file is structurally HTML, so the SFC shell is parsed with the
//! tree-sitter HTML grammar: `<template>` becomes an element tree and
//! `<script>` becomes one `raw_text` node. The script is then re-parsed with
//! the real TypeScript or JavaScript grammar and handed to the shared TS walker,
//! so a Vue component gets the same symbols and call edges as a `.ts` file
//! rather than a weaker approximation of them.

use super::common::{node_text, push_call_with_type};
use super::LanguageExtractor;
use crate::store::PendingFile;
use tree_sitter::{Node, Parser, Tree};

pub struct VueExtractor;

impl LanguageExtractor for VueExtractor {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_html::LANGUAGE.into()
    }

    fn name(&self) -> &'static str {
        "vue"
    }

    fn extract(&self, tree: &Tree, source: &[u8], pending: &mut PendingFile, module_index: usize) {
        let component_index = push_component_symbol(pending, module_index);
        let root = tree.root_node();
        extract_script(&root, source, pending, component_index, module_index);
        extract_template(&root, source, pending, component_index);
    }
}

/// Vue resolves `<my-widget />`, `<MyWidget />`, and `my-widget.vue` to the same
/// component. The graph matches by name, so both the defining symbol and every
/// template usage are normalized to the PascalCase form or they would never meet.
pub(super) fn pascal_case(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut capitalize = true;
    for ch in raw.chars() {
        if ch == '-' || ch == '_' {
            capitalize = true;
            continue;
        }
        if capitalize {
            out.extend(ch.to_uppercase());
            capitalize = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn push_component_symbol(pending: &mut PendingFile, module_index: usize) -> usize {
    let stem = pending
        .path
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".vue"))
        .unwrap_or("");
    let name = pascal_case(stem);
    if name.is_empty() {
        return module_index;
    }
    let module = &pending.symbols[module_index];
    let (line_start, line_end) = (module.line_start, module.line_end);
    let signature = Some(format!("component {name}"));
    pending.symbols.push(crate::store::PendingSymbol {
        name,
        kind: "component".to_string(),
        line_start,
        line_end,
        signature,
        parent_index: Some(module_index),
        decorators: None,
    });
    pending.symbols.len() - 1
}

/// Re-parse the `<script>` body with its real grammar. The script text is copied
/// back into a blanked buffer of the same length so every reported line still
/// points at the `.vue` file rather than at an offset inside the block.
fn extract_script(
    root: &Node,
    source: &[u8],
    pending: &mut PendingFile,
    parent_index: usize,
    module_index: usize,
) {
    let Some(script) = find_first(root, "script_element") else {
        return;
    };
    let Some(body) = find_first(&script, "raw_text") else {
        return;
    };

    let mut isolated = vec![b' '; source.len()];
    for (offset, byte) in source.iter().enumerate() {
        if *byte == b'\n' {
            isolated[offset] = b'\n';
        }
    }
    let (start, end) = (body.start_byte(), body.end_byte());
    if end > source.len() || start >= end {
        return;
    }
    isolated[start..end].copy_from_slice(&source[start..end]);

    let language = if script_is_typescript(&script, source) {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    } else {
        tree_sitter_javascript::LANGUAGE.into()
    };
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return;
    }
    let Some(script_tree) = parser.parse(&isolated, None) else {
        return;
    };
    super::typescript::walk(
        script_tree.root_node(),
        &isolated,
        pending,
        Some(parent_index),
        module_index,
    );
}

fn script_is_typescript(script: &Node, source: &[u8]) -> bool {
    let Some(start_tag) = find_first(script, "start_tag") else {
        return false;
    };
    let mut cursor = start_tag.walk();
    for attribute in start_tag.children(&mut cursor) {
        if attribute.kind() != "attribute" {
            continue;
        }
        let name = find_first(&attribute, "attribute_name")
            .and_then(|n| node_text(&n, source))
            .unwrap_or("");
        if name != "lang" {
            continue;
        }
        let value = find_first(&attribute, "attribute_value")
            .and_then(|n| node_text(&n, source))
            .unwrap_or("");
        return value.starts_with("ts");
    }
    false
}

fn extract_template(root: &Node, source: &[u8], pending: &mut PendingFile, parent_index: usize) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "element" && element_tag_name(&child, source) == Some("template") {
            collect_component_tags(&child, source, pending, parent_index);
        }
    }
}

fn collect_component_tags(
    node: &Node,
    source: &[u8],
    pending: &mut PendingFile,
    parent_index: usize,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "start_tag" | "self_closing_tag") {
            if let Some(tag) = find_first(&child, "tag_name").and_then(|n| node_text(&n, source)) {
                if is_component_tag(tag) {
                    push_call_with_type(
                        pending,
                        parent_index,
                        pascal_case(tag),
                        Some(tag.to_string()),
                        None,
                        (child.start_position().row + 1) as u32,
                    );
                }
            }
        }
        collect_component_tags(&child, source, pending, parent_index);
    }
}

/// A custom element always contains a hyphen and a native HTML tag never does,
/// so kebab-case usage is unambiguous. PascalCase covers the other convention.
fn is_component_tag(tag: &str) -> bool {
    if tag.contains('-') {
        return true;
    }
    tag.chars().next().map(char::is_uppercase).unwrap_or(false)
}

fn element_tag_name<'a>(element: &Node, source: &'a [u8]) -> Option<&'a str> {
    let opening =
        find_first(element, "start_tag").or_else(|| find_first(element, "self_closing_tag"))?;
    find_first(&opening, "tag_name").and_then(|n| node_text(&n, source))
}

#[allow(clippy::manual_find)]
fn find_first<'tree>(node: &Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parse_one;
    use std::env;
    use std::path::PathBuf;

    fn write_tmp(name: &str, content: &str) -> PathBuf {
        let mut dir = env::temp_dir();
        dir.push(format!("mmcg-vue-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn parse(name: &str, content: &str) -> PendingFile {
        let path = write_tmp(name, content);
        let root = path.parent().unwrap();
        parse_one(&path, root, &VueExtractor).unwrap()
    }

    #[test]
    fn pascal_case_normalizes_both_conventions() {
        assert_eq!(pascal_case("my-widget"), "MyWidget");
        assert_eq!(pascal_case("MyWidget"), "MyWidget");
        assert_eq!(pascal_case("base_button"), "BaseButton");
        assert_eq!(pascal_case(""), "");
    }

    #[test]
    fn file_becomes_a_component_symbol_owning_its_script() {
        let pending = parse(
            "MyCard.vue",
            "<template>\n  <div />\n</template>\n\n\
             <script setup lang=\"ts\">\n\
             const label: string = 'x';\n\
             function bump(step: number): void { void step; }\n\
             </script>\n",
        );
        let component = pending
            .symbols
            .iter()
            .find(|s| s.kind == "component")
            .expect("component symbol");
        assert_eq!(component.name, "MyCard");

        let bump = pending
            .symbols
            .iter()
            .find(|s| s.name == "bump")
            .expect("script symbol");
        // Line 7 of the .vue file, not line 3 of the script block.
        assert_eq!(bump.line_start, 7);
        assert_eq!(
            bump.signature.as_deref(),
            Some("function bump(step: number): void")
        );
    }

    #[test]
    fn template_component_usage_becomes_a_call_edge_and_host_tags_do_not() {
        let pending = parse(
            "Screen.vue",
            "<template>\n\
               <div>\n\
                 <BaseButton />\n\
                 <my-widget />\n\
                 <span>text</span>\n\
               </div>\n\
             </template>\n",
        );
        let calls: Vec<&str> = pending
            .edges
            .iter()
            .filter(|e| e.kind == "calls")
            .map(|e| e.to_name.as_str())
            .collect();
        assert!(calls.contains(&"BaseButton"));
        assert!(calls.contains(&"MyWidget"));
        assert!(!calls.contains(&"div"));
        assert!(!calls.contains(&"span"));
        assert!(!calls.contains(&"template"));
    }

    #[test]
    fn plain_javascript_script_block_still_parses() {
        let pending = parse(
            "Legacy.vue",
            "<template><div /></template>\n\
             <script>\n\
             export default { methods: { go() { this.$router.push('/'); } } };\n\
             </script>\n",
        );
        assert!(pending.symbols.iter().any(|s| s.name == "go"));
    }

    #[test]
    fn style_block_contributes_nothing() {
        let pending = parse(
            "Styled.vue",
            "<template><div /></template>\n<style scoped>.a { color: red }</style>\n",
        );
        assert!(pending.edges.iter().all(|e| e.to_name != "a"));
        assert_eq!(
            pending
                .symbols
                .iter()
                .filter(|s| s.kind == "component")
                .count(),
            1
        );
    }
}
