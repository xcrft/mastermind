//! Rust extractor — functions, structs, enums, traits, impls (with methods),
//! calls, macro invocations, and use declarations.

use super::common::{node_text, push_def, push_def_with_decorators, push_import};
use super::{DocumentationTextBuilder, LanguageExtractor, RawConceptDocumentation};
use crate::store::{PendingEdge, PendingFile};
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node, Parser, Tree};

pub struct RustExtractor;

impl LanguageExtractor for RustExtractor {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn name(&self) -> &'static str {
        "rust"
    }

    fn extract(&self, tree: &Tree, source: &[u8], pending: &mut PendingFile, module_index: usize) {
        let mut known_callables = HashSet::new();
        collect_callable_names(tree.root_node(), source, &mut known_callables);
        RustWalker {
            source,
            pending,
            module_index,
            known_callables: &known_callables,
            bindings: vec![HashSet::new()],
            item_bindings: vec![],
            references_only: false,
            line_offset: 0,
            macro_depth: 0,
            macro_budget: &mut MacroBudget::default(),
        }
        .visit(tree.root_node(), Some(module_index), None);
    }
}

// Macro tokens are opaque to the Rust grammar. Reparse only bounded, valid
// expression/statement bodies, and preserve their uncertainty as references.
const MACRO_BODY_BYTE_LIMIT: usize = 64 * 1024;
const MACRO_PARSE_BYTE_LIMIT: usize = 1024 * 1024;
const MACRO_PARSE_LIMIT: usize = 256;
const MACRO_DEPTH_LIMIT: u8 = 8;

struct MacroBudget {
    bytes: usize,
    parses: usize,
}

impl Default for MacroBudget {
    fn default() -> Self {
        Self {
            bytes: MACRO_PARSE_BYTE_LIMIT,
            parses: MACRO_PARSE_LIMIT,
        }
    }
}

struct RustWalker<'a, 'p> {
    source: &'a [u8],
    pending: &'p mut PendingFile,
    module_index: usize,
    known_callables: &'a HashSet<String>,
    bindings: Vec<HashSet<String>>,
    item_bindings: Vec<HashSet<String>>,
    references_only: bool,
    line_offset: u32,
    macro_depth: u8,
    macro_budget: &'p mut MacroBudget,
}

impl RustWalker<'_, '_> {
    fn line(&self, node: Node) -> u32 {
        self.line_offset
            .saturating_add(node.start_position().row as u32 + 1)
    }

    fn is_bound(&self, name: &str) -> bool {
        self.bindings
            .iter()
            .chain(&self.item_bindings)
            .any(|scope| scope.contains(name))
    }

    fn bind_pattern(&mut self, pattern: Node) {
        collect_pattern_bindings(pattern, self.source, self.bindings.last_mut().unwrap());
    }

