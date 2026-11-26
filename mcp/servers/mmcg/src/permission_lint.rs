//! Read-only lint for client permission settings that let an agent run code it
//! chooses without asking.
//!
//! A rule is broad when its open part can choose what runs: any command, an
//! interpreter or shell with no fixed script or with a code option, a command
//! that runs another command unless that command is fixed and known, a package
//! manager or task runner with no fixed subcommand, with one that runs a named
//! command or with one that installs packages, `git` or `gh` with no fixed
//! subcommand, a network fetch with no fixed host, or a fetch piped into a
//! shell. Settings that skip every prompt are reported as well.
//!
//! Sources, checked against the client documentation:
//!
//! | Client | Files | What is read |
//! |---|---|---|
//! | Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` (default `~/.claude/settings.json`), `.claude/settings.json`, `.claude/settings.local.json`: <https://code.claude.com/docs/en/settings> | `permissions.allow`, `ask` and `deny` entries `Bash(...)` and `PowerShell(...)`, where `*` matches any text and a trailing `:*` equals ` *`; deny and ask rules from any file are evaluated before allow rules: <https://code.claude.com/docs/en/permissions>. `permissions.defaultMode` and `skipDangerousModePermissionPrompt`: <https://code.claude.com/docs/en/settings-reference> |
//! | Codex | `$CODEX_HOME/rules/*.rules` (default `~/.codex/rules`) and `.codex/rules/*.rules`: <https://developers.openai.com/codex/rules> | `prefix_rule(pattern = [...], decision = ...)`; a pattern matches a command prefix, so later arguments are always open, and the strictest matching decision wins |
//! | Codex | `$CODEX_HOME/config.toml`, `.codex/config.toml` and profile files `$CODEX_HOME/<name>.config.toml`: <https://developers.openai.com/codex/config-reference> | `approval_policy`, `sandbox_mode` and `default_permissions` |
//!
//! Claude Code strips wrappers such as `timeout` from a command before it
//! matches rules. The documentation does not say whether a rule that itself
//! starts with such a wrapper is stripped too, so the lint reads past the
//! wrapper and judges the wrapped command.
//!
//! The lint never writes these files and never prints rule text: a finding
//! names the file label, the rule's position and a program or key name from
//! the lists below, so a token or URL inside a rule cannot reach the output.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Why a rule or setting is broad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    AnyCommand,
    Interpreter,
    Shell,
    Launcher,
    PackageRunner,
    PackageInstall,
    GitTool,
    Fetcher,
    FetchIntoShell,
    SkipsPrompts,
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
            Kind::PackageInstall => "installs packages",
            Kind::GitTool => "git or gh without a fixed subcommand",
            Kind::Fetcher => "network fetch without a fixed host",
            Kind::FetchIntoShell => "fetch piped into a shell",
            Kind::SkipsPrompts => "skips permission prompts",
            Kind::UnsandboxedNoApproval => "never asks and has no sandbox",
        }
    }
}

/// One broad rule or setting. `program` always comes from this module's
/// lists, never from the settings file.
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

/// Programs whose arguments name another command to run. PowerShell names
/// are lower case because PowerShell matches them without case.
const LAUNCHERS: &[&str] = &[
    "&",
    ".",
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
    "icm",
    "iex",
    "ii",
    "invoke-command",
    "invoke-expression",
    "invoke-item",
    "ionice",
    "nice",
    "noglob",
    "nohup",
    "nsenter",
    "parallel",
    "sajb",
    "saps",
    "script",
    "setsid",
    "source",
    "ssh",
    "start",
    "start-job",
    "start-process",
    "start-threadjob",
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

/// Launchers whose open arguments are always run as shell or script text, so
/// a fixed first word after them does not make the rule narrow.
const TEXT_LAUNCHERS: &[&str] = &[
    "eval",
    "icm",
    "iex",
    "invoke-command",
    "invoke-expression",
    "parallel",
    "script",
    "ssh",
    "su",
];

/// Claude Code refuses to approve these from a prefix rule (permissions page,
/// "Exec wrappers and `find` actions"), so an open rule for them adds nothing.
const CLAUDE_REFUSED_LAUNCHERS: &[&str] = &["find", "flock", "ionice", "setsid", "watch"];

/// Read-only programs that make a launcher rule narrow when they are the
/// fixed wrapped command.
const KNOWN_NARROW: &[&str] = &[
    "cat", "date", "df", "diff", "du", "echo", "grep", "head", "jq", "ls", "pwd", "rg", "stat",
    "tail", "tree", "uniq", "wc", "which",
];

/// A package manager or task runner, the subcommands that run a command named
/// in their arguments, and the subcommands that install packages.
struct Runner {
    name: &'static str,
    runs: &'static [&'static str],
    installs: &'static [&'static str],
}

const fn runner(
    name: &'static str,
    runs: &'static [&'static str],
    installs: &'static [&'static str],
) -> Runner {
    Runner {
        name,
        runs,
        installs,
    }
}

const RUNNERS: &[Runner] = &[
    runner("bundle", &["exec"], &["add", "install"]),
    runner("bunx", &[], &[]),
    runner("cargo", &[], &["install"]),
    runner("composer", &["exec"], &["install", "require"]),
    runner("conda", &["run"], &["create", "install"]),
    runner("devbox", &["run"], &["add"]),
    runner("direnv", &["exec"], &[]),
    runner("docker", &["compose", "exec", "run"], &[]),
    runner("gem", &[], &["install"]),
    runner("go", &["run"], &["get", "install"]),
    runner("gradle", &[], &[]),
    runner("gradlew", &[], &[]),
    runner("invoke", &[], &[]),
    runner("just", &[], &[]),
    runner("kubectl", &["exec", "run"], &[]),
    runner("make", &[], &[]),
    runner("mise", &["exec", "x"], &["install", "use"]),
    runner("mvn", &[], &[]),
    runner("mvnw", &[], &[]),
    runner("nox", &[], &[]),
    runner("npm", &["exec", "x"], &["add", "ci", "i", "install"]),
    runner("npx", &[], &[]),
    runner("nx", &[], &[]),
    runner("pip", &[], &["download", "install"]),
    runner("pipenv", &["run"], &["install"]),
    runner("pipx", &["run"], &["install"]),
    runner("pnpm", &["dlx", "exec"], &["add", "i", "install"]),
    runner("pnpx", &[], &[]),
    runner("podman", &["compose", "exec", "run"], &[]),
    runner("poetry", &["run"], &["add", "install"]),
    runner("rake", &[], &[]),
    runner("task", &[], &[]),
    runner("tox", &[], &[]),
    runner("turbo", &[], &[]),
    runner("uv", &["run", "tool"], &["add", "pip", "sync"]),
    runner("uvx", &[], &[]),
    runner("yarn", &["dlx", "exec"], &["add", "install"]),
];

