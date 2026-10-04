//! Source-client routing and model provenance at the public native boundary.
//! Disposable clients emulate inference only; no real account or LLM is used.
#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const NATIVE: &str = r#"#!/usr/bin/python3
import sys,json,os,pathlib,subprocess
client=pathlib.Path(sys.argv[0]).name
args=sys.argv[1:]
model=args[args.index('--model')+1]
request=json.load(sys.stdin)
with open(os.environ['MMCG_TEST_HOME']+'/calls.jsonl','a') as f:
 f.write(json.dumps({'client':client,'model':model,'episode':request.get('episode',{}).get('id'),'cwd':os.getcwd(),'home':os.environ['HOME'],'codex_home':os.environ['CODEX_HOME'],'miner':os.environ.get('MASTERMIND_MINER'),'nested_claude':os.environ.get('CLAUDECODE'),'args':args})+'\n')
# Inherited collection hooks must ignore the generated inference input.
hook={'session_id':'internal-miner','hook_event_name':'UserPromptSubmit','prompt':'I prefer a synthetic preference from the model.'}
env=dict(os.environ); env['HOME']=os.environ['MMCG_TEST_HOME']
p=subprocess.run([os.environ['MMCG_TEST_BIN'],'miner','hooks','receive','--client',client,'--project-root',os.environ['MMCG_TEST_ROOT']],input=json.dumps(hook),capture_output=True,text=True,env=env)
assert p.returncode==0 and json.loads(p.stdout)=={}
if 'episode' in request:
 answer={'schema':1,'episode_id':request['episode']['id'],'episode_revision':request['episode']['revision'],'drafts':[]}
else:
 i=request['input']
 answer={'schema':1,'intake_id':i['id'],'prompt_digest':i['prompt_digest'],'action':'passthrough','workflow_intent':'ordinary','intent_evidence':None,'refined_prompt':i['original'],'questions':[]}
text=json.dumps(answer)
if client=='claude':
 print(json.dumps({'type':'result','is_error':False,'result':text,'modelUsage':{model:{}}}))
else:
 print(json.dumps({'type':'thread.started','thread_id':'fake'}))
 print(json.dumps({'type':'turn.started'}))
 print(json.dumps({'type':'item.completed','item':{'type':'agent_message','text':text}}))
 print(json.dumps({'type':'turn.completed'}))
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        for dir in [&root, &home, &bin] {
            fs::create_dir(dir).unwrap();
        }
        let f = Self {
            _temp: temp,
            root: root.canonicalize().unwrap(),
            home,
            bin,
        };
        for client in ["claude", "codex"] {
            let executable = f.bin.join(client);
            fs::write(&executable, NATIVE).unwrap();
            fs::set_permissions(executable, fs::Permissions::from_mode(0o700)).unwrap();
            f.success(&["miner", "hooks", "setup", "--client", client, "--write"]);
        }
        f
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mmcg"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home)
            .env("CODEX_HOME", self.home.join("custom-codex"))
            .env("CLAUDECODE", "parent-session")
            .env("MMCG_TEST_BIN", env!("CARGO_BIN_EXE_mmcg"))
            .env("MMCG_TEST_ROOT", &self.root)
            .env("MMCG_TEST_HOME", &self.home)
            .args(args);
        command
    }

    fn success(&self, args: &[&str]) -> Value {
        parse(self.command(args).output().unwrap())
    }

    fn event(&self, client: &str, session: &str, turn: &str, kind: &str, extra: Value) -> Value {
        let mut value = json!({"session_id":session,"hook_event_name":kind,"cwd":self.root});
        if !turn.is_empty() {
            value["turn_id"] = json!(turn);
        }
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut child = self
            .command(&["miner", "hooks", "receive", "--client", client])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(value.to_string().as_bytes())
            .unwrap();
        parse(child.wait_with_output().unwrap())
    }

    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.home.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn automatic(&self, max_calls: u64) {
        let mut settings = mmcg::onboarding::Settings::local(&self.root);
        settings.clients = vec!["codex".into()];
        settings.mining = mmcg::onboarding::Mining::On;
        settings.provider = Some("native".into());
        settings.max_calls = max_calls;
        settings.max_runtime = 60;
        fs::create_dir_all(self.root.join(".mastermind")).unwrap();
        mmcg::onboarding::Session::begin(&self.root)
            .unwrap()
            .save(&settings)
            .unwrap();
    }

    fn worker(&self) -> Value {
        self.success(&["miner", "hooks", "worker", "status", "--client", "codex"])
    }

    fn terminal(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let report = self.worker();
            if report["owner"] == "available" && report["status"] != "starting" {
                return report;
            }
            assert!(Instant::now() < deadline, "{report}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for client in ["codex", "claude"] {
            let _ = self
                .command(&["miner", "hooks", "worker", "stop", "--client", client])
                .output();
        }
    }
}