    fn walk(&mut self, node: Node, parent_index: Option<usize>) {
        let mut cursor = node.walk();
        let mut pending_attrs = Vec::new();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "attribute_item" => pending_attrs.push(child),
                // Inner attributes belong to the enclosing item. Comments do
                // not break the association between outer attributes and an item.
                "inner_attribute_item" | "line_comment" | "block_comment" => {}
                _ => self.visit(
                    child,
                    parent_index,
                    take_attrs(&mut pending_attrs, self.source),
                ),
            }
        }
    }

    fn visit(
        &mut self,
        child: Node,
        parent_index: Option<usize>,
        attrs: Option<DeclarationAttributes>,
    ) {
        match child.kind() {
            "source_file" | "block" | "declaration_list" => {
                self.bindings.push(HashSet::new());
                // Item values are visible throughout the block, including inside
                // nested functions, unlike captured parameters and local variables.
                let mut item_values = HashSet::new();
                let mut cursor = child.walk();
                for item in child.named_children(&mut cursor) {
                    if matches!(item.kind(), "const_item" | "static_item") {
                        if let Some(name) = name_field(&item, self.source) {
                            item_values.insert(name.to_owned());
                        }
                    }
                }
                self.item_bindings.push(item_values);
                self.walk(child, parent_index);
                self.item_bindings.pop();
                self.bindings.pop();
            }
            "function_item" => {
                if self.references_only {
                    return; // A macro's item tokens do not establish a generated function.
                }
                let kind = match parent_index {
                    Some(p)
                        if matches!(self.pending.symbols[p].kind.as_str(), "impl" | "trait") =>
                    {
                        "method"
                    }
                    _ => "function",
                };
                let name = name_field(&child, self.source)
                    .unwrap_or("<anon>")
                    .to_string();
                let signature = signature_for_function(&child, self.source);
                let idx = push_def_or_decorated(
                    self.pending,
                    name,
                    kind,
                    &child,
                    signature,
                    parent_index,
                    attrs,
                );
                let saved_bindings = std::mem::replace(&mut self.bindings, vec![HashSet::new()]);
                if let Some(parameters) = child.child_by_field_name("parameters") {
                    self.bind_pattern(parameters);
                }
                if let Some(body) = child.child_by_field_name("body") {
                    self.visit(body, Some(idx), None);
                }
                self.bindings = saved_bindings;
            }
            "struct_item" | "enum_item" if !self.references_only => {
                let name = name_field(&child, self.source)
                    .unwrap_or("<anon>")
                    .to_string();
                let sig = signature_until_body_or_semi(&child, self.source);
                let kind = if child.kind() == "struct_item" {
                    "struct"
                } else {
                    "enum"
                };
                push_def_or_decorated(self.pending, name, kind, &child, sig, parent_index, attrs);
            }
            "trait_item" if !self.references_only => {
                let name = name_field(&child, self.source)
                    .unwrap_or("<anon>")
                    .to_string();
                let sig = signature_until_body_or_semi(&child, self.source);
                let idx = push_def_or_decorated(
                    self.pending,
                    name,
                    "trait",
                    &child,
                    sig,
                    parent_index,
                    attrs,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    self.visit(body, Some(idx), None);
                }
            }
            "impl_item" if !self.references_only => {
                // The impl block becomes a symbol named after its target type;
                // methods inside parent to this impl symbol.
                let target_name =
                    impl_target_name(&child, self.source).unwrap_or_else(|| "<impl>".to_string());
                let sig = signature_until_body_or_semi(&child, self.source);
                let idx = push_def_or_decorated(
                    self.pending,
                    target_name,
                    "impl",
                    &child,
                    sig,
                    parent_index,
                    attrs,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    self.visit(body, Some(idx), None);
                }
            }
            "mod_item" if !self.references_only => {
                // `mod foo { ... }` — container symbol.
                let name = name_field(&child, self.source)
                    .unwrap_or("<anon>")
                    .to_string();
                let sig = signature_until_body_or_semi(&child, self.source);
                let idx = push_def_or_decorated(
                    self.pending,
                    name,
                    "mod",
                    &child,
                    sig,
                    parent_index,
                    attrs,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    self.visit(body, Some(idx), None);
                }
            }
            "const_item" | "static_item" => {
                if let Some(value) = child.child_by_field_name("value") {
                    self.visit(value, parent_index, None);
                }
            }
            "let_declaration" | "let_condition" => {
                if let Some(value) = child.child_by_field_name("value") {
                    self.visit(value, parent_index, None);
                }
                if let Some(alternative) = child.child_by_field_name("alternative") {
                    self.visit(alternative, parent_index, None);
                }
                if let Some(pattern) = child.child_by_field_name("pattern") {
                    self.bind_pattern(pattern);
                }
            }
            "closure_expression" => {
                self.bindings.push(HashSet::new());
                if let Some(parameters) = child.child_by_field_name("parameters") {
                    self.bind_pattern(parameters);
                }
                if let Some(body) = child.child_by_field_name("body") {
                    self.visit(body, parent_index, None);
                }
                self.bindings.pop();
            }
            "for_expression" => {
                if let Some(value) = child.child_by_field_name("value") {
                    self.visit(value, parent_index, None);
                }
                self.bindings.push(HashSet::new());
                if let Some(pattern) = child.child_by_field_name("pattern") {
                    self.bind_pattern(pattern);
                }
                if let Some(body) = child.child_by_field_name("body") {
                    self.visit(body, parent_index, None);
                }
                self.bindings.pop();
            }
            "if_expression" | "while_expression" => {
                self.bindings.push(HashSet::new());
                for field in ["condition", "consequence", "body"] {
                    if let Some(node) = child.child_by_field_name(field) {
                        self.visit(node, parent_index, None);
                    }
                }
                self.bindings.pop();
                if let Some(alternative) = child.child_by_field_name("alternative") {
                    self.visit(alternative, parent_index, None);
                }
            }
            "match_arm" => {
                self.bindings.push(HashSet::new());
                if let Some(pattern) = child.child_by_field_name("pattern") {
                    self.bind_pattern(pattern);
                    if let Some(condition) = pattern.child_by_field_name("condition") {
                        self.visit(condition, parent_index, None);
                    }
                }
                if let Some(value) = child.child_by_field_name("value") {
                    self.visit(value, parent_index, None);
                }
                self.bindings.pop();
            }
            "call_expression" => {
                if let Some(function) = child.child_by_field_name("function") {
                    if let Some(target) = callable_target(function, self.source) {
                        self.push_target(target, child, parent_index, self.references_only);
                    }
                }
                self.walk(child, parent_index);
            }
            "macro_invocation" => {
                if let Some(macro_node) = child.child_by_field_name("macro") {
                    if let Some(name) = rightmost_identifier(&macro_node, self.source) {
                        let path = node_text(&macro_node, self.source).map(|t| format!("{t}!"));
                        self.push_target(
                            CallTarget {
                                name,
                                path,
                                type_prefix: None,
                                kind: "macro",
                            },
                            child,
                            parent_index,
                            self.references_only,
                        );
                    }
                }
                self.macro_references(child, parent_index);
            }
            "use_declaration" if !self.references_only => {
                if let Some(arg) = child.child_by_field_name("argument") {
                    let line = self.line(child);
                    collect_use_names(
                        &arg,
                        self.source,
                        self.pending,
                        self.module_index,
                        line,
                        None,
                    );
                }
            }
            "identifier" | "scoped_identifier" | "generic_function" if is_value_position(child) => {
                if let Some(target) = callable_target(child, self.source) {
                    if target.kind == "scoped"
                        || (target.kind == "function"
                            && self.known_callables.contains(&target.name))
                    {
                        self.push_target(target, child, parent_index, true);
                    }
                }
            }
            // These contain names, types, or quoted tokens rather than values.
            "use_declaration"
            | "macro_definition"
            | "token_tree"
            | "attribute_item"
            | "inner_attribute_item"
            | "type_arguments"
            | "type_parameters"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "impl_item"
            | "mod_item"
            | "function_signature_item"
            | "type_item" => {}
            _ => self.walk(child, parent_index),
        }
    }

    fn push_target(
        &mut self,
        mut target: CallTarget,
        node: Node,
        parent_index: Option<usize>,
        reference: bool,
    ) {
        if target.kind == "function" && self.is_bound(&target.name) {
            if reference {
                return;
            }
            target.kind = "indirect";
        }
        if target.type_prefix.as_deref() == Some("Self") {
            let mut parent = parent_index;
            while let Some(index) = parent {
                let symbol = &self.pending.symbols[index];
                if symbol.kind == "impl" {
                    target.type_prefix = Some(symbol.name.clone());
                    break;
                }
                parent = symbol.parent_index;
            }
        }
        self.pending.edges.push(PendingEdge {
            from_index: parent_index.unwrap_or(self.module_index),
            to_name: target.name,
            to_path: target.path,
            to_type: target.type_prefix,
            target_kind: Some(target.kind.to_owned()),
            kind: if reference { "references" } else { "calls" }.to_owned(),
            line: self.line(node),
        });
    }

    fn macro_references(&mut self, node: Node, parent_index: Option<usize>) {
        if self.macro_depth >= MACRO_DEPTH_LIMIT {
            return;
        }
        let mut cursor = node.walk();
        let Some(tokens) = node
            .named_children(&mut cursor)
            .find(|node| node.kind() == "token_tree")
        else {
            return;
        };
        let Some(body) = self
            .source
            .get(tokens.start_byte().saturating_add(1)..tokens.end_byte().saturating_sub(1))
        else {
            return;
        };
        if body.len() > MACRO_BODY_BYTE_LIMIT {
            return;
        }
        let mut parser = Parser::new();
        if parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .is_err()
        {
            return;
        }
        for (prefix, suffix) in [
            ("fn __mmcg_macro() { let _ = (", "\n); }"),
            ("fn __mmcg_macro() {", "\n}"),
        ] {
            let bytes = prefix.len() + body.len() + suffix.len();
            if self.macro_budget.parses == 0 || bytes > self.macro_budget.bytes {
                return;
            }
            self.macro_budget.parses -= 1;
            self.macro_budget.bytes -= bytes;
            let mut source = Vec::with_capacity(bytes);
            source.extend_from_slice(prefix.as_bytes());
            source.extend_from_slice(body);
            source.extend_from_slice(suffix.as_bytes());
            let Some(tree) = parser.parse(&source, None) else {
                return;
            };
            if tree.root_node().has_error() {
                continue;
            }
            let Some(block) = tree
                .root_node()
                .named_child(0)
                .and_then(|function| function.child_by_field_name("body"))
            else {
                return;
            };
            RustWalker {
                source: &source,
                pending: self.pending,
                module_index: self.module_index,
                known_callables: self.known_callables,
                bindings: self.bindings.clone(),
                item_bindings: self.item_bindings.clone(),
                references_only: true,
                line_offset: self
                    .line_offset
                    .saturating_add(tokens.start_position().row as u32),
                macro_depth: self.macro_depth + 1,
                macro_budget: self.macro_budget,
            }
            .visit(block, parent_index, None);
            break;
        }
    }
}