/// Version control tools: subcommands and options that run configured
/// commands, such as `git -c core.sshCommand=...` or a `!` alias.
const GIT_TOOLS: &[(&str, &[&str], &[&str])] = &[
    ("gh", &["alias", "ext", "extension"], &[]),
    ("git", &["config"], &["-c", "--config-env", "--exec-path"]),
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

/// A command pattern: literal words in order, whether more text may follow,
/// and whether the last word is only the literal start of a longer word.
#[derive(Debug, Default)]
struct Pattern {
    words: Vec<String>,
    open: bool,
    last_partial: bool,
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

fn is_code_option(word: &str) -> bool {
    CODE_OPTIONS.contains(&word.to_ascii_lowercase().as_str())
}

fn lookup(list: &'static [&'static str], name: &str) -> Option<&'static str> {
    list.iter().copied().find(|entry| *entry == name)
}

fn find_runner(name: &str) -> Option<&'static Runner> {
    RUNNERS.iter().find(|runner| runner.name == name)
}

fn find_git_tool(
    name: &str,
) -> Option<(
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
)> {
    GIT_TOOLS
        .iter()
        .copied()
        .find(|(entry, _, _)| *entry == name)
}

fn is_known_program(name: &str) -> bool {
    [SHELLS, INTERPRETERS, LAUNCHERS, FETCHERS, KNOWN_NARROW]
        .iter()
        .any(|list| list.contains(&name))
        || find_runner(name).is_some()
        || find_git_tool(name).is_some()
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
    if let Some((program, _, _)) = GIT_TOOLS
        .iter()
        .find(|(name, _, _)| name.starts_with(&prefix))
    {
        return Some(Finding {
            program,
            kind: Kind::GitTool,
        });
    }
    RUNNERS
        .iter()
        .find(|runner| runner.name.starts_with(&prefix))
        .map(|runner| Finding {
            program: runner.name,
            kind: Kind::PackageRunner,
        })
}

/// Whether a fixed argument is an address whose host cannot be widened by the
/// wildcard: `https://host/...`, or a whole word `https://host`.
fn has_fixed_host(rest: &[&str], last_partial: bool) -> bool {
    let last = rest.len().saturating_sub(1);
    rest.iter().enumerate().any(|(index, word)| {
        let Some((_, after)) = word.split_once("://") else {
            return false;
        };
        let partial = last_partial && index == last;
        let (host, path) = match after.split_once('/') {
            Some((host, _)) => (host, true),
            None => (after, false),
        };
        !host.is_empty() && (path || !partial)
    })
}

fn classify(words: &[&str], open: bool, last_partial: bool, client: Client) -> Option<Finding> {
    let Some(start) = words.iter().position(|word| !is_assignment(word)) else {
        return open.then_some(Finding {
            program: "*",
            kind: Kind::AnyCommand,
        });
    };
    let name = normalise(words[start]);
    let rest = &words[start + 1..];
    let no_fixed_argument = rest.iter().all(|word| is_option(word));
    let leading_code_option = rest
        .iter()
        .take_while(|word| is_option(word) || is_code_option(word))
        .any(|word| is_code_option(word));

    if let Some(program) = lookup(SHELLS, &name) {
        return (open && (no_fixed_argument || leading_code_option)).then_some(Finding {
            program,
            kind: Kind::Shell,
        });
    }
    if let Some(program) = lookup(INTERPRETERS, &name) {
        return (open && (no_fixed_argument || leading_code_option)).then_some(Finding {
            program,
            kind: Kind::Interpreter,
        });
    }
    if let Some(program) = lookup(LAUNCHERS, &name) {
        if !open || (client == Client::Claude && CLAUDE_REFUSED_LAUNCHERS.contains(&program)) {
            return None;
        }
        let broad = Some(Finding {
            program,
            kind: Kind::Launcher,
        });
        if matches!(program, "find" | "fd") || TEXT_LAUNCHERS.contains(&program) {
            return broad;
        }
        // Option values such as `sudo -u root` or `xargs -I {}` cannot be
        // told apart from the wrapped program, so the rule stays broad unless
        // a fixed word names a program this module knows.
        let Some(inner) = rest
            .iter()
            .position(|word| is_known_program(&normalise(word)))
        else {
            return broad;
        };
        if KNOWN_NARROW.contains(&normalise(rest[inner]).as_str()) {
            return None;
        }
        return classify(&rest[inner..], open, last_partial, client);
    }
    if let Some(runner) = find_runner(&name) {
        if !open {
            return None;
        }
        let mut positional = rest.iter().filter(|word| !is_option(word));
        let kind = match positional.next() {
            None => Some(Kind::PackageRunner),
            Some(sub) => {
                let sub = sub.to_ascii_lowercase();
                if runner.installs.contains(&sub.as_str()) {
                    Some(Kind::PackageInstall)
                } else if runner.runs.contains(&sub.as_str()) && positional.next().is_none() {
                    Some(Kind::PackageRunner)
                } else {
                    None
                }
            }
        };
        return kind.map(|kind| Finding {
            program: runner.name,
            kind,
        });
    }
    if let Some((program, subcommands, options)) = find_git_tool(&name) {
        if !open {
            return None;
        }
        let configures = rest.iter().any(|word| {
            let option = word.split_once('=').map_or(*word, |(head, _)| head);
            options.contains(&option)
        });
        let broad = configures
            || match rest.iter().find(|word| !is_option(word)) {
                None => true,
                Some(sub) => subcommands.contains(&sub.to_ascii_lowercase().as_str()),
            };
        return broad.then_some(Finding {
            program,
            kind: Kind::GitTool,
        });
    }
    if let Some(program) = lookup(FETCHERS, &name) {
        return (open && !has_fixed_host(rest, last_partial)).then_some(Finding {
            program,
            kind: Kind::Fetcher,
        });
    }
    None
}

const QUOTES: [char; 3] = ['"', '\'', '`'];