fn parse(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn native_mining_routes_each_episode_to_its_client_and_captured_model() {
    let f = Fixture::new();
    for (client, model) in [("codex", "gpt-task-model"), ("claude", "claude-task-model")] {
        f.event(
            client,
            client,
            "",
            "SessionStart",
            json!({"source":"startup","model":model}),
        );
        f.event(
            client,
            client,
            "one",
            "UserPromptSubmit",
            json!({"prompt":"Inspect the callers before editing."}),
        );
        f.event(client, client, "one", "Stop", json!({}));
    }
    let mined = f.success(&["miner", "hooks", "mine", "--provider", "native"]);
    assert_eq!(mined["results"].as_array().unwrap().len(), 2);
    assert_eq!(mined["failed"], false);
    let calls = f.calls();
    assert_eq!(calls.len(), 2);
    for call in &calls {
        let client = call["client"].as_str().unwrap();
        assert_eq!(
            call["model"],
            if client == "codex" {
                "gpt-task-model"
            } else {
                "claude-task-model"
            }
        );
        assert_eq!(call["miner"], "1");
        assert!(call["nested_claude"].is_null());
        if client == "claude" {
            assert_eq!(call["home"], f.home.to_str().unwrap());
        } else {
            assert_eq!(call["home"], call["cwd"]);
        }
        assert_eq!(
            call["codex_home"],
            f.home.join("custom-codex").to_str().unwrap()
        );
        assert_ne!(call["cwd"], f.root.to_str().unwrap());
        assert!(!PathBuf::from(call["cwd"].as_str().unwrap()).exists());
        let argv = call["args"].as_array().unwrap();
        assert!(argv.iter().any(|arg| arg
            == if client == "codex" {
                "--ignore-user-config"
            } else {
                "--safe-mode"
            }));
        assert!(!argv.iter().any(|arg| arg == "--bare"));
    }
    assert!(
        f.success(&["miner", "hooks", "mine", "--provider", "native"])["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.calls().len(),
        2,
        "completed revisions do not call the model again"
    );
    let episodes = f.success(&["miner", "hooks", "episodes"]);
    assert_eq!(
        episodes["episodes"].as_array().unwrap().len(),
        2,
        "internal inference never becomes user evidence"
    );
    for episode in episodes["episodes"].as_array().unwrap() {
        let show = f.success(&["miner", "hooks", "show", episode["id"].as_str().unwrap()]);
        assert_eq!(show["analyses"][0]["current"], true);
        assert_eq!(
            show["analyses"][0]["processor"]["provider"],
            show["episode"]["client"]
        );
        assert_eq!(
            show["analyses"][0]["processor"]["model"],
            show["episode"]["model"]
        );
        assert_eq!(
            show["analyses"][0]["processor"]["model_source"],
            "native_hook"
        );
    }
}

#[test]
fn model_switch_applies_to_the_next_turn_without_rewriting_the_previous_model() {
    let f = Fixture::new();
    f.event(
        "claude",
        "switch",
        "",
        "SessionStart",
        json!({"source":"startup","model":"claude-before"}),
    );
    f.event(
        "claude",
        "switch",
        "one",
        "UserPromptSubmit",
        json!({"prompt":"Inspect the callers before editing."}),
    );
    f.event("claude", "switch", "one", "Stop", json!({}));
    f.event(
        "claude",
        "switch",
        "",
        "PostModelSwitch",
        json!({"to_model":"claude-after"}),
    );
    f.event(
        "claude",
        "switch",
        "two",
        "UserPromptSubmit",
        json!({"prompt":"Inspect the callers before editing."}),
    );
    f.event("claude", "switch", "two", "Stop", json!({}));
    assert_eq!(
        f.success(&["miner", "hooks", "mine", "--provider", "native"])["failed"],
        false
    );
    let mut models: Vec<String> = f
        .calls()
        .iter()
        .map(|c| c["model"].as_str().unwrap().into())
        .collect();
    models.sort();
    assert_eq!(models, ["claude-after", "claude-before"]);
}

#[test]
fn unknown_models_and_cross_client_overrides_never_start_inference() {
    let f = Fixture::new();
    f.event(
        "codex",
        "unknown",
        "",
        "SessionStart",
        json!({"source":"startup"}),
    );
    f.event(
        "codex",
        "unknown",
        "one",
        "UserPromptSubmit",
        json!({"prompt":"Inspect the callers before editing."}),
    );
    f.event("codex", "unknown", "one", "Stop", json!({}));
    let result = f.success(&["miner", "hooks", "mine", "--provider", "native"]);
    assert_eq!(result["skipped"][0]["reason"], "native_model_not_captured");
    f.event(
        "codex",
        "known",
        "",
        "SessionStart",
        json!({"source":"startup","model":"gpt-task-model"}),
    );
    f.event(
        "codex",
        "known",
        "one",
        "UserPromptSubmit",
        json!({"prompt":"Inspect the callers before editing."}),
    );
    f.event("codex", "known", "one", "Stop", json!({}));
    let result = f
        .command(&["miner", "hooks", "mine", "--provider", "claude"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("must match"));
    assert!(f.calls().is_empty());
}

#[test]
fn prompt_refinement_uses_the_same_native_client_and_model() {
    let f = Fixture::new();
    f.success(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "codex",
        "--refiner-provider",
        "native",
        "--write",
    ]);
    f.event(
        "codex",
        "refine",
        "",
        "SessionStart",
        json!({"source":"startup","model":"gpt-refiner-model"}),
    );
    f.event(
        "codex",
        "refine",
        "one",
        "UserPromptSubmit",
        json!({"prompt":"Inspect the callers before editing."}),
    );
    let calls = f.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["client"], "codex");
    assert_eq!(calls[0]["model"], "gpt-refiner-model");
}

#[test]
fn native_session_automatically_mines_and_only_a_new_session_renews_its_budget() {
    let f = Fixture::new();
    f.automatic(1);
    let mut prior_run = Value::Null;
    for session in ["first", "second"] {
        if !prior_run.is_null() {
            // Resume revised the preceding episode. Checkpoint that history
            // before testing the next run's single-call budget on its new turn.
            let previous = f.success(&["miner", "hooks", "mine", "--provider", "native"]);
            assert_eq!(previous["failed"], false);
            assert_eq!(previous["results"].as_array().unwrap().len(), 1);
        }
        let previous_episodes = f.success(&["miner", "hooks", "episodes"])["episodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|episode| episode["id"].clone())
            .collect::<Vec<_>>();
        let previous_calls = f.calls().len();
        f.event(
            "codex",
            session,
            "",
            "SessionStart",
            json!({"source":"startup","model":"gpt-auto-model"}),
        );
        let running = f.worker();
        assert_eq!(running["autostart"], true);
        assert_ne!(running["run"]["run_id"], prior_run);
        prior_run = running["run"]["run_id"].clone();
        f.event(
            "codex",
            session,
            "one",
            "UserPromptSubmit",
            json!({"prompt":"Inspect the callers before editing."}),
        );
        f.event("codex", session, "one", "Stop", json!({}));
        let terminal = f.terminal();
        assert_eq!(terminal["status"], "budget_exhausted");
        assert_eq!(terminal["run"]["attempts"], 1);
        assert_eq!(terminal["run"]["completed"], 1, "{session}: {terminal}");
        let calls = f.calls();
        assert_eq!(calls.len(), previous_calls + 1);
        let episodes = f.success(&["miner", "hooks", "episodes"]);
        let current_episode = episodes["episodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|episode| !previous_episodes.contains(&episode["id"]))
            .unwrap();
        assert_eq!(calls.last().unwrap()["episode"], current_episode["id"]);
        f.event(
            "codex",
            session,
            "",
            "SessionStart",
            json!({"source":"resume","model":"gpt-auto-model"}),
        );
        assert_eq!(
            f.worker()["run"]["run_id"],
            prior_run,
            "resume cannot renew the spent budget"
        );
    }
    assert_eq!(f.calls().len(), 3); // Two automatic calls and one history checkpoint.
}

#[test]
fn explicit_stop_survives_a_new_native_session() {
    let f = Fixture::new();
    f.automatic(2);
    f.event(
        "codex",
        "first",
        "",
        "SessionStart",
        json!({"source":"startup","model":"gpt-auto-model"}),
    );
    let run = f.worker()["run"]["run_id"].clone();
    f.success(&["miner", "hooks", "worker", "stop", "--client", "codex"]);
    assert_eq!(f.terminal()["status"], "stopped");
    f.event(
        "codex",
        "second",
        "",
        "SessionStart",
        json!({"source":"startup","model":"gpt-auto-model"}),
    );
    assert_eq!(f.worker()["run"]["run_id"], run);
    assert_eq!(f.worker()["autostart"], false);
    assert!(f.calls().is_empty());
}

#[test]
fn an_automated_native_prompt_is_recorded_without_human_evidence() {
    let f = Fixture::new();
    f.event(
        "codex",
        "automation",
        "",
        "SessionStart",
        json!({"source":"startup","model":"gpt-auto-model"}),
    );
    let payload = json!({"session_id":"automation","turn_id":"one","hook_event_name":"UserPromptSubmit","cwd":f.root,"prompt":"I prefer short reviews only for simple changes."});
    let mut child = f
        .command(&["miner", "hooks", "receive", "--client", "codex"])
        .env("MMCG_INPUT_ORIGIN", "automation")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    parse(child.wait_with_output().unwrap());
    f.event("codex", "automation", "one", "Stop", json!({}));
    f.success(&["miner", "hooks", "mine-local"]);
    let episodes = f.success(&["miner", "hooks", "episodes"]);
    let id = episodes["episodes"][0]["id"].as_str().unwrap();
    let shown = f.success(&["miner", "hooks", "show", id]);
    assert_eq!(
        shown["episode"]["events"][0]["origin"],
        "automation_or_agent"
    );
    assert!(shown["drafts"].as_array().unwrap().is_empty());
    assert!(f.calls().is_empty());
}