/// Outer attributes retained as marker names and complete declaration text.
struct DeclarationAttributes {
    decorators: Option<String>,
    text: String,
}

fn take_attrs(attrs: &mut Vec<Node<'_>>, source: &[u8]) -> Option<DeclarationAttributes> {
    if attrs.is_empty() {
        return None;
    }
    let mut names = Vec::new();
    let mut text = Vec::new();
    for attr in attrs.drain(..) {
        if let Some(name) = extract_attribute_name(&attr, source) {
            names.push(name);
        }
        if let Some(value) = node_text(&attr, source) {
            text.push(value);
        }
    }
    Some(DeclarationAttributes {
        decorators: (!names.is_empty()).then(|| format!(",{},", names.join(","))),
        text: text.join(" "),
    })
}

/// Include outer attributes in the header while preserving declaration coordinates.
fn push_def_or_decorated(
    pending: &mut PendingFile,
    name: String,
    kind: &str,
    node: &Node,
    signature: Option<String>,
    parent_index: Option<usize>,
    attrs: Option<DeclarationAttributes>,
) -> usize {
    let (signature, decorators) = match attrs {
        Some(attrs) => (
            signature.map(|signature| format!("{} {signature}", attrs.text)),
            attrs.decorators,
        ),
        None => (signature, None),
    };
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

struct CallTarget {
    name: String,
    path: Option<String>,
    type_prefix: Option<String>,
    kind: &'static str,
}

fn callable_target(mut node: Node, source: &[u8]) -> Option<CallTarget> {
    let path = node_text(&node, source).map(String::from);
    // Turbofish type arguments cannot become the target name (`map::<T>`).
    while matches!(node.kind(), "generic_function" | "parenthesized_expression") {
        node = if node.kind() == "generic_function" {
            node.child_by_field_name("function")?
        } else {
            node.named_child(0)?
        };
    }
    let (name, type_prefix, kind) = match node.kind() {
        "identifier" => (node_text(&node, source)?.to_owned(), None, "function"),
        "field_expression" => (
            node_text(&node.child_by_field_name("field")?, source)?.to_owned(),
            None,
            "method",
        ),
        "scoped_identifier" => {
            let name = node_text(&node.child_by_field_name("name")?, source)?.to_owned();
            let prefix = node
                .child_by_field_name("path")
                .filter(|prefix| {
                    matches!(
                        prefix.kind(),
                        "identifier"
                            | "scoped_identifier"
                            | "generic_type"
                            | "self"
                            | "super"
                            | "crate"
                    )
                })
                .and_then(|prefix| rightmost_identifier(&prefix, source));
            (name, prefix, "scoped")
        }
        // `factory()()` and `(callback as fn())()` do not name a directly
        // resolved function. Their nested expressions are still traversed.
        _ => return None,
    };
    Some(CallTarget {
        name,
        path,
        type_prefix,
        kind,
    })
}

fn is_value_position(node: Node) -> bool {
    let mut wrapped = node;
    while let Some(parent) = wrapped
        .parent()
        .filter(|parent| parent.kind() == "parenthesized_expression")
    {
        wrapped = parent;
    }
    if wrapped.parent().is_some_and(|parent| {
        parent.kind() == "call_expression"
            && parent.child_by_field_name("function") == Some(wrapped)
    }) {
        return false;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        "arguments"
        | "array_expression"
        | "tuple_expression"
        | "return_expression"
        | "break_expression"
        | "expression_statement"
        | "block"
        | "parenthesized_expression"
        | "reference_expression"
        | "unary_expression"
        | "binary_expression"
        | "assignment_expression"
        | "compound_assignment_expr"
        | "range_expression"
        | "index_expression"
        | "await_expression"
        | "try_expression" => true,
        "let_declaration"
        | "const_item"
        | "static_item"
        | "field_initializer"
        | "type_cast_expression"
        | "match_arm" => parent.child_by_field_name("value") == Some(node),
        "closure_expression" => parent.child_by_field_name("body") == Some(node),
        "shorthand_field_initializer" => true,
        _ => false,
    }
}

fn collect_pattern_bindings(node: Node, source: &[u8], bindings: &mut HashSet<String>) {
    match node.kind() {
        "identifier" | "self" | "shorthand_field_identifier" => {
            if let Some(name) = node_text(&node, source) {
                bindings.insert(name.to_owned());
            }
        }
        "parameter" | "field_pattern" => {
            if let Some(pattern) = node.child_by_field_name("pattern") {
                collect_pattern_bindings(pattern, source, bindings);
            } else if node.kind() == "field_pattern" {
                if let Some(name) = node.child_by_field_name("name") {
                    if let Some(name) = node_text(&name, source) {
                        bindings.insert(name.to_owned());
                    }
                }
            }
        }
        "scoped_identifier"
        | "scoped_type_identifier"
        | "type_identifier"
        | "range_pattern"
        | "const_block" => {}
        _ => {
            let type_node = node.child_by_field_name("type");
            let guard = node.child_by_field_name("condition");
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if Some(child) != type_node && Some(child) != guard {
                    collect_pattern_bindings(child, source, bindings);
                }
            }
        }
    }
}