/// Split a Claude Code command specifier into literal words. Quotes are
/// removed, because `bash -c "*"` leaves the code as open as `bash -c *`. A
/// word that held only quotes, such as `""`, may expand to anything and counts
/// as open. Words after the first `*` are not literal; the literal start of
/// the word holding it is kept, so `curl https://example.com/*` keeps its host.
fn claude_pattern(spec: &str) -> Pattern {
    let mut pattern = Pattern::default();
    for raw in spec.split_whitespace() {
        let word: String = raw.chars().filter(|c| !QUOTES.contains(c)).collect();
        if word.is_empty() {
            pattern.open = true;
            break;
        }
        match word.find('*') {
            Some(index) => {
                pattern.open = true;
                if index > 0 {
                    pattern.words.push(word[..index].to_string());
                    pattern.last_partial = true;
                }
                break;
            }
            None => pattern.words.push(word),
        }
    }
    pattern
}

/// The tool and normalised specifier of a Claude Code shell rule. `Ok(None)`
/// is a rule for another tool; `Err` is an entry Claude Code could not apply.
fn parse_claude_rule(rule: &str) -> Result<Option<(String, String)>, ()> {
    let rule = rule.trim();
    let (tool, spec) = match rule.split_once('(') {
        Some((tool, rest)) => (tool, rest.strip_suffix(')').ok_or(())?),
        None => (rule, "*"),
    };
    if !matches!(tool, "Bash" | "PowerShell") {
        return Ok(None);
    }
    let spec = spec.split_whitespace().collect::<Vec<_>>().join(" ");
    if spec.is_empty() {
        return Err(());
    }
    let spec = match spec.strip_suffix(":*") {
        Some(head) => format!("{head} *"),
        None => spec,
    };
    Ok(Some((tool.to_string(), spec)))
}

fn classify_claude_spec(spec: &str) -> Option<Finding> {
    let segments: Vec<&str> = spec.split('|').collect();
    if segments.len() > 1 {
        let first_word = |segment: &str| {
            claude_pattern(segment)
                .words
                .first()
                .map(|word| normalise(word))
        };
        let fetches = first_word(segments[0]).is_some_and(|name| lookup(FETCHERS, &name).is_some());
        let into_shell = segments[1..].iter().any(|segment| {
            first_word(segment).is_some_and(|name| {
                lookup(SHELLS, &name).is_some() || lookup(INTERPRETERS, &name).is_some()
            })
        });
        if fetches && into_shell {
            return Some(Finding {
                program: "|",
                kind: Kind::FetchIntoShell,
            });
        }
    }
    let pattern = claude_pattern(spec);
    let first = spec.split_whitespace().next().unwrap_or("");
    let first: String = first.chars().filter(|c| !QUOTES.contains(c)).collect();
    if let Some(index) = first.find('*') {
        if index == 0 {
            return Some(Finding {
                program: "*",
                kind: Kind::AnyCommand,
            });
        }
        return glob_program(&first[..index]);
    }
    if pattern.words.is_empty() {
        return pattern.open.then_some(Finding {
            program: "*",
            kind: Kind::AnyCommand,
        });
    }
    let words: Vec<&str> = pattern.words.iter().map(String::as_str).collect();
    classify(&words, pattern.open, pattern.last_partial, Client::Claude)
}

/// Result of reading one settings file.
#[derive(Debug, PartialEq, Eq)]
pub enum FileLint {
    Absent,
    /// Rules read, rules that could not be applied, broad rules that a deny,
    /// ask or stricter rule already covers, and the broad rules or settings.
    Read {
        rules: usize,
        invalid: usize,
        covered: usize,
        findings: Vec<(Option<usize>, Finding)>,
    },
    Unreadable(&'static str),
}

/// The parts of a Claude Code settings file the lint reads.
#[derive(Debug, Default)]
struct ClaudeSettings {
    allow: Vec<Option<String>>,
    /// Normalised `(tool, specifier)` of every deny and ask shell rule.
    guards: Vec<(String, String)>,
    bypass_mode: bool,
    skip_prompt: bool,
}

fn parse_claude_settings(bytes: &[u8]) -> Result<ClaudeSettings, &'static str> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| "invalid_json")?;
    let object = value.as_object().ok_or("invalid_shape")?;
    let mut settings = ClaudeSettings {
        skip_prompt: object.get("skipDangerousModePermissionPrompt")
            == Some(&serde_json::Value::Bool(true)),
        ..ClaudeSettings::default()
    };
    let permissions = match object.get("permissions") {
        None => return Ok(settings),
        Some(serde_json::Value::Object(permissions)) => permissions,
        Some(_) => return Err("invalid_shape"),
    };
    settings.bypass_mode = permissions
        .get("defaultMode")
        .and_then(serde_json::Value::as_str)
        == Some("bypassPermissions");
    let list = |key: &str| match permissions.get(key) {
        None => Ok(&[][..]),
        Some(serde_json::Value::Array(entries)) => Ok(entries.as_slice()),
        Some(_) => Err("invalid_shape"),
    };
    settings.allow = list("allow")?
        .iter()
        .map(|entry| entry.as_str().map(str::to_string))
        .collect();
    for key in ["deny", "ask"] {
        for entry in list(key)? {
            if let Some(Ok(Some(guard))) = entry.as_str().map(parse_claude_rule) {
                settings.guards.push(guard);
            }
        }
    }
    Ok(settings)
}

fn lint_claude(settings: &ClaudeSettings, guards: &BTreeSet<(String, String)>) -> FileLint {
    let mut invalid = 0;
    let mut covered = 0;
    let mut findings = Vec::new();
    if settings.bypass_mode {
        findings.push((
            None,
            Finding {
                program: "defaultMode",
                kind: Kind::SkipsPrompts,
            },
        ));
    }
    if settings.skip_prompt {
        findings.push((
            None,
            Finding {
                program: "skipDangerousModePermissionPrompt",
                kind: Kind::SkipsPrompts,
            },
        ));
    }
    for (index, entry) in settings.allow.iter().enumerate() {
        let Some(Ok(parsed)) = entry.as_deref().map(parse_claude_rule) else {
            invalid += 1;
            continue;
        };
        let Some((tool, spec)) = parsed else { continue };
        let Some(finding) = classify_claude_spec(&spec) else {
            continue;
        };
        // Deny and ask rules from any file are evaluated before allow rules.
        if guards.contains(&(tool.clone(), spec)) || guards.contains(&(tool, "*".to_string())) {
            covered += 1;
            continue;
        }
        findings.push((Some(index + 1), finding));
    }
    FileLint::Read {
        rules: settings.allow.len(),
        invalid,
        covered,
        findings,
    }
}

