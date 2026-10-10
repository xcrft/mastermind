use mmcg::{indexer::Indexer, store::Store};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Session {
    process: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    id: u32,
}

impl Session {
    fn new(root: &std::path::Path) -> Self {
        let mut process = Command::new(env!("CARGO_BIN_EXE_mmcg"))
            .args([
                "--index",
                root.join("graph.db").to_str().unwrap(),
                "serve",
                "--root",
                root.to_str().unwrap(),
            ])
            .current_dir(root)
            .env("MMCG_QUERY_BUDGET_MS", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = process.stdin.take().unwrap();
        let output = BufReader::new(process.stdout.take().unwrap());
        let mut session = Self {
            process,
            input,
            output,
            id: 0,
        };
        session.request("initialize", json!({"protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"source-test", "version":"1"}}));
        writeln!(
            session.input,
            "{}",
            json!({"jsonrpc":"2.0", "method":"notifications/initialized"})
        )
        .unwrap();
        session.input.flush().unwrap();
        session
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        writeln!(
            self.input,
            "{}",
            json!({"jsonrpc":"2.0", "id":self.id, "method":method, "params":params})
        )
        .unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        assert!(
            self.output.read_line(&mut line).unwrap() > 0,
            "server exited before replying"
        );
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], self.id);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn read(&mut self, args: Value) -> Value {
        let result = self.request("tools/call", json!({"name":"mmcg_read", "arguments":args}));
        assert_ne!(result["isError"], true, "{result}");
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn fixture(text: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("lib.rs"), text).unwrap();
    let mut store = Store::open(root.path().join("graph.db")).unwrap();
    Indexer::new(root.path())
        .index_all(&mut store, true)
        .unwrap();
    root
}

#[test]
fn receipt_reuses_only_delivered_text_and_falls_back_across_bindings_and_edits() {
    let root = fixture("one\ntwo\nthree\nfour\nfive\nsix\n");
    std::fs::write(root.path().join("other.txt"), "other\n").unwrap();
    let mut server = Session::new(root.path());
    let first =
        server.read(json!({"file":"lib.rs", "task":"review", "start_line":1, "end_line":3}));
    assert_eq!(first["segments"][0]["lines"].as_array().unwrap().len(), 3);
    let second = server.read(json!({"file":"lib.rs", "task":"review", "start_line":3, "end_line":6, "previous_receipt":first["receipt"]}));
    assert_eq!(
        second["reused_ranges"],
        json!([{"start_line":3,"end_line":3}])
    );
    assert_eq!(second["segments"][0]["start_line"], 4);
    assert_eq!(
        second["segments"][0]["lines"][0],
        json!({"line":4,"text":"four"})
    );
    let repeat = server.read(json!({"file":"lib.rs", "task":"review", "end_line":6, "previous_receipt":second["receipt"]}));
    assert_eq!(repeat["segments"], json!([]));
    assert_eq!(
        repeat["reused_ranges"],
        json!([{"start_line":1,"end_line":6}])
    );
    assert_eq!(repeat["range_truncated"], false);
    for (file, task) in [("lib.rs", "other-task"), ("other.txt", "review")] {
        let fresh =
            server.read(json!({"file":file, "task":task, "previous_receipt":repeat["receipt"]}));
        assert_eq!(fresh["reuse_status"], "binding_mismatch");
        assert_eq!(fresh["reused_ranges"], json!([]));
        assert!(!fresh["segments"].as_array().unwrap().is_empty());
    }
    std::fs::write(
        root.path().join("lib.rs"),
        "ONE\ntwo\nthree\nfour\nfive\nsix\n",
    )
    .unwrap();
    let changed = server.read(json!({"file":"lib.rs", "task":"review", "end_line":6, "previous_receipt":repeat["receipt"]}));
    assert_eq!(changed["reuse_status"], "source_changed");
    assert_ne!(changed["source_sha256"], first["source_sha256"]);
    assert_eq!(changed["segments"][0]["lines"][0]["text"], "ONE");
    let mut restarted = Session::new(root.path());
    let fresh = restarted
        .read(json!({"file":"lib.rs", "task":"review", "previous_receipt":changed["receipt"]}));
    assert_eq!(fresh["reuse_status"], "receipt_unavailable");
    assert!(!fresh["segments"].as_array().unwrap().is_empty());
}

#[test]
fn pagination_counts_new_lines_and_preserves_noncontiguous_citations() {
    let text = (1..=450)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    let root = fixture(&text);
    let mut server = Session::new(root.path());
    let middle =
        server.read(json!({"file":"lib.rs", "task":"review", "start_line":51,"end_line":100}));
    let page = server.read(json!({"file":"lib.rs", "task":"review", "end_line":450, "previous_receipt":middle["receipt"]}));
    assert_eq!(page["segments"].as_array().unwrap().len(), 2);
    assert_eq!(page["segments"][0]["start_line"], 1);
    assert_eq!(page["segments"][0]["end_line"], 50);
    assert_eq!(page["segments"][1]["start_line"], 101);
    assert_eq!(page["segments"][1]["end_line"], 250);
    assert_eq!(page["next_line"], 251);
    assert_eq!(page["range_truncated"], true);
    let tail = server.read(json!({"file":"lib.rs", "task":"review", "end_line":999, "previous_receipt":page["receipt"]}));
    assert_eq!(tail["segments"][0]["start_line"], 251);
    assert_eq!(tail["segments"][0]["end_line"], 450);
    assert_eq!(tail["range_truncated"], false);
    assert_eq!(tail["next_line"], Value::Null);
    let full = server.read(json!({"file":"lib.rs", "task":"review", "end_line":450}));
    assert_eq!(full["reuse_status"], "not_requested");
    assert_eq!(full["next_line"], 201);
    assert_eq!(full["segments"][0]["lines"][0]["line"], 1);
}

#[test]
fn bounded_receipts_forget_ranges_without_omitting_their_text() {
    let text = (1..=257)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    let root = fixture(&text);
    let mut server = Session::new(root.path());
    let mut previous = Value::Null;
    let mut first = Value::Null;
    for line in (1..=257).step_by(2) {
        let mut args =
            json!({"file":"lib.rs", "task":"review", "start_line":line, "end_line":line});
        if !previous.is_null() {
            args["previous_receipt"] = previous["receipt"].clone();
        }
        previous = server.read(args);
        assert_eq!(previous["segments"][0]["lines"][0]["line"], line);
        if line == 1 {
            first = previous["receipt"].clone();
        }
    }
    assert_eq!(previous["receipt_ranges"].as_array().unwrap().len(), 128);
    assert_eq!(previous["receipt_coverage_truncated"], true);
    let forgotten = server.read(json!({"file":"lib.rs", "task":"review", "start_line":257, "end_line":257, "previous_receipt":previous["receipt"]}));
    assert_eq!(forgotten["reuse_status"], "applied");
    assert_eq!(forgotten["segments"][0]["lines"][0]["text"], "line 257");
    let evicted = server.read(json!({"file":"lib.rs", "task":"review", "start_line":1, "end_line":1, "previous_receipt":first}));
    assert_eq!(evicted["reuse_status"], "receipt_unavailable");
    assert_eq!(evicted["segments"][0]["lines"][0]["text"], "line 1");
}

#[test]
fn unsafe_paths_encodings_and_ranges_fail_without_disclosing_source() {
    let root = fixture("fn safe() {}\n");
    std::fs::write(root.path().join(".secret"), "private source\n").unwrap();
    std::fs::create_dir(root.path().join(".hidden")).unwrap();
    std::fs::write(root.path().join(".hidden/open.txt"), "private source\n").unwrap();
    std::fs::write(root.path().join("empty.txt"), "").unwrap();
    std::fs::write(root.path().join("bad.txt"), [0xff, 0xfe]).unwrap();
    std::fs::write(root.path().join("large.txt"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    let mut server = Session::new(root.path());
    let empty = server.read(json!({"file":"empty.txt", "task":"review"}));
    assert_eq!(empty["total_lines"], 0);
    assert_eq!(empty["segments"], json!([]));
    for args in [
        json!({"file":"../lib.rs", "task":"review"}),
        json!({"file":"./lib.rs", "task":"review"}),
        json!({"file":".secret", "task":"review"}),
        json!({"file":".hidden/open.txt", "task":"review"}),
        json!({"file":"bad.txt", "task":"review"}),
        json!({"file":"large.txt", "task":"review"}),
        json!({"file":"lib.rs", "task":"review", "start_line":2}),
        json!({"file":"lib.rs", "task":"review", "start_line":1,"end_line":0}),
        json!({"file":"lib.rs", "task":"review", "previous_receipt":"made-up"}),
    ] {
        let result = server.request("tools/call", json!({"name":"mmcg_read", "arguments":args}));
        assert_eq!(result["isError"], true, "{result}");
        assert!(!result.to_string().contains("fn safe"));
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.path().join("lib.rs"), root.path().join("alias.rs"))
            .unwrap();
        let result = server.request(
            "tools/call",
            json!({"name":"mmcg_read", "arguments":{"file":"alias.rs", "task":"review"}}),
        );
        assert_eq!(result["isError"], true);
    }
}