fn collect_callable_names(node: Node, source: &[u8], names: &mut HashSet<String>) {
    match node.kind() {
        "function_item" | "function_signature_item" | "struct_item" | "enum_item" => {
            if let Some(name) = name_field(&node, source) {
                names.insert(name.to_owned());
            }
        }
        "use_declaration" => {
            if let Some(argument) = node.child_by_field_name("argument") {
                collect_import_bindings(argument, source, names);
            }
            return;
        }
        "macro_definition" | "macro_invocation" => return,
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_callable_names(child, source, names);
    }
}

fn collect_import_bindings(node: Node, source: &[u8], names: &mut HashSet<String>) {
    match node.kind() {
        "identifier" | "scoped_identifier" => {
            if let Some(name) = rightmost_identifier(&node, source) {
                names.insert(name);
            }
        }
        "use_as_clause" => {
            if let Some(alias) = node
                .child_by_field_name("alias")
                .and_then(|alias| node_text(&alias, source))
            {
                names.insert(alias.to_owned());
            }
        }
        "scoped_use_list" => {
            if let Some(list) = node.child_by_field_name("list") {
                collect_import_bindings(list, source, names);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_import_bindings(child, source, names);
            }
        }
        _ => {}
    }
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
    let consumed_line_ending = usize::from(
        left.kind() == "line_comment"
            && left.end_position().column == 0
            && left.end_position().row > left.start_position().row,
    );
    newlines.saturating_add(consumed_line_ending) <= 1
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

fn is_outer_doc_comment(node: &Node) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
        && node.child_by_field_name("outer").is_some()
}

