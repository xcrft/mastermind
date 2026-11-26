//! Read-only lint for client allow rules that pre-approve arbitrary code.
//!
//! A rule is broad when its open arguments can name the code that runs: an
//! interpreter or shell without a fixed script, a launcher whose wrapped
//! command is open, a package manager or task runner without a fixed
//! subcommand (or with one that runs a named command), a network fetcher
//! without a fixed address, or a fetch piped into a shell. Exact commands and
//! rules with a fixed subcommand or script are not reported.
//!
//! Sources, checked against the client documentation:
//!
//! | Client | Files | Rule syntax |
//! |---|---|---|
//! | Claude Code | `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR/settings.json`), `.claude/settings.json`, `.claude/settings.local.json`: <https://code.claude.com/docs/en/settings> | `permissions.allow` entries `Bash(...)` and `PowerShell(...)`; `*` matches any text and a trailing `:*` equals ` *`: <https://code.claude.com/docs/en/permissions> |
//! | Codex | `$CODEX_HOME/rules/*.rules` (default `~/.codex/rules`) and `.codex/rules/*.rules`: <https://developers.openai.com/codex/rules> | `prefix_rule(pattern = [...], decision = "allow")`; a pattern matches a command prefix, so later arguments are always open |
//! | Codex | `$CODEX_HOME/config.toml` and `.codex/config.toml`: <https://developers.openai.com/codex/config-reference> | `approval_policy = "never"` with `sandbox_mode = "danger-full-access"` runs every command unsandboxed without asking |
//!
//! The lint never writes these files and never prints rule text: a finding
//! names the file label, the rule's position and a program name from the
//! lists below, so a token or URL inside a rule cannot reach the output.

use std::path::{Path, PathBuf};

/// Why a rule is broad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    AnyCommand,
    Interpreter,
    Shell,
    Launcher,
    PackageRunner,
    Fetcher,
    FetchIntoShell,
    UnsandboxedNoApproval,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::AnyCommand => "any command",
            Kind::Interpreter => "interpreter",
            Kind::Shell => "shell",
            Kind::Launcher => "runs another command",
            Kind::PackageRunner => "package manager or task runner",
            Kind::Fetcher => "network fetch",
            Kind::FetchIntoShell => "fetch piped into a shell",
            Kind::UnsandboxedNoApproval => "never asks and has no sandbox",
        }
    }
}

/// One broad rule. `program` always comes from this module's lists, never
/// from the settings file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub program: &'static str,
    pub kind: Kind,
}

const INTERPRETERS: &[&str] = &[
    "awk",
    "bun",
    "deno",
    "gawk",
    "groovy",
    "irb",
    "jshell",
    "julia",
    "lua",
    "luajit",
    "node",
    "osascript",
    "perl",
    "php",
    "powershell",
    "pwsh",
    "py",
    "python",
    "r",
    "rscript",
    "ruby",
    "tclsh",
    "ts-node",
    "tsx",
];

const SHELLS: &[&str] = &[
    "ash", "bash", "cmd", "csh", "dash", "fish", "ksh", "sh", "tcsh", "zsh",
];

/// Programs whose arguments name another command to run.
const LAUNCHERS: &[&str] = &[
    "builtin",
    "caffeinate",
    "chroot",
    "command",
    "doas",
    "env",
    "eval",
    "exec",
    "fd",
    "find",
    "flock",
    "iex",
    "invoke-expression",
    "ionice",
    "nice",
    "nohup",
    "nsenter",
    "parallel",
    "script",
    "setsid",
    "ssh",
    "start-process",
    "stdbuf",
    "su",
    "sudo",
    "systemd-run",
    "time",
    "timeout",
    "unbuffer",
    "unshare",
    "watch",
    "xargs",
];

/// Claude Code strips these wrappers before matching, or refuses to approve
/// them from a prefix rule (permissions page, "Wrappers" and "Exec wrappers and
/// `find` actions"), so an open rule for them does not widen what runs.
const CLAUDE_HANDLED_LAUNCHERS: &[&str] = &[
    "builtin", "command", "find", "flock", "ionice", "nice", "noglob", "nohup", "setsid", "stdbuf",
    "time", "timeout", "watch",
];

