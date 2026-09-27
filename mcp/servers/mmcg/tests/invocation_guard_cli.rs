#![cfg(unix)]

//! Public CLI contract tests. The fake client honors actual guard decisions;
//! the missing-hook case intentionally demonstrates the native fallback limit.
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const SPEC: &str = ".mastermind/tasks/001-guard/spec.md";
const RECEIPT: &str = ".mastermind/tasks/001-guard/invocation.json";
const MANIFEST: &str = ".mastermind/tasks/001-guard/invocation.guard.json";
const LEDGER: &str = ".mastermind/tasks/001-guard/invocation.guard-decisions.json";

const FAKE: &str = r#"#!/usr/bin/python3
import json, os, pathlib, subprocess, sys
args = sys.argv[1:]
if args == ['--version']:
    print('2.1.267 (Claude Code)'); sys.exit(0)
if args == ['--help']:
    print('--input-format text --output-format stream-json --verbose --tools --permission-mode acceptEdits dontAsk --permission-prompts none --no-session-persistence --no-chrome --restricted --setting-sources --settings --strict-mcp-config --mcp-config --disable-slash-commands --session-id'); sys.exit(0)
h = pathlib.Path(os.environ['HARNESS'])
mode = (h / 'mode').read_text()
raw = sys.stdin.read()
(h / 'stdin').write_text(raw)
(h / 'argv.json').write_text(json.dumps(args))
session = args[args.index('--session-id') + 1]
settings = json.loads(args[args.index('--settings') + 1])
handler = settings['hooks']['PreToolUse'][0]['hooks'][0]
hook = [handler['command']] + handler['args']
root = pathlib.Path.cwd()
manifest_path = pathlib.Path(hook[-1])
manifest = json.loads(manifest_path.read_text())
verify_command = next(iter(manifest['commands']))
tools = ['Read', 'Edit', 'Write', 'Grep', 'Glob', 'Bash']
def emit(v):
    print(json.dumps(v), flush=True)
emit(dict(type='system', subtype='init', cwd=str(root), permissionMode='dontAsk', claude_code_version='2.1.267', model='synthetic-model', session_id=session, tools=tools, mcp_servers=[], plugins=[], skills=[]))
denials = []
def authorize(i, name, data, alter=None, emit_call=True):
    if emit_call:
        emit(dict(type='assistant', session_id=session, message=dict(role='assistant', content=[dict(type='tool_use', id=i, name=name, input=data)])))
    event = dict(hook_event_name='PreToolUse', cwd=str(root), session_id=session, tool_use_id=i, tool_name=name, tool_input=data)
    if alter:
        event.update(alter)
    (h / 'event.json').write_text(json.dumps(event))
    p = subprocess.run(hook, input=json.dumps(event), text=True, capture_output=True)
    (h / ('hook-' + i + '.txt')).write_text(p.stdout + p.stderr)
    allowed = p.returncode == 0 and json.loads(p.stdout)['hookSpecificOutput']['permissionDecision'] == 'allow'
    if not allowed:
        denials.append(dict(tool_use_id=i, tool_name=name))
    return allowed