fn is_inner_doc_comment(node: &Node) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
        && node.child_by_field_name("inner").is_some()
}

fn push_rust_doc_comment(builder: &mut DocumentationTextBuilder, node: &Node, source: &[u8]) {
    let Some(text) = node_text(node, source) else {
        return;
    };
    if node.kind() == "line_comment" {
        let body = text.get(3..).unwrap_or_default();
        builder.push_line(body.strip_prefix(' ').unwrap_or(body));
        return;
    }
    let body = text
        .strip_prefix("/**")
        .or_else(|| text.strip_prefix("/*!"))
        .unwrap_or_default()
        .strip_suffix("*/")
        .unwrap_or_default();
    for line in body.lines() {
        let line = line.trim_start();
        let line = line.strip_prefix('*').unwrap_or(line);
        builder.push_line(line.strip_prefix(' ').unwrap_or(line).trim_end());
    }
}

fn symbol_index_for_declaration(
    node: &Node,
    source: &[u8],
    symbols: &HashMap<(u32, &str), usize>,
) -> Option<usize> {
    let name = match node.kind() {
        "impl_item" => impl_target_name(node, source)?,
        _ => name_field(node, source)?.to_string(),
    };
    let line = node.start_position().row as u32 + 1;
    symbols.get(&(line, name.as_str())).copied()
}

fn declaration_owns_outer_docs(kind: &str) -> bool {
    matches!(
        kind,
        "function_item" | "struct_item" | "enum_item" | "trait_item" | "impl_item" | "mod_item"
    )
}

fn collect_outer_docs_in(
    node: Node,
    source: &[u8],
    symbols: &HashMap<(u32, &str), usize>,
    output: &mut Vec<RawConceptDocumentation>,
) {
    let mut cursor = node.walk();
    let children = node.named_children(&mut cursor).collect::<Vec<_>>();
    for (declaration_index, declaration) in children.iter().enumerate() {
        if !declaration_owns_outer_docs(declaration.kind()) {
            continue;
        }
        let Some(symbol_index) = symbol_index_for_declaration(declaration, source, symbols) else {
            continue;
        };
        let mut preceding_index = declaration_index;
        let mut next = *declaration;
        while preceding_index > 0 {
            let candidate = children[preceding_index - 1];
            if candidate.kind() != "attribute_item"
                || !documentation_nodes_adjacent(source, &candidate, &next)
            {
                break;
            }
            preceding_index -= 1;
            next = candidate;
        }

        let mut docs = Vec::new();
        while preceding_index > 0 {
            let candidate = children[preceding_index - 1];
            if !is_outer_doc_comment(&candidate)
                || !documentation_comment_starts_line(source, &candidate)
                || !documentation_nodes_adjacent(source, &candidate, &next)
            {
                break;
            }
            docs.push(candidate);
            preceding_index -= 1;
            next = candidate;
        }
        if docs.is_empty() {
            continue;
        }
        let mut builder = DocumentationTextBuilder::default();
        for doc in docs.into_iter().rev() {
            push_rust_doc_comment(&mut builder, &doc, source);
        }
        if let Some(candidate) = builder.finish(symbol_index) {
            output.push(candidate);
        }
    }
}