/// Package managers and task runners, with the subcommands that run a command
/// named in their arguments.
const RUNNERS: &[(&str, &[&str])] = &[
    ("bundle", &["exec"]),
    ("bunx", &[]),
    ("cargo", &[]),
    ("composer", &["exec"]),
    ("conda", &["run"]),
    ("devbox", &["run"]),
    ("direnv", &["exec"]),
    ("docker", &["exec", "run"]),
    ("gem", &[]),
    ("go", &["run"]),
    ("gradle", &[]),
    ("gradlew", &[]),
    ("invoke", &[]),
    ("just", &[]),
    ("kubectl", &["exec", "run"]),
    ("make", &[]),
    ("mise", &["exec", "x"]),
    ("mvn", &[]),
    ("mvnw", &[]),
    ("nox", &[]),
    ("npm", &["exec", "x"]),
    ("npx", &[]),
    ("nx", &[]),
    ("pip", &[]),
    ("pipenv", &["run"]),
    ("pipx", &["run"]),
    ("pnpm", &["dlx", "exec"]),
    ("pnpx", &[]),
    ("podman", &["exec", "run"]),
    ("poetry", &["run"]),
    ("rake", &[]),
    ("task", &[]),
    ("tox", &[]),
    ("turbo", &[]),
    ("uv", &["run", "tool"]),
    ("uvx", &[]),
    ("yarn", &["dlx", "exec"]),
];

const FETCHERS: &[&str] = &[
    "aria2c",
    "curl",
    "http",
    "https",
    "invoke-restmethod",
    "invoke-webrequest",
    "irm",
    "iwr",
    "wget",
    "xh",
];

/// Options that take program text as their value.
const CODE_OPTIONS: &[&str] = &[
    "-c",
    "-e",
    "--eval",
    "-p",
    "--print",
    "-r",
    "--command",
    "-command",
    "-encodedcommand",
    "/c",
    "/k",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Client {
    Claude,
    Codex,
}

/// A command pattern: literal words in order, an optional literal start of a
/// word that continues with a wildcard, and whether more text may follow.
#[derive(Debug, Default)]
struct Pattern<'a> {
    words: Vec<&'a str>,
    open: bool,
}

fn normalise(word: &str) -> String {
    let base = word
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(word)
        .to_ascii_lowercase();
    let base = base.strip_suffix(".exe").unwrap_or(&base).to_string();
    let trimmed = base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-');
    if trimmed.is_empty() {
        base
    } else {
        trimmed.to_string()
    }
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

fn is_option(word: &str) -> bool {
    word.starts_with('-')
}

fn lookup(list: &'static [&'static str], name: &str) -> Option<&'static str> {
    list.iter().copied().find(|entry| *entry == name)
}

fn runner(name: &str) -> Option<(&'static str, &'static [&'static str])> {
    RUNNERS.iter().copied().find(|(entry, _)| *entry == name)
}

/// The program a glob such as `py*` can reach, if any listed program starts
/// with its literal part.
fn glob_program(prefix: &str) -> Option<Finding> {
    let prefix = prefix.to_ascii_lowercase();
    let lists: [(&'static [&'static str], Kind); 4] = [
        (SHELLS, Kind::Shell),
        (INTERPRETERS, Kind::Interpreter),
        (LAUNCHERS, Kind::Launcher),
        (FETCHERS, Kind::Fetcher),
    ];
    for (list, kind) in lists {
        if let Some(program) = list.iter().copied().find(|name| name.starts_with(&prefix)) {
            return Some(Finding { program, kind });
        }
    }
    RUNNERS
        .iter()
        .find(|(name, _)| name.starts_with(&prefix))
        .map(|(program, _)| Finding {
            program,
            kind: Kind::PackageRunner,
        })
}

