use mmcg::{indexer::Indexer, store::Store};
use std::fs;
use tempfile::TempDir;

fn setup_store(tmp: &TempDir, source_name: &str, content: &str) -> Store {
    fs::write(tmp.path().join(source_name), content).unwrap();
    let db_path = tmp.path().join("mmcg.db");
    let mut store = Store::open(&db_path).unwrap();
    Indexer::new(tmp.path())
        .index_all(&mut store, true)
        .unwrap();
    store
}

fn has_symbol(store: &Store, name: &str) -> bool {
    store
        .search_symbols(name, None, None)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

fn has_symbol_kind(store: &Store, name: &str, kind: &str) -> bool {
    store
        .search_symbols(name, Some(kind), None)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

fn has_call(store: &Store, from: &str, to: &str) -> bool {
    store
        .search_symbols(from, None, None)
        .unwrap_or_default()
        .into_iter()
        .any(|s| {
            store
                .callees_of(s.id, None)
                .unwrap_or_default()
                .iter()
                .any(|(n, _)| n == to)
        })
}

fn has_import(store: &Store, file: &str, import_name: &str) -> bool {
    store
        .imports_of(file)
        .unwrap_or_default()
        .iter()
        .any(|(name, _, _)| name == import_name)
}

#[test]
fn golden_python() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.py",
        r#"
import os

def greet(name):
    print(name)

class Greeter:
    def hello(self, name):
        greet(name)
"#,
    );
    assert!(has_symbol(&store, "greet"), "function greet");
    assert!(has_symbol_kind(&store, "Greeter", "class"), "class Greeter");
    assert!(has_symbol(&store, "hello"), "method hello");
    assert!(has_call(&store, "hello", "greet"), "hello → greet");
    assert!(has_import(&store, "test.py", "os"), "import os");
}

#[test]
fn golden_typescript() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.ts",
        r#"
import { join } from 'path';

function greet(name: string): void {
    join(name, '.');
}

class Greeter {
    hello(name: string): void {
        greet(name);
    }
}
"#,
    );
    assert!(has_symbol(&store, "greet"), "function greet");
    assert!(has_symbol_kind(&store, "Greeter", "class"), "class Greeter");
    assert!(has_symbol(&store, "hello"), "method hello");
    assert!(has_call(&store, "hello", "greet"), "hello → greet");
    assert!(has_import(&store, "test.ts", "join"), "import join");
}

#[test]
fn golden_javascript() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.js",
        r#"
function greet(name) {
    console.log(name);
}

class Greeter {
    hello(name) {
        greet(name);
    }
}
"#,
    );
    assert!(has_symbol(&store, "greet"), "function greet");
    assert!(has_symbol_kind(&store, "Greeter", "class"), "class Greeter");
    assert!(has_call(&store, "hello", "greet"), "hello → greet");
}

#[test]
fn golden_rust() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.rs",
        r#"
fn greet(name: &str) {
    println!("{}", name);
}

struct Greeter;

impl Greeter {
    fn hello(&self, name: &str) {
        greet(name);
    }
}
"#,
    );
    assert!(has_symbol(&store, "greet"), "function greet");
    assert!(
        has_symbol_kind(&store, "Greeter", "struct"),
        "struct Greeter"
    );
    assert!(has_symbol_kind(&store, "hello", "method"), "method hello");
    assert!(has_call(&store, "hello", "greet"), "hello → greet");
}

#[test]
fn golden_go() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.go",
        r#"
package main

import "fmt"

func Greet(name string) {
    fmt.Println(name)
}

type Greeter struct{}

func (g Greeter) Hello(name string) {
    Greet(name)
}
"#,
    );
    assert!(
        has_symbol_kind(&store, "Greet", "function"),
        "function Greet"
    );
    assert!(has_symbol_kind(&store, "Greeter", "struct"), "type Greeter");
    assert!(has_symbol_kind(&store, "Hello", "method"), "method Hello");
    assert!(has_call(&store, "Hello", "Greet"), "Hello → Greet");
    assert!(has_import(&store, "test.go", "fmt"), "import fmt");
}

#[test]
fn golden_java() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "Test.java",
        r#"
import java.util.List;

public class Test {
    public static void greet(String name) {
        System.out.println(name);
    }

    public void hello(String name) {
        greet(name);
    }
}
"#,
    );
    assert!(has_symbol_kind(&store, "Test", "class"), "class Test");
    assert!(has_symbol(&store, "greet"), "method greet");
    assert!(has_symbol(&store, "hello"), "method hello");
    assert!(has_call(&store, "hello", "greet"), "hello → greet");
    assert!(has_import(&store, "Test.java", "List"), "import List");
}

#[test]
fn golden_csharp() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "Test.cs",
        r#"