fn collect_inner_module_docs(
    node: Node,
    module_index: usize,
    source: &[u8],
    output: &mut Vec<RawConceptDocumentation>,
) {
    let mut cursor = node.walk();
    let mut builder = DocumentationTextBuilder::default();
    for child in node.named_children(&mut cursor) {
        if is_inner_doc_comment(&child) && documentation_comment_starts_line(source, &child) {
            push_rust_doc_comment(&mut builder, &child, source);
        }
    }
    if let Some(candidate) = builder.finish(module_index) {
        output.push(candidate);
    }
}

fn walk_concept_documentation(
    node: Node,
    source: &[u8],
    symbols: &HashMap<(u32, &str), usize>,
    output: &mut Vec<RawConceptDocumentation>,
) {
    collect_outer_docs_in(node, source, symbols, output);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "mod_item" {
            if let (Some(module_index), Some(body)) = (
                symbol_index_for_declaration(&child, source, symbols),
                child.child_by_field_name("body"),
            ) {
                collect_inner_module_docs(body, module_index, source, output);
            }
        }
        walk_concept_documentation(child, source, symbols, output);
    }
}

pub(super) fn collect_concept_documentation(
    tree: &Tree,
    source: &[u8],
    pending: &PendingFile,
    module_index: usize,
) -> Vec<RawConceptDocumentation> {
    let root = tree.root_node();
    let mut output = Vec::new();
    let mut symbols = HashMap::with_capacity(pending.symbols.len());
    for (index, symbol) in pending.symbols.iter().enumerate() {
        if symbol.kind != "module" {
            symbols
                .entry((symbol.line_start, symbol.name.as_str()))
                .or_insert(index);
        }
    }
    collect_inner_module_docs(root, module_index, source, &mut output);
    walk_concept_documentation(root, source, &symbols, &mut output);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::common;
    use crate::indexer::parse_one;

    fn fixture(name: &str, source: &str) -> PendingFile {
        let path = common::write_tmp("rs", name, source);
        parse_one(&path, path.parent().unwrap(), &RustExtractor).unwrap()
    }

    fn outgoing<'a>(pending: &'a PendingFile, name: &str) -> Vec<&'a PendingEdge> {
        pending
            .edges
            .iter()
            .filter(|edge| pending.symbols[edge.from_index].name == name)
            .collect()
    }

    #[test]
    fn outer_attributes_extend_signatures_without_moving_declarations() {
        let source = r##"#![allow(dead_code)]
#[cfg(
    feature = "checks"
)]
// Keep the attribute attached across comments.
#[label(r#"literal,[value]"#)]
/* Another comment. */
#[test]
fn checks_value() { assert!(true); }
fn plain() {}
"##;
        let pending = fixture("outer_attributes.rs", source);
        let test = pending
            .symbols
            .iter()
            .find(|s| s.name == "checks_value")
            .unwrap();
        assert_eq!(test.line_start, 9);
        assert_eq!(test.line_end, 9);
        assert_eq!(test.decorators.as_deref(), Some(",cfg,label,test,"));
        assert_eq!(
            test.signature.as_deref(),
            Some("#[cfg(\n    feature = \"checks\"\n)] #[label(r#\"literal,[value]\"#)] #[test] fn checks_value()")
        );
        let plain = pending.symbols.iter().find(|s| s.name == "plain").unwrap();
        assert_eq!(plain.decorators, None);
        assert_eq!(plain.signature.as_deref(), Some("fn plain()"));
        assert_eq!(plain.line_start, 10);
        assert!(!pending.edges.iter().any(|edge| edge.to_name == "label"));
    }

    #[test]
    fn outer_attributes_cover_existing_rust_declaration_kinds() {
        let pending = fixture(
            "attributed_items.rs",
            r#"#[derive(Debug)]
struct Value;
#[derive(Clone)]
enum Choice { One }
#[cfg(feature = "api")]
trait Api {}
#[cfg(feature = "impl")]
impl Value {
    #[inline(always)]
    fn value(&self) {}
}
#[cfg(test)]
mod checks {
    #![allow(dead_code)]
    fn plain_nested() {}
}
"#,
        );
        for (name, kind, line, decorators, signature) in [
            (
                "Value",
                "struct",
                2,
                ",derive,",
                "#[derive(Debug)] struct Value",
            ),
            (
                "Choice",
                "enum",
                4,
                ",derive,",
                "#[derive(Clone)] enum Choice",
            ),
            (
                "Api",
                "trait",
                6,
                ",cfg,",
                "#[cfg(feature = \"api\")] trait Api",
            ),
            (
                "Value",
                "impl",
                8,
                ",cfg,",
                "#[cfg(feature = \"impl\")] impl Value",
            ),
            (
                "value",
                "method",
                10,
                ",inline,",
                "#[inline(always)] fn value(&self)",
            ),
            ("checks", "mod", 13, ",cfg,", "#[cfg(test)] mod checks"),
        ] {
            let symbol = pending
                .symbols
                .iter()
                .find(|s| s.name == name && s.kind == kind)
                .unwrap();
            assert_eq!(symbol.line_start, line, "{name}/{kind}");
            assert_eq!(
                symbol.decorators.as_deref(),
                Some(decorators),
                "{name}/{kind}"
            );
            assert_eq!(
                symbol.signature.as_deref(),
                Some(signature),
                "{name}/{kind}"
            );
        }
        let nested = pending
            .symbols
            .iter()
            .find(|s| s.name == "plain_nested")
            .unwrap();
        assert_eq!(nested.decorators, None);
        assert_eq!(nested.signature.as_deref(), Some("fn plain_nested()"));
    }

    #[test]
    fn classifies_receiver_scoped_generic_and_indirect_call_targets() {
        let pending = fixture(
            "call_targets.rs",
            r#"
fn generic<T>() {}
struct Worker;
impl Worker {
    fn associated<T>() {}
    fn render<T>(&self) {}
    fn run(&self) { self.render::<Item>(); Self::associated::<Item>(); }
}
fn entry(worker: Worker, callback: fn()) {
    generic::<Item>();
    worker.render::<Item>();
    Worker::associated::<Item>();
    callback();
    (generic)();
}
"#,
        );
        let edges = outgoing(&pending, "entry");
        let classified = edges
            .iter()
            .map(|edge| {
                (
                    edge.to_name.as_str(),
                    edge.target_kind.as_deref(),
                    edge.to_type.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            classified,
            vec![
                ("generic", Some("function"), None),
                ("render", Some("method"), None),
                ("associated", Some("scoped"), Some("Worker")),
                ("callback", Some("indirect"), None),
                ("generic", Some("function"), None),
            ]
        );
        let method = outgoing(&pending, "run");
        assert_eq!(method[0].target_kind.as_deref(), Some("method"));
        assert_eq!(method[1].to_name, "associated");
        assert_eq!(method[1].to_type.as_deref(), Some("Worker"));
        assert!(pending.edges.iter().all(|edge| edge.to_name != "Item"));
    }

    #[test]
    fn records_callback_registries_tuples_and_returned_function_values() {
        let pending = fixture(
            "callback_values.rs",
            r#"
fn schema_search() {}
fn handle_search() {}
fn generic<T>() {}
static TOOLS: [Tool; 1] = [refreshable_tool("mmcg_search", schema_search, handle_search)];
static PAIRS: [(fn(), fn()); 1] = [(schema_search, handle_search)];
fn returned() -> fn() { handle_search }
fn explicit_return() -> fn() { return schema_search; }
fn pointers() {
    let pair = (schema_search as fn(), &handle_search);
    register(pair);
    register(generic::<u8>);
    register(module::external);
}
"#,
        );
        let module_refs = pending
            .edges
            .iter()
            .filter(|edge| {
                edge.kind == "references" && pending.symbols[edge.from_index].kind == "module"
            })
            .map(|edge| edge.to_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            module_refs,
            vec![
                "schema_search",
                "handle_search",
                "schema_search",
                "handle_search"
            ]
        );
        for (function, expected) in [
            ("returned", "handle_search"),
            ("explicit_return", "schema_search"),
        ] {
            let refs = outgoing(&pending, function);
            assert_eq!(refs.len(), 1);
            assert_eq!(refs[0].to_name, expected);
            assert_eq!(refs[0].kind, "references");
        }
        let refs = outgoing(&pending, "pointers")
            .into_iter()
            .filter(|edge| edge.kind == "references")
            .map(|edge| edge.to_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            refs,
            vec!["schema_search", "handle_search", "generic", "external"]
        );
        assert!(!pending
            .edges
            .iter()
            .any(|edge| edge.to_name == "mmcg_search" || edge.to_name == "pair"));
    }

    #[test]
    fn ignores_parameter_local_closure_and_pattern_shadows() {
        let pending = fixture(
            "callback_shadows.rs",
            r#"
fn target() {}
fn schema() {}
fn params(target: fn(), pair: (fn(), fn())) {
    register(target);
    target();
    let (schema, _) = pair;
    register(schema);
    register(crate::target);
}
fn locals() {
    register(target);
    let target = || {};
    register(target);
    target();
    { let schema = 0; register(schema); }
    register(schema);
    let first = |target: fn()| register(target);
    let second = |(schema, _)| register(schema);
}
fn branches(values: Vec<fn()>, value: Option<fn()>) {
    for target in values { register(target); }
    if let Some(target) = value { register(target); } else { register(target); }
    while let Some(target) = value { register(target); }
    match value { Some(target) if check(target) => register(target), _ => register(target) }
}
"#,
        );
        let refs = |name| {
            outgoing(&pending, name)
                .into_iter()
                .filter(|edge| edge.kind == "references")
                .map(|edge| edge.to_path.as_deref().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(refs("params"), vec!["crate::target"]);
        assert_eq!(refs("locals"), vec!["target", "schema"]);
        assert_eq!(refs("branches"), vec!["target", "target"]);
        assert!(outgoing(&pending, "params").iter().any(
            |edge| edge.to_name == "target" && edge.target_kind.as_deref() == Some("indirect")
        ));
        assert!(!outgoing(&pending, "params").iter().any(
            |edge| edge.to_name == "target" && edge.target_kind.as_deref() == Some("function")
        ));
    }

    #[test]
    fn macro_body_references_keep_source_lines_and_containing_function() {
        let source = "fn target() {}\nfn inner() {}\nfn entry() {\n\
            println!(\"target() is text\", target());\n\
            assert_eq!(\n\
                nested!(target(), inner()),\n\
                target());\n\
            vec![target; 2];\n\
            println!(\"{{target()}}\");\n\
            opaque! { target => inner };\n\
        }\n";
        let pending = fixture("macro_references.rs", source);
        let target_refs = outgoing(&pending, "entry")
            .into_iter()
            .filter(|edge| edge.to_name == "target")
            .collect::<Vec<_>>();
        assert_eq!(
            target_refs.iter().map(|edge| edge.line).collect::<Vec<_>>(),
            vec![4, 6, 7, 8]
        );
        assert!(target_refs.iter().all(
            |edge| edge.kind == "references" && edge.target_kind.as_deref() == Some("function")
        ));
        let inner = pending
            .edges
            .iter()
            .find(|edge| edge.to_name == "inner")
            .unwrap();
        assert_eq!(inner.line, 6);
        assert_eq!(inner.kind, "references");
        assert_eq!(pending.symbols[inner.from_index].name, "entry");
        assert!(pending
            .symbols
            .iter()
            .all(|symbol| symbol.name != "__mmcg_macro"));
    }

    #[test]
    fn macro_tokens_do_not_turn_strings_comments_patterns_or_shadows_into_edges() {
        let pending = fixture(
            "macro_reference_noise.rs",
            r##"
fn target() {}
macro_rules! declare { () => { fn generated() { target(); } } }
fn shadow(target: fn()) {
    println!("target()", target);
    println!("{}", target());
}
fn text() {
    println!(r#"target()"#);
    println!("{}", 1 /* target() */);
    opaque! { target => 1 };
    opaque! { fn generated() { target(); } };
}
"##,
        );
        assert!(!pending.edges.iter().any(|edge| edge.to_name == "target"));
        assert!(pending
            .symbols
            .iter()
            .all(|symbol| symbol.name != "generated"));
    }

    #[test]
    fn oversized_macro_bodies_preserve_only_the_macro_invocation() {
        let source = format!(
            "fn target() {{}}\nfn entry() {{ huge!(\"{}\", target()); }}",
            "x".repeat(MACRO_BODY_BYTE_LIMIT)
        );
        let pending = fixture("macro_reference_limit.rs", &source);
        let edges = outgoing(&pending, "entry");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to_name, "huge");
        assert_eq!(edges[0].target_kind.as_deref(), Some("macro"));
    }

    #[test]
    fn extracts_function_and_struct() {
        let path = common::write_tmp(
            "rs",
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
        let path = common::write_tmp(
            "rs",
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
        let path = common::write_tmp(
            "rs",
            "m.rs",
            "fn main() { println!(\"hi\"); vec![1, 2, 3]; }\n",
        );
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
        let path = common::write_tmp(
            "rs",
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