fn classify(words: &[&str], open: bool, client: Client) -> Option<Finding> {
    let start = words.iter().position(|word| !is_assignment(word));
    let Some(start) = start else {
        return open.then_some(Finding {
            program: "*",
            kind: Kind::AnyCommand,
        });
    };
    let name = normalise(words[start]);
    let rest = &words[start + 1..];
    let no_fixed_argument = rest.iter().all(|word| is_option(word));
    let ends_in_code_option = rest
        .last()
        .is_some_and(|word| CODE_OPTIONS.contains(&word.to_ascii_lowercase().as_str()));

    if let Some(program) = lookup(SHELLS, &name) {
        return (open && (no_fixed_argument || ends_in_code_option)).then_some(Finding {
            program,
            kind: Kind::Shell,
        });
    }
    if let Some(program) = lookup(INTERPRETERS, &name) {
        return (open && (no_fixed_argument || ends_in_code_option)).then_some(Finding {
            program,
            kind: Kind::Interpreter,
        });
    }
    if let Some(program) = lookup(LAUNCHERS, &name) {
        if client == Client::Claude && CLAUDE_HANDLED_LAUNCHERS.contains(&program) {
            return None;
        }
        if !open {
            return None;
        }
        if matches!(program, "find" | "fd") {
            return Some(Finding {
                program,
                kind: Kind::Launcher,
            });
        }
        let inner: Vec<&str> = rest
            .iter()
            .copied()
            .skip_while(|word| {
                is_option(word)
                    || is_assignment(word)
                    || word
                        .trim_end_matches(['s', 'm', 'h', 'd'])
                        .parse::<f64>()
                        .is_ok()
            })
            .collect();
        if inner.is_empty() {
            return Some(Finding {
                program,
                kind: Kind::Launcher,
            });
        }
        return classify(&inner, open, client);
    }
    if let Some((program, code_subcommands)) = runner(&name) {
        if !open {
            return None;
        }
        let mut positional = rest.iter().filter(|word| !is_option(word));
        let broad = match positional.next() {
            None => true,
            Some(sub) => {
                code_subcommands.contains(&sub.to_ascii_lowercase().as_str())
                    && positional.next().is_none()
            }
        };
        return broad.then_some(Finding {
            program,
            kind: Kind::PackageRunner,
        });
    }
    if let Some(program) = lookup(FETCHERS, &name) {
        return (open && no_fixed_argument).then_some(Finding {
            program,
            kind: Kind::Fetcher,
        });
    }
    None
}

/// Split a Claude Code command specifier into literal words. Words after the
/// first `*` are not literal; the literal start of the word holding it counts
/// as a fixed word, so `Bash(curl https://example.com/*)` keeps its address.
fn claude_pattern(spec: &str) -> Pattern<'_> {
    let mut pattern = Pattern::default();
    for word in spec.split_whitespace() {
        match word.find('*') {
            Some(index) => {
                pattern.open = true;
                if index > 0 {
                    pattern.words.push(&word[..index]);
                }
                break;
            }
            None => pattern.words.push(word),
        }
    }
    pattern
}

/// Classify one Claude Code `permissions.allow` entry. `Err` means the entry
/// is not a string rule Claude Code could apply.
fn classify_claude_rule(rule: &str) -> Result<Option<Finding>, ()> {
    let rule = rule.trim();
    let (tool, spec) = match rule.split_once('(') {
        Some((tool, rest)) => (tool, rest.strip_suffix(')').ok_or(())?),
        None => (rule, "*"),
    };
    if !matches!(tool, "Bash" | "PowerShell") {
        return Ok(None);
    }
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(());
    }
    let spec = match spec.strip_suffix(":*") {
        Some(head) => format!("{head} *"),
        None => spec.to_string(),
    };
    let segments: Vec<&str> = spec.split('|').collect();
    if segments.len() > 1 {
        let fetches = claude_pattern(segments[0])
            .words
            .first()
            .is_some_and(|word| lookup(FETCHERS, &normalise(word)).is_some());
        let into_shell = segments[1..].iter().any(|segment| {
            claude_pattern(segment).words.first().is_some_and(|word| {
                let name = normalise(word);
                lookup(SHELLS, &name).is_some() || lookup(INTERPRETERS, &name).is_some()
            })
        });
        if fetches && into_shell {
            return Ok(Some(Finding {
                program: "|",
                kind: Kind::FetchIntoShell,
            }));
        }
    }
    let first_word = spec.split_whitespace().next().unwrap_or("");
    if let Some(index) = first_word.find('*') {
        if index == 0 {
            return Ok(Some(Finding {
                program: "*",
                kind: Kind::AnyCommand,
            }));
        }
        return Ok(glob_program(&first_word[..index]));
    }
    let pattern = claude_pattern(&spec);
    Ok(classify(&pattern.words, pattern.open, Client::Claude))
}