body = 'def keep():\n    return 2  # PRIVATE_TOOL_PAYLOAD\n'
write = dict(file_path=str(root / 'service.py'), content=body)
if mode in ['protected', 'outside', 'shell', 'shell_suffix', 'background', 'unknown_mcp', 'bad_session', 'stale_state', 'symlink', 'hardlink', 'duplicate_conflict', 'missing_hook', 'input_mismatch', 'missing_ledger']:
    if mode == 'protected':
        data = dict(file_path=str(root / '.mastermind/tasks/001-guard/spec.md'), content='REPLACED')
        if authorize('blocked', 'Write', data): pathlib.Path(data['file_path']).write_text(data['content'])
    elif mode == 'outside':
        data = dict(file_path=str(root.parent / 'outside.txt'), content='REPLACED')
        if authorize('blocked', 'Write', data): pathlib.Path(data['file_path']).write_text(data['content'])
    elif mode in ['shell', 'shell_suffix', 'background']:
        cmd = '/bin/touch SHOULD_NOT_EXIST' if mode == 'shell' else verify_command + ' && /bin/touch SHOULD_NOT_EXIST' if mode == 'shell_suffix' else verify_command
        data = dict(command=cmd)
        if mode == 'background': data['run_in_background'] = True
        if authorize('blocked', 'Bash', data): subprocess.run(cmd, shell=True)
    elif mode == 'unknown_mcp':
        data = dict(path='service.py')
        authorize('blocked', 'mcp__unknown__write', data, emit_call=False)
        emit(dict(type='assistant', session_id=session, message=dict(role='assistant', content=[dict(type='tool_use', id='blocked', name='mcp__unknown__write', input=data)])))
    elif mode == 'bad_session':
        if authorize('blocked', 'Write', write, dict(session_id='wrong-session')): (root / 'service.py').write_text(body)
    elif mode == 'stale_state':
        state_path = root / '.mastermind/tasks/001-guard/state.json'
        state = json.loads(state_path.read_text()); state['iteration'] += 1; state_path.write_text(json.dumps(state))
        if authorize('blocked', 'Write', write): (root / 'service.py').write_text(body)
    elif mode in ['symlink', 'hardlink']:
        source = root / 'service.py'; source.unlink()
        victim = h / 'victim.py'
        if mode == 'symlink': source.symlink_to(victim)
        else: os.link(victim, source)
        if authorize('blocked', 'Write', write): source.write_text(body)
    elif mode == 'duplicate_conflict':
        authorize('same', 'Write', write)
        changed = dict(write, content='def keep():\n    return 3\n')
        if authorize('same', 'Write', changed, emit_call=False): (root / 'service.py').write_text(changed['content'])
    elif mode == 'missing_hook':
        emit(dict(type='assistant', session_id=session, message=dict(role='assistant', content=[dict(type='tool_use', id='unmediated', name='Write', input=write)])))
        # Deliberate fake native fallback: an effect occurred without our hook.
        (root / 'service.py').write_text(body)
    elif mode == 'input_mismatch':
        authorize('different', 'Write', write, emit_call=False)
        changed = dict(write, content='def keep():\n    return 3\n')
        emit(dict(type='assistant', session_id=session, message=dict(role='assistant', content=[dict(type='tool_use', id='different', name='Write', input=changed)])))
    elif mode == 'missing_ledger':
        authorize('write', 'Write', write)
        (root / '.mastermind/tasks/001-guard/invocation.guard-decisions.json').unlink()
else:
    assert authorize('read', 'Read', dict(file_path=str(root / '.mastermind/tasks/001-guard/spec.md')))
    if authorize('write', 'Write', write): (root / 'service.py').write_text(body)
    if mode == 'duplicate_delivery': assert authorize('write', 'Write', write, emit_call=False)
    code = 99
    if authorize('verify', 'Bash', dict(command=verify_command, run_in_background=False)):
        p = subprocess.run(verify_command, shell=True, text=True, capture_output=True)
        (h / 'verification.json').write_text(p.stdout + p.stderr)
        code = p.returncode
    report = dict(schema_version=1, spec='.mastermind/tasks/001-guard/spec.md', status='complete' if code == 0 else 'partial', phases=[], files_modified=['service.py'], claims=[], defects=[], verifications=[dict(cmd='./.mastermind/check.sh', result='pass' if code == 0 else 'fail', observed=dict(exit_code=code))])
    data = dict(file_path=str(root / '.mastermind/tasks/001-guard/executor-report.md'), content=json.dumps(report))
    if authorize('report', 'Write', data): pathlib.Path(data['file_path']).write_text(data['content'])