/// Lint one Claude Code settings file on its own.
pub fn lint_claude_settings(bytes: &[u8]) -> FileLint {
    match parse_claude_settings(bytes) {
        Ok(settings) => {
            let guards = settings.guards.iter().cloned().collect();
            lint_claude(&settings, &guards)
        }
        Err(class) => FileLint::Unreadable(class),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Punct(char),
    Other,
}

fn starlark_tokens(text: &str) -> Result<Vec<Token>, &'static str> {
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
                        return Err("unsupported_syntax");
                    }
                    tokens.push(Token::Str(String::new()));
                    continue;
                }
                let mut value = String::new();
                loop {
                    match chars.next() {
                        None | Some('\n') => return Err("unsupported_syntax"),
                        Some('\\') => match chars.next() {
                            Some('n') => value.push('\n'),
                            Some('t') => value.push('\t'),
                            Some(escaped) => value.push(escaped),
                            None => return Err("unsupported_syntax"),
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

/// Deepest list nesting read in a `.rules` value. A pattern needs two levels.
const MAX_VALUE_DEPTH: usize = 8;

fn parse_value(tokens: &[Token], at: &mut usize, depth: usize) -> Result<Value, &'static str> {
    match tokens.get(*at) {
        Some(Token::Str(value)) => {
            *at += 1;
            Ok(Value::Str(value.clone()))
        }
        Some(Token::Punct('[')) => {
            if depth >= MAX_VALUE_DEPTH {
                return Err("too_deep");
            }
            *at += 1;
            let mut items = Vec::new();
            loop {
                match tokens.get(*at) {
                    Some(Token::Punct(']')) => {
                        *at += 1;
                        return Ok(Value::List(items));
                    }
                    Some(Token::Punct(',')) => *at += 1,
                    Some(_) => items.push(parse_value(tokens, at, depth + 1)?),
                    None => return Err("unsupported_syntax"),
                }
            }
        }
        Some(Token::Punct(_)) | None => Err("unsupported_syntax"),
        Some(_) => {
            *at += 1;
            Ok(Value::Other)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Allow,
    Prompt,
    Forbidden,
}

/// One `prefix_rule` call: alternatives per position, and its decision.
#[derive(Debug, PartialEq, Eq)]
struct PrefixRule {
    pattern: Vec<Vec<String>>,
    decision: Decision,
}

/// Read the `prefix_rule` calls in a `.rules` file. The documentation shows
/// only direct calls. Any other use of the `prefix_rule` name, such as an
/// alias assignment, could hide rules, so the file is reported as unreadable.
fn parse_codex_rules(text: &str) -> Result<Vec<PrefixRule>, &'static str> {
    let tokens = starlark_tokens(text)?;
    let mut rules = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        if tokens[at] != Token::Ident("prefix_rule".into()) {
            at += 1;
            continue;
        }
        if tokens.get(at + 1) != Some(&Token::Punct('(')) {
            return Err("unsupported_syntax");
        }
        at += 2;
        let mut pattern = None;
        let mut decision = Decision::Allow;
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
                    let value = parse_value(&tokens, &mut at, 0)?;
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
                                            _ => Err("unsupported_syntax"),
                                        })
                                        .collect::<Result<_, _>>()?,
                                    Value::Other => return Err("unsupported_syntax"),
                                });
                            }
                            if positions.is_empty() || positions.iter().any(Vec::is_empty) {
                                return Err("unsupported_syntax");
                            }
                            pattern = Some(positions);
                        }
                        ("decision", Value::Str(value)) => {
                            decision = match value.as_str() {
                                "allow" => Decision::Allow,
                                "prompt" => Decision::Prompt,
                                "forbidden" => Decision::Forbidden,
                                _ => return Err("unsupported_syntax"),
                            }
                        }
                        ("pattern" | "decision", _) => return Err("unsupported_syntax"),
                        _ => {}
                    }
                }
                _ => return Err("unsupported_syntax"),
            }
        }
        rules.push(PrefixRule {
            pattern: pattern.ok_or("unsupported_syntax")?,
            decision,
        });
    }
    Ok(rules)
}

/// Most commands one pattern is expanded into before the file is reported as
/// unreadable.
const MAX_EXPANSIONS: usize = 256;

fn expand(pattern: &[Vec<String>]) -> Option<Vec<Vec<&str>>> {
    let mut commands: Vec<Vec<&str>> = vec![Vec::new()];
    for options in pattern {
        if commands.len().saturating_mul(options.len()) > MAX_EXPANSIONS {
            return None;
        }
        commands = commands
            .iter()
            .flat_map(|prefix| {
                options.iter().map(move |option| {
                    let mut command = prefix.clone();
                    command.push(option.as_str());
                    command
                })
            })
            .collect();
    }
    Some(commands)
}

/// Whether every command `allow` matches also matches `stricter`, so the
/// strictest-decision rule applies.
fn covers(stricter: &PrefixRule, allow: &PrefixRule) -> bool {
    stricter.pattern.len() <= allow.pattern.len()
        && stricter
            .pattern
            .iter()
            .zip(&allow.pattern)
            .all(|(wide, narrow)| narrow.iter().all(|word| wide.contains(word)))
}

fn lint_codex_rule_set(
    rules: &[PrefixRule],
    stricter: &[&PrefixRule],
) -> Result<FileLint, &'static str> {
    let mut covered = 0;
    let mut findings = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        if rule.decision != Decision::Allow {
            continue;
        }
        let commands = expand(&rule.pattern).ok_or("too_many_alternatives")?;
        let Some(finding) = commands
            .iter()
            .find_map(|words| classify(words, true, false, Client::Codex))
        else {
            continue;
        };
        if stricter.iter().any(|guard| covers(guard, rule)) {
            covered += 1;
            continue;
        }
        findings.push((Some(index + 1), finding));
    }
    Ok(FileLint::Read {
        rules: rules.len(),
        invalid: 0,
        covered,
        findings,
    })
}

/// Lint one Codex `.rules` file on its own.
pub fn lint_codex_rules(bytes: &[u8]) -> FileLint {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return FileLint::Unreadable("invalid_encoding");
    };
    let rules = match parse_codex_rules(text) {
        Ok(rules) => rules,
        Err(class) => return FileLint::Unreadable(class),
    };
    let stricter: Vec<&PrefixRule> = rules
        .iter()
        .filter(|rule| rule.decision != Decision::Allow)
        .collect();
    lint_codex_rule_set(&rules, &stricter).unwrap_or_else(FileLint::Unreadable)
}