/// Result of reading one settings file.
#[derive(Debug, PartialEq, Eq)]
pub enum FileLint {
    Absent,
    /// Rules read, rules that could not be applied, broad rules by position.
    Read {
        rules: usize,
        invalid: usize,
        findings: Vec<(usize, Finding)>,
    },
    Unreadable(&'static str),
}

/// Lint the JSON text of a Claude Code settings file.
pub fn lint_claude_settings(bytes: &[u8]) -> FileLint {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return FileLint::Unreadable("invalid_json");
    };
    let Some(object) = value.as_object() else {
        return FileLint::Unreadable("invalid_shape");
    };
    let allow = match object.get("permissions") {
        None => None,
        Some(serde_json::Value::Object(permissions)) => permissions.get("allow"),
        Some(_) => return FileLint::Unreadable("invalid_shape"),
    };
    let entries = match allow {
        None => &[][..],
        Some(serde_json::Value::Array(entries)) => entries.as_slice(),
        Some(_) => return FileLint::Unreadable("invalid_shape"),
    };
    let mut invalid = 0;
    let mut findings = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        match entry.as_str().map(classify_claude_rule) {
            Some(Ok(Some(finding))) => findings.push((index + 1, finding)),
            Some(Ok(None)) => {}
            Some(Err(())) | None => invalid += 1,
        }
    }
    FileLint::Read {
        rules: entries.len(),
        invalid,
        findings,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Punct(char),
    Other,
}

fn starlark_tokens(text: &str) -> Result<Vec<Token>, ()> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '#' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '"' | '\'' => {
                if chars.peek() == Some(&c) {
                    chars.next();
                    if chars.peek() == Some(&c) {
                        // Triple-quoted strings are outside the subset read here.
                        return Err(());
                    }
                    tokens.push(Token::Str(String::new()));
                    continue;
                }
                let mut value = String::new();
                loop {
                    match chars.next() {
                        None | Some('\n') => return Err(()),
                        Some('\\') => match chars.next() {
                            Some('n') => value.push('\n'),
                            Some('t') => value.push('\t'),
                            Some(escaped) => value.push(escaped),
                            None => return Err(()),
                        },
                        Some(end) if end == c => break,
                        Some(other) => value.push(other),
                    }
                }
                tokens.push(Token::Str(value));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut ident = c.to_string();
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_alphanumeric() || next == '_' {
                        ident.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Ident(ident));
            }
            '(' | ')' | '[' | ']' | ',' | '=' => tokens.push(Token::Punct(c)),
            _ => tokens.push(Token::Other),
        }
    }
    Ok(tokens)
}

#[derive(Debug)]
enum Value {
    Str(String),
    List(Vec<Value>),
    Other,
}

fn parse_value(tokens: &[Token], at: &mut usize) -> Result<Value, ()> {
    match tokens.get(*at) {
        Some(Token::Str(value)) => {
            *at += 1;
            Ok(Value::Str(value.clone()))
        }
        Some(Token::Punct('[')) => {
            *at += 1;
            let mut items = Vec::new();
            loop {
                match tokens.get(*at) {
                    Some(Token::Punct(']')) => {
                        *at += 1;
                        return Ok(Value::List(items));
                    }
                    Some(Token::Punct(',')) => *at += 1,
                    Some(_) => items.push(parse_value(tokens, at)?),
                    None => return Err(()),
                }
            }
        }
        Some(Token::Punct(_)) | None => Err(()),
        Some(_) => {
            *at += 1;
            Ok(Value::Other)
        }
    }
}

/// One `prefix_rule` call: alternatives per position, and its decision.
#[derive(Debug, PartialEq, Eq)]
struct PrefixRule {
    pattern: Vec<Vec<String>>,
    decision: String,
}

fn parse_codex_rules(text: &str) -> Result<Vec<PrefixRule>, ()> {
    let tokens = starlark_tokens(text)?;
    let mut rules = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        let is_rule = tokens[at] == Token::Ident("prefix_rule".into())
            && tokens.get(at + 1) == Some(&Token::Punct('('));
        if !is_rule {
            at += 1;
            continue;
        }
        at += 2;
        let mut pattern = None;
        let mut decision = "allow".to_string();
        loop {
            match tokens.get(at) {
                Some(Token::Punct(')')) => {
                    at += 1;
                    break;
                }
                Some(Token::Punct(',')) => at += 1,
                Some(Token::Ident(key)) if tokens.get(at + 1) == Some(&Token::Punct('=')) => {
                    let key = key.clone();
                    at += 2;
                    let value = parse_value(&tokens, &mut at)?;
                    match (key.as_str(), value) {
                        ("pattern", Value::List(items)) => {
                            let mut positions = Vec::new();
                            for item in items {
                                positions.push(match item {
                                    Value::Str(word) => vec![word],
                                    Value::List(options) => options
                                        .into_iter()
                                        .map(|option| match option {
                                            Value::Str(word) => Ok(word),
                                            _ => Err(()),
                                        })
                                        .collect::<Result<_, _>>()?,
                                    Value::Other => return Err(()),
                                });
                            }
                            if positions.is_empty() || positions.iter().any(Vec::is_empty) {
                                return Err(());
                            }
                            pattern = Some(positions);
                        }
                        ("pattern", _) => return Err(()),
                        ("decision", Value::Str(value)) => decision = value,
                        ("decision", _) => return Err(()),
                        _ => {}
                    }
                }
                _ => return Err(()),
            }
        }
        rules.push(PrefixRule {
            pattern: pattern.ok_or(())?,
            decision,
        });
    }
    Ok(rules)
}

