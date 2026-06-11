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