using System;

public class Greeter {
    public void Hello(string name) {
        Console.WriteLine(name);
    }
}
"#,
    );
    assert!(has_symbol_kind(&store, "Greeter", "class"), "class Greeter");
    assert!(has_symbol(&store, "Hello"), "method Hello");
    assert!(has_import(&store, "Test.cs", "System"), "using System");
}

#[test]
fn golden_php() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.php",
        r#"<?php

function greet(string $name): void {
    echo $name;
}

class Greeter {
    public function hello(string $name): void {
        greet($name);
    }
}
"#,
    );
    assert!(
        has_symbol_kind(&store, "greet", "function"),
        "function greet"
    );
    assert!(has_symbol_kind(&store, "Greeter", "class"), "class Greeter");
    assert!(has_symbol(&store, "hello"), "method hello");
    assert!(has_call(&store, "hello", "greet"), "hello → greet");
}

#[test]
fn golden_cpp() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.cpp",
        r#"
void greet(const char* name) {
    (void)name;
}

class Greeter {
public:
    void hello(const char* name) {
        greet(name);
    }
};
"#,
    );
    assert!(
        has_symbol_kind(&store, "greet", "function"),
        "function greet"
    );
    assert!(has_symbol_kind(&store, "Greeter", "class"), "class Greeter");
    assert!(has_symbol(&store, "hello"), "method hello");
    assert!(has_call(&store, "hello", "greet"), "hello → greet");
}

/// Python: obj.method() calls are stored with the leaf name only.
/// callers_of("hello") will find any function that calls *anything* named
/// "hello" — there is no type resolution to distinguish A.hello from B.hello.
/// This documents the precision limitation for Python heuristic edges.
#[test]
fn golden_python_leaf_name_precision() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.py",
        r#"
class A:
    def hello(self):
        pass

class B:
    def hello(self):
        pass

def caller():
    a = A()
    a.hello()
"#,
    );
    let hello_syms = store.search_symbols("hello", None, None).unwrap();
    assert_eq!(
        hello_syms.len(),
        2,
        "both A.hello and B.hello are indexed as 'hello'"
    );

    let callers = store.callers_of("hello", None, None).unwrap();
    assert!(
        !callers.is_empty(),
        "caller() is reported as a caller of 'hello' (leaf-name match)"
    );
}

/// Callees: every language that records call edges should expose them via callees_of.
#[test]
fn golden_callees_rust() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.rs",
        r#"
fn helper() {}
fn orchestrate() {
    helper();
}
fn unrelated() {}
"#,
    );
    let syms = store.search_symbols("orchestrate", None, None).unwrap();
    assert!(!syms.is_empty(), "orchestrate indexed");
    let callees = store.callees_of(syms[0].id, None).unwrap();
    assert!(
        callees.iter().any(|(n, _)| n == "helper"),
        "orchestrate → helper callee edge"
    );
    assert!(
        !callees.iter().any(|(n, _)| n == "unrelated"),
        "unrelated not a callee"
    );
}

#[test]
fn golden_callees_go() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.go",
        r#"
package main

func helper() {}

func orchestrate() {
    helper()
}
"#,
    );
    let syms = store.search_symbols("orchestrate", None, None).unwrap();
    assert!(!syms.is_empty(), "orchestrate indexed");
    let callees = store.callees_of(syms[0].id, None).unwrap();
    assert!(
        callees.iter().any(|(n, _)| n == "helper"),
        "orchestrate → helper callee edge (Go)"
    );
}

#[test]
fn golden_callees_typescript() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.ts",
        r#"
function helper(): void {}
function orchestrate(): void {
    helper();
}
"#,
    );
    let syms = store.search_symbols("orchestrate", None, None).unwrap();
    assert!(!syms.is_empty(), "orchestrate indexed");
    let callees = store.callees_of(syms[0].id, None).unwrap();
    assert!(
        callees.iter().any(|(n, _)| n == "helper"),
        "orchestrate → helper callee edge (TypeScript)"
    );
}

/// Known limitation: C++ macros are not expanded. A macro-defined function
/// body will not have its call edges captured.
#[test]
fn golden_cpp_macro_limitation_documented() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.cpp",
        r#"
void real_func() {}

#define CALL_HIDDEN() real_func()

void user() {
    real_func();
}
"#,
    );
    assert!(has_symbol(&store, "real_func"), "real_func indexed");
    assert!(has_symbol(&store, "user"), "user indexed");
    let user_syms = store.search_symbols("user", None, None).unwrap();
    let callees = store.callees_of(user_syms[0].id, None).unwrap();
    assert!(
        callees.iter().any(|(n, _)| n == "real_func"),
        "direct call user → real_func captured"
    );
    let prec = mmcg::queries::lang_precision("test.cpp");
    assert_eq!(prec.confidence, "low", "C++ is low confidence");
    assert!(
        prec.limitations.contains(&"macros not expanded"),
        "macro limitation documented"
    );
}