/// Lint the text of a Codex `.rules` file. Only `allow` rules are read; the
/// first two positions are expanded because they hold the program and its
/// subcommand.
pub fn lint_codex_rules(bytes: &[u8]) -> FileLint {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return FileLint::Unreadable("invalid_encoding");
    };
    let Ok(rules) = parse_codex_rules(text) else {
        return FileLint::Unreadable("unsupported_syntax");
    };
    let mut findings = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        if rule.decision != "allow" {
            continue;
        }
        let programs = &rule.pattern[0];
        let subcommands: Vec<Option<&String>> = match rule.pattern.get(1) {
            Some(options) => options.iter().map(Some).collect(),
            None => vec![None],
        };
        let finding = programs.iter().find_map(|program| {
            subcommands.iter().find_map(|sub| {
                let mut words: Vec<&str> = vec![program.as_str()];
                words.extend(sub.map(String::as_str));
                words.extend(
                    rule.pattern
                        .iter()
                        .skip(2)
                        .map(|options| options[0].as_str()),
                );
                classify(&words, true, Client::Codex)
            })
        });
        if let Some(finding) = finding {
            findings.push((index + 1, finding));
        }
    }
    FileLint::Read {
        rules: rules.len(),
        invalid: 0,
        findings,
    }
}

/// Lint a Codex `config.toml` for the setting that approves every command.
pub fn lint_codex_config(bytes: &[u8]) -> FileLint {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return FileLint::Unreadable("invalid_encoding");
    };
    let Ok(value) = toml::from_str::<toml::Table>(text) else {
        return FileLint::Unreadable("invalid_toml");
    };
    let never = value.get("approval_policy").and_then(toml::Value::as_str) == Some("never");
    let full =
        value.get("sandbox_mode").and_then(toml::Value::as_str) == Some("danger-full-access");
    FileLint::Read {
        rules: usize::from(never) + usize::from(full),
        invalid: 0,
        findings: if never && full {
            vec![(
                1,
                Finding {
                    program: "approval_policy",
                    kind: Kind::UnsandboxedNoApproval,
                },
            )]
        } else {
            Vec::new()
        },
    }
}

#[derive(Debug, Clone, Copy)]
enum Format {
    ClaudeSettings,
    CodexRules,
    CodexConfig,
}

/// The files read for one run, with the label printed for each.
pub struct Locations {
    pub root: PathBuf,
    pub claude_config_dir: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
}

impl Locations {
    pub fn from_environment(root: &Path) -> Self {
        let home = std::env::home_dir();
        let from_env = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            root: root.to_path_buf(),
            claude_config_dir: from_env("CLAUDE_CONFIG_DIR")
                .or_else(|| home.as_ref().map(|home| home.join(".claude"))),
            codex_home: from_env("CODEX_HOME").or_else(|| home.map(|home| home.join(".codex"))),
        }
    }

    fn files(&self) -> Vec<(String, PathBuf, Format)> {
        let mut files = Vec::new();
        if let Some(dir) = &self.claude_config_dir {
            files.push((
                "claude-user".into(),
                dir.join("settings.json"),
                Format::ClaudeSettings,
            ));
        }
        let project = self.root.join(".claude");
        files.push((
            "claude-project".into(),
            project.join("settings.json"),
            Format::ClaudeSettings,
        ));
        files.push((
            "claude-local".into(),
            project.join("settings.local.json"),
            Format::ClaudeSettings,
        ));
        let codex_layers = [
            ("codex-user", self.codex_home.clone()),
            ("codex-project", Some(self.root.join(".codex"))),
        ];
        for (label, dir) in codex_layers {
            let Some(dir) = dir else { continue };
            files.push((
                label.to_string(),
                dir.join("config.toml"),
                Format::CodexConfig,
            ));
            for path in rules_files(&dir.join("rules")) {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| {
                        name.chars()
                            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
                    })
                    .unwrap_or("?.rules")
                    .to_string();
                files.push((format!("{label}:{name}"), path, Format::CodexRules));
            }
        }
        files
    }
}