emit(dict(type='result', subtype='success', is_error=False, session_id=session, permission_denials=denials, result='PRIVATE_NATIVE_RESULT'))
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    harness: PathBuf,
    bin: PathBuf,
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "status {:?}\n{}\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project with ' quotes");
        let home = temp.path().join("home");
        let harness = temp.path().join("harness");
        let bin = temp.path().join("bin");
        for path in [&root, &home, &harness, &bin] {
            fs::create_dir_all(path).unwrap();
        }
        let f = Self {
            root: root.canonicalize().unwrap(),
            home,
            harness,
            bin,
            _temp: temp,
        };
        f.write(".gitignore", ".mastermind/\n.claude/\n");
        f.write("service.py", "def keep():\n    return 1\n");
        f.write(
            "CONTEXT.md",
            "# Project\n\n## Runtime\nAn explicit small service.\n",
        );
        for args in [
            vec!["init", "-q", "--initial-branch=main"],
            vec!["config", "user.name", "Guard fixture"],
            vec!["config", "user.email", "guard@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "core.hooksPath", ""],
            vec!["add", "."],
            vec!["commit", "-qm", "Synthetic baseline"],
            vec!["tag", "baseline"],
        ] {
            success(&f.command("/usr/bin/git").args(args).output().unwrap());
        }
        let check_id = if mode == "dash_check" {
            "-unit"
        } else {
            "unit"
        };
        let frontmatter = json!({"mode":"verified","title":"Service behavior", "touches":[{"file":"service.py","symbols":["keep"]}],
            "verify":[{"cmd":"./.mastermind/check.sh","run":{"id":check_id,"argv":["./.mastermind/check.sh"],"cwd":".","timeout_secs":10}}],
            "acceptance":[{"id":"return-two","statement":"The service returns two.","checks":[check_id]}]});
        f.write(SPEC, &format!("---\n{}---\n# Guard fixture\n\n## Goals\nUpdate the service.\n\n## Scope\nEdit service.py.\n\n## Acceptance Criteria\nThe service returns two.\n\n## Tests Plan\nRun the declared check.\n\n## Final Verification\nRun the declared command.\n\n## Alternatives Considered\nKeep the same service.\n\n## Risk Register\nLocal behavior only.\n\n## Evidence Ledger\nUse observed evidence.\n\n## Documentation Plan\nNo public documentation change.\n\n## Observability Plan\nNo logs needed.\n\n## Performance Considerations\nConstant work.\n\n## Rollback / Migration\nRestore the return value.\n", serde_norway::to_string(&frontmatter).unwrap()));
        f.write(
            ".mastermind/check.sh",
            "#!/bin/sh\n/usr/bin/grep -q 'return 2' service.py\n",
        );
        fs::set_permissions(
            f.root.join(".mastermind/check.sh"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::write(f.bin.join("claude"), FAKE).unwrap();
        fs::set_permissions(f.bin.join("claude"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(f.harness.join("mode"), mode).unwrap();
        fs::write(f.harness.join("victim.py"), "PRIVATE_VICTIM_UNCHANGED").unwrap();
        f.write(
            ".claude/settings.local.json",
            "{\"permissions\":{\"allow\":[\"Bash\",\"Write\"]}}\n",
        );
        success(&f.cli(&["index", "."]).output().unwrap());
        f
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.bin.display()),
            )
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", self.home.join("codex"))
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_COUNT", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("MMCG_GIT_TIMEOUT_MS", "20000")
            .env("HARNESS", &self.harness);
        command
    }
    fn cli(&self, args: &[&str]) -> Command {
        let mut command = self.command(env!("CARGO_BIN_EXE_mmcg"));
        command.args(["--index", ".mastermind/index.db"]).args(args);
        command
    }
    fn execute(&self) -> Output {
        self.cli(&[
            "run-task",
            SPEC,
            "--exec",
            "--guarded-exec",
            "--exec-timeout",
            "30",
        ])
        .output()
        .unwrap()
    }
    fn write(&self, path: &str, value: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
    fn json(&self, path: &str) -> Value {
        serde_json::from_slice(&fs::read(self.root.join(path)).unwrap()).unwrap()
    }
    fn assert_success(&self, output: &Output) {
        assert!(
            output.status.success(),
            "status {:?}\n{}\n{}\nreceipt: {}\nhook outputs: {:?}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            fs::read_to_string(self.root.join(RECEIPT)).unwrap_or_default(),
            fs::read_dir(&self.harness)
                .unwrap()
                .filter_map(|entry| {
                    let path = entry.ok()?.path();
                    path.file_name()?
                        .to_str()?
                        .starts_with("hook-")
                        .then(|| fs::read_to_string(path).unwrap_or_default())
                })
                .collect::<Vec<_>>()
        );
    }
    fn assert_started(&self, mode: &str) {
        assert!(
            self.harness.join("argv.json").exists(),
            "native client never started for {mode}: {}",
            fs::read_to_string(self.root.join(RECEIPT)).unwrap_or_default()
        );
    }
}

#[test]
fn guarded_executor_reconciles_exact_calls_and_keeps_payloads_private() {
    for mode in ["positive", "duplicate_delivery", "dash_check"] {
        let f = Fixture::new(mode);
        let output = f.execute();
        f.assert_success(&output);
        let receipt = f.json(RECEIPT);
        assert_eq!(receipt["schema_version"], 2);
        assert_eq!(receipt["status"], "passed");
        assert_eq!(receipt["policy"]["permission_mode"], "dontAsk");
        assert_eq!(receipt["mediation"]["observed_calls"], 4);
        assert_eq!(receipt["mediation"]["allowed_calls"], 4);
        assert_eq!(receipt["mediation"]["denied_calls"], 0);
        assert_eq!(receipt["mediation"]["reconciled"], true);
        assert_eq!(
            receipt["mediation"]["coverage"],
            "observed_native_tool_use_only"
        );
        assert_eq!(
            f.json(".mastermind/tasks/001-guard/state.json")["status"],
            "history_review_required"
        );
        let check_id = if mode == "dash_check" {
            "-unit"
        } else {
            "unit"
        };
        assert_eq!(
            f.json(&format!(
                ".mastermind/tasks/001-guard/verification/{check_id}.json"
            ))["status"],
            "passed"
        );
        let argv: Vec<String> =
            serde_json::from_slice(&fs::read(f.harness.join("argv.json")).unwrap()).unwrap();
        for flag in [
            "--restricted",
            "--strict-mcp-config",
            "--disable-slash-commands",
        ] {
            assert!(argv.contains(&flag.into()));
        }
        assert!(!argv.contains(&"--safe-mode".into()));
        assert_eq!(
            argv[argv.iter().position(|v| v == "--setting-sources").unwrap() + 1],
            ""
        );
        for path in [RECEIPT, MANIFEST, LEDGER] {
            let bytes = fs::read_to_string(f.root.join(path)).unwrap();
            for secret in [
                "PRIVATE_TOOL_PAYLOAD",
                "PRIVATE_NATIVE_RESULT",
                "PRIVATE_VICTIM_UNCHANGED",
            ] {
                assert!(!bytes.contains(secret), "{path} retained raw content");
            }
            assert_eq!(
                fs::metadata(f.root.join(path))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(!f.home.join(".mastermind/style.db").exists());
    }
}

#[test]
fn guarded_executor_denies_protected_outside_alias_and_unknown_actions_before_effects() {
    for mode in [
        "protected",
        "outside",
        "shell",
        "shell_suffix",
        "background",
        "unknown_mcp",
        "symlink",
        "hardlink",
    ] {
        let f = Fixture::new(mode);
        let spec = fs::read(f.root.join(SPEC)).unwrap();
        let output = f.execute();
        assert!(!output.status.success(), "unexpected success for {mode}");
        f.assert_started(mode);
        let receipt = f.json(RECEIPT);
        assert_ne!(receipt["status"], "passed", "{mode}");
        assert_eq!(
            receipt["mediation"]["observed_calls"], 1,
            "{mode}: {receipt}"
        );
        assert_eq!(receipt["mediation"]["denied_calls"], 1, "{mode}: {receipt}");
        assert_eq!(
            receipt["mediation"]["allowed_calls"], 0,
            "{mode}: {receipt}"
        );
        assert_eq!(fs::read(f.root.join(SPEC)).unwrap(), spec);
        assert!(!f.root.parent().unwrap().join("outside.txt").exists());
        assert!(!f.root.join("SHOULD_NOT_EXIST").exists());
        assert_eq!(
            fs::read_to_string(f.harness.join("victim.py")).unwrap(),
            "PRIVATE_VICTIM_UNCHANGED"
        );
        if !["symlink", "hardlink"].contains(&mode) {
            assert!(fs::read_to_string(f.root.join("service.py"))
                .unwrap()
                .contains("return 1"));
        }
    }
}

#[test]
fn guarded_executor_refuses_missing_mismatched_and_conflicting_mediation() {
    for mode in [
        "missing_hook",
        "input_mismatch",
        "duplicate_conflict",
        "bad_session",
        "stale_state",
        "missing_ledger",
    ] {
        let f = Fixture::new(mode);
        assert!(
            !f.execute().status.success(),
            "unexpected success for {mode}"
        );
        f.assert_started(mode);
        let receipt = f.json(RECEIPT);
        assert_ne!(receipt["status"], "passed", "{mode}");
        assert_ne!(receipt["mediation"]["reconciled"], true);
        assert_eq!(
            receipt["mediation"]["observed_calls"], 1,
            "{mode}: {receipt}"
        );
        if mode == "missing_ledger" {
            assert!(receipt["mediation"]["allowed_calls"].is_null());
            assert!(receipt["mediation"]["denied_calls"].is_null());
        }
        if mode == "missing_hook" {
            // Honest boundary: missing mediation withholds the receipt even
            // though an uncooperative native client has already made an edit.
            assert!(fs::read_to_string(f.root.join("service.py"))
                .unwrap()
                .contains("return 2"));
            assert_eq!(receipt["reason"], "invocation_guard_mediation_incomplete");
            assert_eq!(receipt["mediation"]["allowed_calls"], 0);
            assert_eq!(receipt["mediation"]["denied_calls"], 0);
        }
    }
}

#[test]
fn guarded_executor_rejects_receipt_replay_and_changed_decision_artifacts() {
    let f = Fixture::new("positive");
    f.assert_success(&f.execute());
    let mut child = f
        .cli(&["invocation", "guard", "--manifest"])
        .arg(f.root.join(MANIFEST))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&fs::read(f.harness.join("event.json")).unwrap())
        .ok(); // A completed invocation may reject before it reads stdin.
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let mut ledger = f.json(LEDGER);
    ledger["decisions"]["write"]["input_sha256"] = json!("0".repeat(64));
    f.write(LEDGER, &ledger.to_string());
    let output = f.cli(&["run-task", SPEC, "--post-only"]).output().unwrap();
    assert!(!output.status.success());
    assert_ne!(
        f.json(".mastermind/tasks/001-guard/state.json")["status"],
        "learned"
    );
}

#[test]
fn guarded_executor_requires_explicit_scope_and_observed_shell_declarations() {
    for legacy in [false, true] {
        let f = Fixture::new("positive");
        let body = fs::read_to_string(f.root.join(SPEC)).unwrap();
        let (yaml, rest) = body
            .strip_prefix("---\n")
            .unwrap()
            .split_once("---\n")
            .unwrap();
        let mut fm: Value = serde_norway::from_str(yaml).unwrap();
        if legacy {
            fm["verify"] = json!([{"cmd":"./.mastermind/check.sh"}]);
            fm.as_object_mut().unwrap().remove("acceptance");
        } else {
            fm.as_object_mut().unwrap().remove("touches");
        }
        f.write(
            SPEC,
            &format!("---\n{}---\n{rest}", serde_norway::to_string(&fm).unwrap()),
        );
        assert!(!f.execute().status.success());
        assert!(!f.harness.join("stdin").exists());
        assert_eq!(
            fs::read_to_string(f.root.join("service.py")).unwrap(),
            "def keep():\n    return 1\n"
        );
    }
}