/// Known limitation: Python dynamic attributes (setattr / getattr patterns)
/// are not tracked as call edges.
#[test]
fn golden_python_dynamic_attribute_limitation_documented() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "test.py",
        r#"
class Dispatcher:
    def dispatch(self, name):
        method = getattr(self, name)
        method()

    def handle_login(self):
        pass
"#,
    );
    assert!(has_symbol(&store, "dispatch"), "dispatch indexed");
    assert!(has_symbol(&store, "handle_login"), "handle_login indexed");
    let prec = mmcg::queries::lang_precision("test.py");
    assert!(
        prec.limitations.contains(&"dynamic attributes not tracked"),
        "dynamic attribute limitation documented"
    );
}

/// `search()` attaches precision metadata to each SymbolHit.
#[test]
fn search_result_carries_precision() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(&tmp, "test.rs", "fn check_precision() {}\n");
    let resp = mmcg::queries::search(&store, "check_precision", None, None, true).unwrap();
    assert!(!resp.results.is_empty(), "symbol found");
    let prec = resp.results[0]
        .precision
        .as_ref()
        .expect("precision field present");
    assert_eq!(prec.confidence, "high");
    assert_eq!(prec.resolution, "syntactic");
}

/// Edge precision structs carry the right confidence and resolution for each language.
#[test]
fn edge_precision_labels() {
    use mmcg::queries::lang_precision;

    let rs = lang_precision("src/main.rs");
    assert_eq!(rs.confidence, "high");
    assert_eq!(rs.resolution, "syntactic");

    let py = lang_precision("app/views.py");
    assert_eq!(py.confidence, "medium");
    assert_eq!(py.resolution, "heuristic");
    assert!(!py.limitations.is_empty());

    let cpp = lang_precision("engine/renderer.cpp");
    assert_eq!(cpp.confidence, "low");
    assert!(!cpp.limitations.is_empty());

    let go = lang_precision("server/main.go");
    assert_eq!(go.confidence, "high");
    assert!(go.limitations.is_empty());
}

#[test]
fn golden_tsx_components_and_jsx_usage() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "App.tsx",
        r#"
import { Button } from './Button';
import { Select } from './Select';

export const Card = ({ title }: { title: string }) => <article>{title}</article>;

export const Wrapped = memo(({ id }: { id: string }) => <Card title={id} />);

export function Screen() {
    return (
        <section>
            <Button variant="primary" />
            <Select.Option value="a" />
            <Wrapped id="x" />
        </section>
    );
}
"#,
    );

    assert!(has_symbol_kind(&store, "Card", "function"));
    assert!(has_symbol_kind(&store, "Wrapped", "function"));
    assert!(has_symbol_kind(&store, "Screen", "function"));

    assert!(has_call(&store, "Screen", "Button"));
    assert!(has_call(&store, "Screen", "Wrapped"));
    assert!(has_call(&store, "Wrapped", "Card"));

    assert!(has_call(&store, "Screen", "Option"));

    assert!(!has_call(&store, "Screen", "section"));
    assert!(!has_call(&store, "Card", "article"));

    assert!(has_import(&store, "App.tsx", "Button"));
}

#[test]
fn golden_jsx_in_plain_javascript() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "app.jsx",
        r#"
export const Row = () => <li />;

export function List() {
    return <ul><Row /></ul>;
}
"#,
    );

    assert!(has_symbol_kind(&store, "Row", "function"));
    assert!(has_call(&store, "List", "Row"));
    assert!(!has_call(&store, "List", "ul"));
}

#[test]
fn golden_vue_single_file_component() {
    let tmp = tempfile::tempdir().unwrap();
    let store = setup_store(
        &tmp,
        "MyCard.vue",
        r#"<template>
  <div class="card">
    <BaseButton label="ok" @click="bump" />
    <my-widget />
    <span>{{ count }}</span>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue';

function bump(step: number): void {
  void step;
}
</script>

<style scoped>.card { color: red }</style>
"#,
    );

    assert!(has_symbol_kind(&store, "MyCard", "component"));
    assert!(has_symbol_kind(&store, "bump", "function"));
    assert!(has_call(&store, "MyCard", "BaseButton"));
    assert!(has_call(&store, "MyCard", "MyWidget"));
    assert!(!has_call(&store, "MyCard", "div"));
    assert!(!has_call(&store, "MyCard", "span"));
    assert!(has_import(&store, "MyCard.vue", "ref"));
}