/// The approval and sandbox keys one Codex configuration layer sets.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct CodexLayer {
    never_asks: Option<bool>,
    full_access: Option<bool>,
}

impl CodexLayer {
    fn keys(self) -> usize {
        usize::from(self.never_asks.is_some()) + usize::from(self.full_access.is_some())
    }
}

fn parse_codex_config(bytes: &[u8]) -> Result<CodexLayer, &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid_encoding")?;
    let table: toml::Table = toml::from_str(text).map_err(|_| "invalid_toml")?;
    let never_asks = table
        .get("approval_policy")
        .map(|value| value.as_str() == Some("never"));
    let sandbox = table
        .get("sandbox_mode")
        .map(|value| value.as_str() == Some("danger-full-access"));
    let permissions = table
        .get("default_permissions")
        .map(|value| value.as_str() == Some(":danger-full-access"));
    let full_access = match (sandbox, permissions) {
        (None, None) => None,
        (sandbox, permissions) => Some(sandbox == Some(true) || permissions == Some(true)),
    };
    Ok(CodexLayer {
        never_asks,
        full_access,
    })
}

/// Combine layers, highest precedence first, and report whether the result
/// never asks and has no sandbox.
fn unsandboxed_without_approval(layers: &[CodexLayer]) -> bool {
    let never = layers.iter().find_map(|layer| layer.never_asks);
    let full = layers.iter().find_map(|layer| layer.full_access);
    never == Some(true) && full == Some(true)
}

/// Lint one Codex `config.toml` on its own.
pub fn lint_codex_config(bytes: &[u8]) -> FileLint {
    match parse_codex_config(bytes) {
        Ok(layer) => FileLint::Read {
            rules: layer.keys(),
            invalid: 0,
            covered: 0,
            findings: if unsandboxed_without_approval(&[layer]) {
                vec![(None, unsandboxed_finding())]
            } else {
                Vec::new()
            },
        },
        Err(class) => FileLint::Unreadable(class),
    }
}

fn unsandboxed_finding() -> Finding {
    Finding {
        program: "approval_policy",
        kind: Kind::UnsandboxedNoApproval,
    }
}

/// The files read for one run.
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

    fn claude_files(&self) -> Vec<(String, PathBuf)> {
        let mut files = Vec::new();
        if let Some(dir) = &self.claude_config_dir {
            files.push(("claude-user".into(), dir.join("settings.json")));
        }
        let project = self.root.join(".claude");
        files.push(("claude-project".into(), project.join("settings.json")));
        files.push(("claude-local".into(), project.join("settings.local.json")));
        files
    }

    fn codex_layers(&self) -> Vec<(&'static str, PathBuf)> {
        let mut layers = Vec::new();
        if let Some(home) = &self.codex_home {
            layers.push(("codex-user", home.clone()));
        }
        layers.push(("codex-project", self.root.join(".codex")));
        layers
    }
}

/// Most files read from one directory; the rest are counted and reported.
const DIRECTORY_FILE_LIMIT: usize = 64;

/// Files in `dir` whose names end with `suffix`, sorted, with the count left
/// out by the limit.
fn listed_files(dir: &Path, suffix: &str) -> (Vec<(String, PathBuf)>, usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (Vec::new(), 0);
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(suffix) && name.len() > suffix.len())
        })
        .collect();
    files.sort();
    let dropped = files.len().saturating_sub(DIRECTORY_FILE_LIMIT);
    files.truncate(DIRECTORY_FILE_LIMIT);
    let named = files
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| {
                    name.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
                })
                .unwrap_or("?")
                .to_string();
            (name, path)
        })
        .collect();
    (named, dropped)
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, &'static str> {
    match crate::setup::read_config_capped(path) {
        Ok(bytes) => Ok(bytes),
        Err(class) if class == "config_too_large" => Err("too_large"),
        Err(_) => Err("unsafe_path"),
    }
}

const SHOWN_FINDINGS: usize = 8;

#[derive(Default)]
struct Summary {
    statuses: Vec<String>,
    shown: Vec<String>,
    total: usize,
    unreadable: bool,
}

impl Summary {
    fn unreadable(&mut self, label: &str, class: &str) {
        self.unreadable = true;
        self.statuses.push(format!("{label}={class}"));
    }

    fn finding(&mut self, label: &str, position: Option<usize>, finding: &Finding) {
        self.total += 1;
        if self.shown.len() < SHOWN_FINDINGS {
            let position = position
                .map(|position| format!(" #{position}"))
                .unwrap_or_default();
            self.shown.push(format!(
                "{label}{position} {} ({})",
                finding.program,
                finding.kind.label()
            ));
        }
    }

    fn add(&mut self, label: &str, lint: FileLint) {
        match lint {
            FileLint::Absent => {}
            FileLint::Unreadable(class) => self.unreadable(label, class),
            FileLint::Read {
                rules,
                invalid,
                covered,
                findings,
            } => {
                let mut status = format!("{label}={rules} read");
                if invalid > 0 {
                    status.push_str(&format!(", {invalid} unparsed"));
                }
                if covered > 0 {
                    status.push_str(&format!(", {covered} broad but denied or asked"));
                }
                if !findings.is_empty() {
                    status.push_str(&format!(", {} broad", findings.len()));
                }
                self.statuses.push(status);
                for (position, finding) in &findings {
                    self.finding(label, *position, finding);
                }
            }
        }
    }
}