const RULES_FILE_LIMIT: usize = 64;

fn rules_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "rules")
        })
        .collect();
    files.sort();
    files.truncate(RULES_FILE_LIMIT);
    files
}

const SHOWN_FINDINGS: usize = 8;

/// Read every location and build the doctor check. Missing files are normal;
/// unreadable files and broad rules warn.
pub fn check(locations: &Locations) -> crate::doctor::Check {
    let mut statuses = Vec::new();
    let mut shown = Vec::new();
    let mut total = 0usize;
    let mut unreadable = false;
    for (label, path, format) in locations.files() {
        let lint = match crate::setup::read_config_capped(&path) {
            Ok(None) => FileLint::Absent,
            Ok(Some(bytes)) => match format {
                Format::ClaudeSettings => lint_claude_settings(&bytes),
                Format::CodexRules => lint_codex_rules(&bytes),
                Format::CodexConfig => lint_codex_config(&bytes),
            },
            Err(class) if class == "config_too_large" => FileLint::Unreadable("too_large"),
            Err(_) => FileLint::Unreadable("unsafe_path"),
        };
        match lint {
            FileLint::Absent => {}
            FileLint::Unreadable(class) => {
                unreadable = true;
                statuses.push(format!("{label}={class}"));
            }
            FileLint::Read {
                rules,
                invalid,
                findings,
            } => {
                let mut status = format!("{label}={rules} read");
                if invalid > 0 {
                    status.push_str(&format!(", {invalid} unparsed"));
                }
                if !findings.is_empty() {
                    status.push_str(&format!(", {} broad", findings.len()));
                }
                statuses.push(status);
                for (position, finding) in findings {
                    total += 1;
                    if shown.len() < SHOWN_FINDINGS {
                        shown.push(format!(
                            "{label} #{position} {} ({})",
                            finding.program,
                            finding.kind.label()
                        ));
                    }
                }
            }
        }
    }
    if statuses.is_empty() {
        statuses.push("no permission settings found".into());
    }
    let mut message = statuses.join("; ");
    if total > 0 {
        message.push_str(" | broad: ");
        message.push_str(&shown.join(", "));
        if total > shown.len() {
            message.push_str(&format!(", and {} more", total - shown.len()));
        }
    }
    let status = if total > 0 || unreadable {
        crate::doctor::Status::Warn
    } else {
        crate::doctor::Status::Ok
    };
    let hint = if total > 0 {
        Some(
            "these rules let an agent run code it chooses without asking; narrow each to a fixed \
             script or subcommand, such as `Bash(npm run test)`, and edit the file yourself"
                .into(),
        )
    } else if unreadable {
        Some("fix or remove the unreadable settings file; doctor only reads it".into())
    } else {
        None
    };
    crate::doctor::Check {
        name: "permission rules",
        status,
        message,
        hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude(rule: &str) -> Option<Kind> {
        classify_claude_rule(rule)
            .expect("valid rule")
            .map(|f| f.kind)
    }

    #[test]
    fn claude_rules_that_run_chosen_code_are_broad() {
        for (rule, kind) in [
            ("Bash", Kind::AnyCommand),
            ("Bash(*)", Kind::AnyCommand),
            ("Bash(* --version)", Kind::AnyCommand),
            ("Bash(python:*)", Kind::Interpreter),
            ("Bash(python3 *)", Kind::Interpreter),
            ("Bash(/usr/bin/python3.12 -c *)", Kind::Interpreter),
            ("Bash(node -e *)", Kind::Interpreter),
            ("Bash(py*)", Kind::Interpreter),
            ("Bash(bash -c *)", Kind::Shell),
            ("Bash(sh *)", Kind::Shell),
            ("Bash(env *)", Kind::Launcher),
            ("Bash(sudo *)", Kind::Launcher),
            ("Bash(xargs *)", Kind::Launcher),
            ("Bash(sudo python *)", Kind::Interpreter),
            ("Bash(npm *)", Kind::PackageRunner),
            ("Bash(npm exec *)", Kind::PackageRunner),
            ("Bash(npx:*)", Kind::PackageRunner),
            ("Bash(pnpm dlx *)", Kind::PackageRunner),
            ("Bash(cargo *)", Kind::PackageRunner),
            ("Bash(make *)", Kind::PackageRunner),
            ("Bash(just *)", Kind::PackageRunner),
            ("Bash(uv run *)", Kind::PackageRunner),
            ("Bash(docker run *)", Kind::PackageRunner),
            ("Bash(FOO=1 pip *)", Kind::PackageRunner),
            ("Bash(curl *)", Kind::Fetcher),
            (
                "Bash(curl -fsSL https://example.com/install.sh | sh)",
                Kind::FetchIntoShell,
            ),
            ("Bash(wget -qO- * | bash)", Kind::FetchIntoShell),
            ("PowerShell(Invoke-Expression *)", Kind::Launcher),
            ("PowerShell(pwsh -Command *)", Kind::Interpreter),
        ] {
            assert_eq!(claude(rule), Some(kind), "{rule}");
        }
    }

    #[test]
    fn narrow_or_unrelated_claude_rules_are_not_reported() {
        for rule in [
            "Bash(npm run test)",
            "Bash(npm run *)",
            "Bash(npm test *)",
            "Bash(cargo test *)",
            "Bash(python -m pytest *)",
            "Bash(python scripts/build.py *)",
            "Bash(node scripts/check.js)",
            "Bash(python)",
            "Bash(git log *)",
            "Bash(ls*)",
            "Bash(curl https://api.example.com/*)",
            "Bash(make test)",
            "Bash(timeout *)",
            "Bash(find *)",
            "Bash(watch *)",
            "Read(./src/**)",
            "WebFetch(domain:example.com)",
            "mcp__mmcg__mmcg_search",
        ] {
            assert_eq!(claude(rule), None, "{rule}");
        }
    }

    #[test]
    fn malformed_claude_entries_are_counted_not_classified() {
        assert!(classify_claude_rule("Bash(python *").is_err());
        assert!(classify_claude_rule("Bash()").is_err());
        let lint = lint_claude_settings(
            br#"{"permissions":{"allow":["Bash(python:*)", 7, "Bash(npm *", "Bash(git status)"]}}"#,
        );
        assert_eq!(
            lint,
            FileLint::Read {
                rules: 4,
                invalid: 2,
                findings: vec![(
                    1,
                    Finding {
                        program: "python",
                        kind: Kind::Interpreter
                    }
                )],
            }
        );
    }

    #[test]
    fn claude_settings_shapes_fail_soft() {
        assert_eq!(
            lint_claude_settings(b"{not json"),
            FileLint::Unreadable("invalid_json")
        );
        assert_eq!(
            lint_claude_settings(b"[]"),
            FileLint::Unreadable("invalid_shape")
        );
        assert_eq!(
            lint_claude_settings(br#"{"permissions":{"allow":"Bash"}}"#),
            FileLint::Unreadable("invalid_shape")
        );
        assert_eq!(
            lint_claude_settings(br#"{"model":"opus"}"#),
            FileLint::Read {
                rules: 0,
                invalid: 0,
                findings: vec![]
            }
        );
    }

    #[test]
    fn codex_allow_rules_are_classified_by_program_and_subcommand() {
        let text = r#"
# Allowed after review.
prefix_rule(pattern = ["python3"], decision = "allow")
prefix_rule(pattern = ["gh", "pr", ["view", "list"]])
prefix_rule(pattern = [["npm", "pnpm"], ["test", "exec"]], justification = "tests")
prefix_rule(pattern = ["bash", "-lc"], decision = "prompt")
prefix_rule(
    pattern = ["cargo", "test"],
    match = ["cargo test --workspace"],
    not_match = [["cargo", "run"]],
)
prefix_rule(pattern = ['find'], decision = 'allow')
"#;
        let FileLint::Read {
            rules, findings, ..
        } = lint_codex_rules(text.as_bytes())
        else {
            panic!("rules should parse");
        };
        assert_eq!(rules, 6);
        let found: Vec<(usize, &str, Kind)> = findings
            .iter()
            .map(|(index, finding)| (*index, finding.program, finding.kind))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, "python", Kind::Interpreter),
                (3, "npm", Kind::PackageRunner),
                (6, "find", Kind::Launcher),
            ]
        );
    }

    #[test]
    fn unsupported_codex_rules_fail_soft() {
        for text in [
            "prefix_rule(pattern = [\"python\"",
            "prefix_rule(pattern = \"python\")",
            "prefix_rule(pattern = [])",
            "prefix_rule(decision = \"allow\")",
            "prefix_rule(pattern = [\"\"\"x\"\"\"])",
            "prefix_rule(pattern = [\"unterminated])",
        ] {
            assert_eq!(
                lint_codex_rules(text.as_bytes()),
                FileLint::Unreadable("unsupported_syntax"),
                "{text}"
            );
        }
        assert_eq!(
            lint_codex_rules(&[0xff, 0xfe]),
            FileLint::Unreadable("invalid_encoding")
        );
    }

    #[test]
    fn codex_config_warns_only_for_no_approval_without_sandbox() {
        let broad = lint_codex_config(
            b"approval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n",
        );
        assert!(matches!(broad, FileLint::Read { ref findings, .. } if findings.len() == 1));
        for text in [
            "approval_policy = \"never\"\nsandbox_mode = \"workspace-write\"\n",
            "approval_policy = \"on-request\"\nsandbox_mode = \"danger-full-access\"\n",
            "[mcp_servers.mmcg]\ncommand = \"mastermind\"\n",
        ] {
            assert!(
                matches!(lint_codex_config(text.as_bytes()), FileLint::Read { ref findings, .. } if findings.is_empty()),
                "{text}"
            );
        }
        assert_eq!(
            lint_codex_config(b"approval_policy = "),
            FileLint::Unreadable("invalid_toml")
        );
    }

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mmcg-permission-lint-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn check_reads_every_location_and_never_prints_rule_text() {
        let root = temp_root();
        let claude_dir = root.join("claude-home");
        let codex_home = root.join("codex-home");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::create_dir_all(root.join("project/.claude")).unwrap();
        std::fs::create_dir_all(codex_home.join("rules")).unwrap();
        let secret = "sk-test-0123456789";
        std::fs::write(
            claude_dir.join("settings.json"),
            format!(r#"{{"permissions":{{"allow":["Bash(curl -H 'Authorization: {secret}' *)","Bash(python:*)"]}}}}"#),
        )
        .unwrap();
        std::fs::write(root.join("project/.claude/settings.local.json"), "{broken").unwrap();
        std::fs::write(
            codex_home.join("rules/default.rules"),
            "prefix_rule(pattern = [\"npx\"])\n",
        )
        .unwrap();
        let before = std::fs::read(claude_dir.join("settings.json")).unwrap();

        let check = check(&Locations {
            root: root.join("project"),
            claude_config_dir: Some(claude_dir.clone()),
            codex_home: Some(codex_home),
        });

        assert_eq!(check.status, crate::doctor::Status::Warn);
        assert!(
            check.message.contains("claude-user=2 read, 1 broad"),
            "{}",
            check.message
        );
        assert!(
            check
                .message
                .contains("claude-user #2 python (interpreter)"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("claude-local=invalid_json"),
            "{}",
            check.message
        );
        assert!(
            check.message.contains("codex-user:default.rules #1 npx"),
            "{}",
            check.message
        );
        assert!(!check.message.contains(secret));
        assert!(!check.message.contains("Authorization"));
        assert!(
            !check.message.contains("claude-project"),
            "absent files are not listed"
        );
        assert_eq!(
            std::fs::read(claude_dir.join("settings.json")).unwrap(),
            before
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn check_is_ok_without_settings_or_with_narrow_rules() {
        let root = temp_root();
        let empty = check(&Locations {
            root: root.clone(),
            claude_config_dir: None,
            codex_home: None,
        });
        assert_eq!(empty.status, crate::doctor::Status::Ok);
        assert_eq!(empty.message, "no permission settings found");

        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(
            root.join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Bash(npm run test)","Bash(git diff *)"]}}"#,
        )
        .unwrap();
        let narrow = check(&Locations {
            root: root.clone(),
            claude_config_dir: None,
            codex_home: None,
        });
        assert_eq!(narrow.status, crate::doctor::Status::Ok);
        assert_eq!(narrow.message, "claude-project=2 read");
        assert!(narrow.hint.is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