/// Read every location and build the doctor check. Missing files are normal;
/// unreadable or skipped files and broad rules warn.
pub fn check(locations: &Locations) -> crate::doctor::Check {
    let mut summary = Summary::default();

    let mut claude = Vec::new();
    for (label, path) in locations.claude_files() {
        match read(&path).map(|bytes| bytes.map(|bytes| parse_claude_settings(&bytes))) {
            Ok(None) => {}
            Ok(Some(Ok(settings))) => claude.push((label, settings)),
            Ok(Some(Err(class))) | Err(class) => summary.unreadable(&label, class),
        }
    }
    let guards: BTreeSet<(String, String)> = claude
        .iter()
        .flat_map(|(_, settings)| settings.guards.iter().cloned())
        .collect();
    for (label, settings) in &claude {
        summary.add(label, lint_claude(settings, &guards));
    }

    let mut rule_files = Vec::new();
    let mut user_config = CodexLayer::default();
    let mut project_config = CodexLayer::default();
    let mut profiles = Vec::new();
    for (layer, dir) in locations.codex_layers() {
        match read(&dir.join("config.toml"))
            .map(|bytes| bytes.map(|bytes| parse_codex_config(&bytes)))
        {
            Ok(None) => {}
            Ok(Some(Ok(config))) => {
                summary
                    .statuses
                    .push(format!("{layer}:config.toml={} read", config.keys()));
                if layer == "codex-user" {
                    user_config = config;
                } else {
                    project_config = config;
                }
            }
            Ok(Some(Err(class))) | Err(class) => {
                summary.unreadable(&format!("{layer}:config.toml"), class)
            }
        }
        if layer == "codex-user" {
            // Profile files live only beside the user configuration.
            let (files, dropped) = listed_files(&dir, ".config.toml");
            if dropped > 0 {
                summary.unreadable(&format!("{layer}:profiles"), "over_64_files_not_read");
            }
            for (name, path) in files {
                let label = format!("{layer}:{name}");
                match read(&path).map(|bytes| bytes.map(|bytes| parse_codex_config(&bytes))) {
                    Ok(None) => {}
                    Ok(Some(Ok(profile))) => {
                        summary
                            .statuses
                            .push(format!("{label}={} read", profile.keys()));
                        profiles.push((label, profile));
                    }
                    Ok(Some(Err(class))) | Err(class) => summary.unreadable(&label, class),
                }
            }
        }
        let (files, dropped) = listed_files(&dir.join("rules"), ".rules");
        if dropped > 0 {
            summary.unreadable(&format!("{layer}:rules"), "over_64_files_not_read");
        }
        for (name, path) in files {
            let label = format!("{layer}:{name}");
            let parsed = read(&path).and_then(|bytes| {
                bytes
                    .map(|bytes| {
                        std::str::from_utf8(&bytes)
                            .map_err(|_| "invalid_encoding")
                            .and_then(parse_codex_rules)
                    })
                    .transpose()
            });
            match parsed {
                Ok(None) => {}
                Ok(Some(rules)) => rule_files.push((label, rules)),
                Err(class) => summary.unreadable(&label, class),
            }
        }
    }
    // The strictest matching decision wins across every loaded file.
    let stricter: Vec<&PrefixRule> = rule_files
        .iter()
        .flat_map(|(_, rules)| rules.iter())
        .filter(|rule| rule.decision != Decision::Allow)
        .collect();
    for (label, rules) in &rule_files {
        match lint_codex_rule_set(rules, &stricter) {
            Ok(lint) => summary.add(label, lint),
            Err(class) => summary.unreadable(label, class),
        }
    }
    // The project layer overrides the user layer, and a selected profile
    // overrides both, so each combination is checked.
    let base = [project_config, user_config];
    if unsandboxed_without_approval(&base) {
        summary.finding("codex-config", None, &unsandboxed_finding());
    }
    for (label, profile) in &profiles {
        if unsandboxed_without_approval(&[*profile, base[0], base[1]])
            && !unsandboxed_without_approval(&base)
        {
            summary.finding(label, None, &unsandboxed_finding());
        }
    }

    if summary.statuses.is_empty() {
        summary.statuses.push("no permission settings found".into());
    }
    let mut message = summary.statuses.join("; ");
    if summary.total > 0 {
        message.push_str(" | broad: ");
        message.push_str(&summary.shown.join(", "));
        if summary.total > summary.shown.len() {
            message.push_str(&format!(
                ", and {} more",
                summary.total - summary.shown.len()
            ));
        }
    }
    let status = if summary.total > 0 || summary.unreadable {
        crate::doctor::Status::Warn
    } else {
        crate::doctor::Status::Ok
    };
    let hint = if summary.total > 0 {
        Some(
            "these settings let an agent run code it chooses without asking; narrow each rule to \
             a fixed script or subcommand, such as `Bash(npm run test)`, and edit the file yourself"
                .into(),
        )
    } else if summary.unreadable {
        Some("fix, move or remove the file doctor could not read; doctor only reads it".into())
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

    fn classify_claude_rule(rule: &str) -> Result<Option<Finding>, ()> {
        Ok(parse_claude_rule(rule)?.and_then(|(_, spec)| classify_claude_spec(&spec)))
    }

    fn claude(rule: &str) -> Option<Kind> {
        classify_claude_rule(rule)
            .expect("valid rule")
            .map(|f| f.kind)
    }

    fn codex(text: &str) -> Vec<(usize, &'static str, Kind)> {
        match lint_codex_rules(text.as_bytes()) {
            FileLint::Read { findings, .. } => findings
                .into_iter()
                .map(|(index, finding)| (index.unwrap(), finding.program, finding.kind))
                .collect(),
            other => panic!("rules should parse: {other:?}"),
        }
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
    fn git_and_gh_without_a_fixed_subcommand_are_broad() {
        for rule in [
            "Bash(git *)",
            "Bash(git:*)",
            "Bash(git -c core.sshCommand=x *)",
            "Bash(git -c alias.x=!sh *)",
            "Bash(git --exec-path=/tmp *)",
            "Bash(git config *)",
            "Bash(gh *)",
            "Bash(gh alias *)",
            "Bash(gh extension *)",
        ] {
            assert_eq!(claude(rule), Some(Kind::GitTool), "{rule}");
        }
        for rule in ["Bash(git log *)", "Bash(git diff *)", "Bash(gh pr view *)"] {
            assert_eq!(claude(rule), None, "{rule}");
        }
        assert_eq!(
            codex("prefix_rule(pattern = [\"git\"])\nprefix_rule(pattern = [\"git\", \"status\"])"),
            vec![(1, "git", Kind::GitTool)]
        );
    }

    #[test]
    fn quoted_wildcards_leave_the_code_open() {
        for (rule, kind) in [
            ("Bash(bash -c \"*\")", Kind::Shell),
            ("Bash(sh -c '*')", Kind::Shell),
            ("Bash(python -c '*')", Kind::Interpreter),
            ("Bash(bash -c 'echo *')", Kind::Shell),
            ("Bash(python -c 'import os; *')", Kind::Interpreter),
            ("Bash(bash -c \"\")", Kind::Shell),
        ] {
            assert_eq!(claude(rule), Some(kind), "{rule}");
        }
        assert_eq!(claude("Bash(python -m pytest -p no:cacheprovider *)"), None);
    }

    #[test]
    fn launcher_option_values_do_not_count_as_the_wrapped_program() {
        for rule in [
            "Bash(sudo -u root *)",
            "Bash(ssh host *)",
            "Bash(ssh host ls *)",
            "Bash(xargs -I {} *)",
            "Bash(env FOO=1 *)",
            "Bash(timeout 5 *)",
            "Bash(timeout *)",
            "Bash(nice -n 10 *)",
        ] {
            assert_eq!(claude(rule), Some(Kind::Launcher), "{rule}");
        }
        assert_eq!(claude("Bash(timeout 5 sh *)"), Some(Kind::Shell));
        assert_eq!(
            claude("Bash(sudo -u root python *)"),
            Some(Kind::Interpreter)
        );
        for rule in [
            "Bash(timeout 5 npm test *)",
            "Bash(env FOO=1 cargo test *)",
            "Bash(xargs grep *)",
        ] {
            assert_eq!(claude(rule), None, "{rule}");
        }
        assert_eq!(
            codex(
                "prefix_rule(pattern = [\"sudo\", \"-u\", \"root\"])\nprefix_rule(pattern = [\"ssh\", \"host\"])\nprefix_rule(pattern = [\"timeout\", \"5\", \"cargo\", \"test\"])"
            ),
            vec![(1, "sudo", Kind::Launcher), (2, "ssh", Kind::Launcher)]
        );
    }

    #[test]
    fn fetchers_need_a_fixed_host() {
        for rule in [
            "Bash(curl *)",
            "Bash(curl https://*)",
            "Bash(curl https://example.com*)",
            "Bash(curl -o /tmp/x *)",
            "Bash(wget -q *)",
        ] {
            assert_eq!(claude(rule), Some(Kind::Fetcher), "{rule}");
        }
        for rule in [
            "Bash(curl https://api.example.com/*)",
            "Bash(curl -s https://api.example.com/v1 *)",
            "Bash(curl https://api.example.com)",
        ] {
            assert_eq!(claude(rule), None, "{rule}");
        }
    }

    #[test]
    fn powershell_launchers_are_broad() {
        for rule in [
            "PowerShell(Invoke-Command *)",
            "PowerShell(icm *)",
            "PowerShell(saps *)",
            "PowerShell(start *)",
            "PowerShell(Start-Job *)",
            "PowerShell(Start-Process *)",
            "PowerShell(& *)",
            "PowerShell(. *)",
        ] {
            assert_eq!(claude(rule), Some(Kind::Launcher), "{rule}");
        }
        assert_eq!(claude("PowerShell(Get-ChildItem *)"), None);
    }

    #[test]
    fn package_installs_are_broad() {
        for rule in [
            "Bash(pip install *)",
            "Bash(npm install *)",
            "Bash(npm i *)",
            "Bash(cargo install *)",
            "Bash(uv pip install *)",
            "Bash(go install *)",
        ] {
            assert_eq!(claude(rule), Some(Kind::PackageInstall), "{rule}");
        }
        assert_eq!(claude("Bash(uv run --with *)"), Some(Kind::PackageRunner));
        assert_eq!(claude("Bash(docker compose *)"), Some(Kind::PackageRunner));
        assert_eq!(
            claude("Bash(npm install)"),
            None,
            "an exact command is not open"
        );
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
            "Bash(make test)",
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
                covered: 0,
                findings: vec![(
                    Some(1),
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
        for text in [
            r#"{"permissions":{"allow":"Bash"}}"#,
            r#"{"permissions":{"deny":"Bash"}}"#,
            r#"{"permissions":[]}"#,
        ] {
            assert_eq!(
                lint_claude_settings(text.as_bytes()),
                FileLint::Unreadable("invalid_shape"),
                "{text}"
            );
        }
        assert_eq!(
            lint_claude_settings(br#"{"model":"opus"}"#),
            FileLint::Read {
                rules: 0,
                invalid: 0,
                covered: 0,
                findings: vec![]
            }
        );
    }

    #[test]
    fn claude_settings_that_skip_prompts_are_reported() {
        let programs = |text: &str| match lint_claude_settings(text.as_bytes()) {
            FileLint::Read { findings, .. } => findings
                .into_iter()
                .map(|(position, finding)| (position, finding.program, finding.kind))
                .collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            programs(
                r#"{"permissions":{"defaultMode":"bypassPermissions"},"skipDangerousModePermissionPrompt":true}"#
            ),
            vec![
                (None, "defaultMode", Kind::SkipsPrompts),
                (
                    None,
                    "skipDangerousModePermissionPrompt",
                    Kind::SkipsPrompts
                ),
            ]
        );
        assert!(programs(
            r#"{"permissions":{"defaultMode":"plan"},"skipDangerousModePermissionPrompt":false}"#
        )
        .is_empty());
    }

    #[test]
    fn deny_and_ask_rules_cover_a_broad_allow_rule() {
        let lint = lint_claude_settings(
            br#"{"permissions":{"allow":["Bash(python:*)","Bash(npm *)","Bash(curl *)"],"deny":["Bash(python *)"],"ask":["Bash(npm:*)"]}}"#,
        );
        let FileLint::Read {
            covered, findings, ..
        } = lint
        else {
            panic!("settings should parse");
        };
        assert_eq!(covered, 2);
        assert_eq!(findings.len(), 1);
        let whole_tool =
            lint_claude_settings(br#"{"permissions":{"allow":["Bash(sh *)"],"deny":["Bash"]}}"#);
        assert!(
            matches!(whole_tool, FileLint::Read { covered: 1, ref findings, .. } if findings.is_empty())
        );
    }

    #[test]
    fn codex_allow_rules_are_classified_by_every_alternative() {
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
prefix_rule(pattern = ["timeout", "5", ["ls", "bash"]])
"#;
        assert_eq!(
            codex(text),
            vec![
                (1, "python", Kind::Interpreter),
                (3, "npm", Kind::PackageRunner),
                (6, "find", Kind::Launcher),
                (7, "bash", Kind::Shell),
            ]
        );
    }

    #[test]
    fn stricter_codex_rules_cover_a_broad_allow_rule() {
        let text = r#"
prefix_rule(pattern = ["python3", "-c"])
prefix_rule(pattern = ["python3"], decision = "forbidden")
prefix_rule(pattern = ["npx"])
"#;
        let FileLint::Read {
            covered, findings, ..
        } = lint_codex_rules(text.as_bytes())
        else {
            panic!("rules should parse");
        };
        assert_eq!(covered, 1);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].1.program, "npx");
    }

    #[test]
    fn unsupported_codex_rules_fail_soft() {
        for (text, class) in [
            ("prefix_rule(pattern = [\"python\"", "unsupported_syntax"),
            ("prefix_rule(pattern = \"python\")", "unsupported_syntax"),
            ("prefix_rule(pattern = [])", "unsupported_syntax"),
            ("prefix_rule(decision = \"allow\")", "unsupported_syntax"),
            (
                "prefix_rule(pattern = [\"x\"], decision = \"maybe\")",
                "unsupported_syntax",
            ),
            (
                "prefix_rule(pattern = [\"\"\"x\"\"\"])",
                "unsupported_syntax",
            ),
            (
                "prefix_rule(pattern = [\"unterminated])",
                "unsupported_syntax",
            ),
            (
                "r = prefix_rule\nr(pattern = [\"python\"])",
                "unsupported_syntax",
            ),
        ] {
            assert_eq!(
                lint_codex_rules(text.as_bytes()),
                FileLint::Unreadable(class),
                "{text}"
            );
        }
        assert_eq!(
            lint_codex_rules(&[0xff, 0xfe]),
            FileLint::Unreadable("invalid_encoding")
        );
        let wide = format!(
            "prefix_rule(pattern = [{}])",
            ["[\"a\", \"b\", \"c\"]"; 6].join(", ")
        );
        assert_eq!(
            lint_codex_rules(wide.as_bytes()),
            FileLint::Unreadable("too_many_alternatives")
        );
    }

    #[test]
    fn deep_codex_nesting_is_refused_without_recursing_further() {
        let deep = format!(
            "prefix_rule(pattern = {}\"x\"{})",
            "[".repeat(100_000),
            "]".repeat(100_000)
        );
        assert_eq!(
            lint_codex_rules(deep.as_bytes()),
            FileLint::Unreadable("too_deep")
        );
        let nested = format!(
            "prefix_rule(pattern = [\"x\"], match = {}\"x\"{})",
            "[".repeat(9),
            "]".repeat(9)
        );
        assert_eq!(
            lint_codex_rules(nested.as_bytes()),
            FileLint::Unreadable("too_deep")
        );
    }

    #[test]
    fn codex_config_warns_only_for_no_approval_without_sandbox() {
        for text in [
            "approval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n",
            "approval_policy = \"never\"\ndefault_permissions = \":danger-full-access\"\n",
        ] {
            assert!(
                matches!(lint_codex_config(text.as_bytes()), FileLint::Read { ref findings, .. } if findings.len() == 1),
                "{text}"
            );
        }
        for text in [
            "approval_policy = \"never\"\nsandbox_mode = \"workspace-write\"\n",
            "approval_policy = \"never\"\ndefault_permissions = \":workspace\"\n",
            "approval_policy = \"on-request\"\nsandbox_mode = \"danger-full-access\"\n",
            "approval_policy = { granular = { rules = true } }\nsandbox_mode = \"danger-full-access\"\n",
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

    #[test]
    fn codex_layers_combine_by_precedence() {
        let never = CodexLayer {
            never_asks: Some(true),
            full_access: None,
        };
        let full = CodexLayer {
            never_asks: None,
            full_access: Some(true),
        };
        let asks = CodexLayer {
            never_asks: Some(false),
            full_access: None,
        };
        assert!(unsandboxed_without_approval(&[full, never]));
        assert!(!unsandboxed_without_approval(&[asks, full, never]));
        assert!(!unsandboxed_without_approval(&[never]));
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
            format!(
                r#"{{"permissions":{{"allow":["Bash(curl -H 'Authorization: {secret}' *)","Bash(python:*)","Bash(node *)"]}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("project/.claude/settings.json"),
            r#"{"permissions":{"deny":["Bash(node *)"]}}"#,
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
        let message = &check.message;
        assert!(
            message.contains("claude-user=3 read, 1 broad but denied or asked, 2 broad"),
            "{message}"
        );
        assert!(
            message.contains("claude-user #2 python (interpreter)"),
            "{message}"
        );
        assert!(message.contains("claude-local=invalid_json"), "{message}");
        assert!(
            message.contains("codex-user:default.rules #1 npx"),
            "{message}"
        );
        assert!(!message.contains(secret));
        assert!(!message.contains("Authorization"));
        assert_eq!(
            std::fs::read(claude_dir.join("settings.json")).unwrap(),
            before
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn check_combines_codex_layers_and_profiles() {
        let root = temp_root();
        let codex_home = root.join("codex-home");
        std::fs::create_dir_all(&codex_home).unwrap();
        std::fs::create_dir_all(root.join("project/.codex")).unwrap();
        std::fs::write(
            codex_home.join("config.toml"),
            "approval_policy = \"never\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("project/.codex/config.toml"),
            "sandbox_mode = \"danger-full-access\"\n",
        )
        .unwrap();
        let check_with = |root: &Path| {
            check(&Locations {
                root: root.join("project"),
                claude_config_dir: None,
                codex_home: Some(root.join("codex-home")),
            })
        };
        let combined = check_with(&root);
        assert!(
            combined.message.contains("codex-config approval_policy"),
            "{}",
            combined.message
        );

        std::fs::write(
            root.join("project/.codex/config.toml"),
            "sandbox_mode = \"workspace-write\"\n",
        )
        .unwrap();
        std::fs::write(
            codex_home.join("yolo.config.toml"),
            "default_permissions = \":danger-full-access\"\n",
        )
        .unwrap();
        let profile = check_with(&root);
        assert!(
            profile
                .message
                .contains("codex-user:yolo.config.toml approval_policy"),
            "{}",
            profile.message
        );
        assert!(
            !profile.message.contains("codex-config approval_policy"),
            "{}",
            profile.message
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn check_reports_files_left_out_by_the_limit() {
        let root = temp_root();
        let rules = root.join("codex-home/rules");
        std::fs::create_dir_all(&rules).unwrap();
        for index in 0..(DIRECTORY_FILE_LIMIT + 2) {
            std::fs::write(rules.join(format!("r{index:03}.rules")), "").unwrap();
        }
        let check = check(&Locations {
            root: root.join("project"),
            claude_config_dir: None,
            codex_home: Some(root.join("codex-home")),
        });
        assert_eq!(check.status, crate::doctor::Status::Warn);
        assert!(
            check
                .message
                .contains("codex-user:rules=over_64_files_not_read"),
            "{}",
            check.message
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
