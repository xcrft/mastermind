# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `.mts` and `.cts` files are indexed as TypeScript.
- The TypeScript extractor records enums, type aliases (kind `type`) and
  namespaces (`namespace A.B { }` and `module A { }`), with their members as
  children. Interface methods, abstract methods and `declare function`
  signatures are methods and functions. An overload signature is left out
  when its implementation is in the same scope. A JSDoc comment on any of
  these declarations is concept documentation for it, and
  `symbols-changed-since` reports them like other symbols.
- TypeScript and JavaScript re-exports are `imports` edges of the module:
  `export { foo } from './a'` is `foo` / `./a::foo`. As with an aliased
  import, `export { foo as bar } from './a'` is `bar` / `./a::foo`, so
  `query imported-by bar` finds it. `export * from './a'` is
  `*` / `./a::*` and `export * as ns from './a'` is `ns` / `./a::*`.
  `import fs = require('fs')` is `fs` / `fs::*`.
- C/C++ namespaces are symbols that own their members, so a spec can name
  `billing::tax::rate`. An anonymous namespace adds no symbol. Declarations
  inside a namespace in a header are recorded as before.
- C/C++ `using X = ...` and `typedef` names are `type` symbols. A typedef that
  repeats its struct, union, enum or class tag adds no row, so
  `typedef struct Node Node;` and `typedef struct Node { } Node;` both leave
  only the struct. An opaque enum, `enum class Tone : int;`,
  `enum Level : char;` or `enum struct Mode;`, is an enum row. A plain
  `enum Color` with no body and no underlying type only refers to an enum and
  adds no row, as in `typedef enum Color Color;`.
  `namespace fs = std::filesystem;` is an `imports` edge `fs` /
  `std::filesystem::*`.
- `.inl` files are indexed as C++.
- The C# extractor records delegates (kind `delegate`), events (kind `event`),
  operators, conversion operators and finalizers as methods named
  `operator +`, `operator decimal` and `~Money`, and indexers as the property
  `this`. Calls in their bodies, in event `add` and `remove` accessors and in
  the initializer of a single field event belong to them instead of the
  enclosing type.

### Fixed

- A Pre-edit Snapshot item that starts with a file span, such as
  `` - `foo.vue` and `Widget` — 2 callers ``, claims `Widget`. It used to claim
  the file name. An item whose only span is a file, such as
  `` - `src/a.rs:12` — 5 callers ``, is `snapshot_line_unparsed`.

- A spec that names an indexed source file in backticks lists it as a
  mentioned file for every indexed extension. `.pyi`, `.vue`, `.phtml`, `.hh`,
  `.hxx`, `.inl`, `.ipp` and `.tpp` were missing. A `.clang-format` scope now
  covers `.hxx`, `.inl`, `.ipp` and `.tpp` files too.

- A C# `global using System.Text.Json;` is the import `Json` /
  `System.Text.Json::*`. The path used to start with `global using`, so it
  never matched the namespace.

### Changed

- The extractor contract is `mmcg-extractors-v11`. The next `index` after an
  upgrade rebuilds the codegraph.

## [5.0.0] - 2026-11-27

### Breaking

- `style.db` is now format 2. The first writable open converts format 1
  replacement records: a replaced preference or habit becomes `rejected`, and a
  habit created by renewal goes back to `candidate` for a new review. Each
  change gets a review event. Mastermind 4.0.x refuses a format 2 store.
- The automatic profile refresh after a task, the task-boundary drain and
  every command that writes the profile convert a format 1 store. A read-only
  command, such as `habit show` or a profile read, does not convert it. While
  the store still holds replacement records, it fails and says that the next
  automatic refresh or any writing command converts them.
- Task records refuse a repeated JSON key or a floating-point number:
  `state.json`, verification, invocation and review invocation receipts,
  preflight approvals, `review-task submit` reports and the guarded execution
  decision log. Records written by 4.0.x still read.
- Digests, receipt IDs, invocation IDs and review pins in those records must be
  lowercase hex.
- `miner feedback supersede`, `miner habit supersede` and `miner habit renew`
  are gone. To replace a claim, reject it and review a new one.
- The `mmcg_scratchpad_append` and `mmcg_scratchpad_read` MCP tools are gone.
  An existing index keeps the old table, and nothing reads it.

### Added

- `--skip-freshness-check` for `query`, `map`, `repo-map` and `impact` leaves
  out the walk of the source tree that checks if the index is current, and
  prints `freshness check skipped` on stderr. The flag only saves the time of
  that scan, which can take seconds per command on a large repository. The
  gain is largest for `query` and `repo-map`. For `query` the flag goes before
  the subcommand. Use it only when you index after every change and never on a
  working tree. For `query`, `map` and `repo-map`, when the index is behind
  the files the answer is old and has no warning. `impact` still compares
  changed and new files with the index and exits 1 with `index_stale` when
  they differ. The flag does not skip that. An index from an older extractor
  still warns, and a missing or old-schema index still fails.
- A skill-selection corpus of 91 prompts in
  `evals/behavior/cases/selection.jsonl`. Each prompt expects one skill or
  subagent, or none, and rejects the near misses. `just check` runs
  `python -m evals.selection`, a check of the case list and the catalogue that
  needs no model. It never decides which skill a prompt should get, so it does
  not show that a model picks well. It fails on a label the prompt's words
  contradict without a reviewed reason, a prompt that names its skill in any
  spelling, gibberish, a conflict case whose cited clause does not pick its
  winner in one direction, a known neighbour pair with no conflict case, a
  core skill that hands off outside core, and a corpus where more than a third
  of cases can be answered by rare-word matching. Cases fall into a fixed
  train and held-out split of about 60/40. The new `selection` behaviour suite
  sends the prompts to Claude against a catalogue of every skill and subagent,
  can run each prompt three times, and reports accuracy per split. Details and
  limits are in `evals/selection/README.md`.

- `mastermind doctor` has a `permission rules` check. It reads the Claude Code
  permission lists and modes, Codex `.rules` files, Codex `config.toml` and
  profile files, and what a repository ships for its clients: `.mcp.json`
  servers, `.claude/skills/*/SKILL.md` and `.claude/agents/**/*.md`. Each
  finding has a severity. An error fails the check and makes `doctor` exit 1:
  a `Read` rule for a secret store such as `~/.ssh` or `.env`, an MCP server
  or hook that downloads and runs code, a repository skill whose
  `allowed-tools` grants a broad shell rule, `defaultMode: bypassPermissions`,
  and Codex never asking while it has no sandbox or has network access. A
  warning covers any command, interpreters and shells, commands that run other
  commands, package managers, task runners and installs, `git` and `gh`
  without a fixed subcommand, `git push`, `gh api` and other publishing
  subcommands, network fetches without a fixed host, `WebFetch` and
  `WebSearch` for any host, `mcp__*`, writes to `.mastermind`, reads of the
  whole home directory, `enableAllProjectMcpServers`, a repository subagent
  with `permissionMode: bypassPermissions`, and Codex commands that inherit
  KEY, SECRET and TOKEN variables. A rule that a deny, ask or stricter rule
  covers is not reported. It never edits the files and never prints rule
  text, commands or addresses.
- `mastermind doctor` has a `client hooks` check. It reads the hooks in Claude
  Code managed, user, project and local settings and in enabled plugins, and
  in Codex system, user and project `hooks.json` and inline `[hooks]`. It
  counts the ones Mastermind installed and reports the rest. A hook counts as
  Mastermind's only when the program is the real mastermind launcher and the
  command names this project, so a copied marker does not hide a hook. A hook
  that downloads and runs code fails the check, and shell text, inline code,
  unpinned package runners, network tools and unknown programs warn. A
  malformed hook group is reported and the valid groups beside it are still
  read. Hooks turned off with `disableAllHooks` are still listed as warnings.
  The hook readiness checks now say how many other hooks exist instead of only
  "not configured".
- One bad hook in `~/.claude/settings.json` or `~/.codex/hooks.json` now fails
  `doctor` in every project, because a user hook runs in every project. The
  same holds for user MCP servers in `~/.claude.json`.
- The `permission rules` check also reads managed settings and
  `managed-mcp.json`, `~/.claude.json` servers and approvals, nested
  `.claude/skills`, credential and header helpers, the status line and file
  suggestion commands, `env` variables that load code or redirect model
  traffic, plugin marketplaces, third-party plugins, and Codex `notify` and
  provider token commands. Approving a flagged `.mcp.json` server is an error,
  and so is an allow rule that lets the agent edit `.codex`. It still does not
  read server-managed settings, files passed with `--settings`, `--mcp-config`
  or `--agents`, user-level skills and subagents, plugin skills and subagents,
  mods, or Codex skills. The reference lists them.
- The command check that `permission rules` and `client hooks` share is now
  structural. Any command piped into a shell or an interpreter that reads
  standard input is an error whatever produces it, so `http`, `ssh`, `nc` or
  `cat` of a temp file count as much as `curl`. So are a substitution or
  here-string run as program text, a file written by a redirect, `tee`, `-o`,
  `scp` or `rsync` that a later command runs, a build
  after `git clone`, awk `system()`, `find -exec sh`, `xargs` into a shell,
  and inline code that decodes and evaluates. The finding now reads "runs
  downloaded or generated code". `go run module@latest` joins the unpinned
  runner warning. Formatters and linters that read repository config, such as
  `prettier` and `eslint`, get a new info note that keeps the check ok, and a
  repository-relative `./node_modules/.bin` tool is no longer a known tool.
- A hook or MCP server counts as Mastermind's own only when its plain absolute
  path resolves to the install doctor runs from, or to that install's
  `bin/mastermind.js` run by a real `node`. A bare `mastermind`,
  `./node_modules/.bin/mastermind`, `npx`, a copy in a temp directory or
  `/bin/sh x/mastermind.js` is checked like any other command. Global npm
  setup now writes the node and launcher npm ran.
- `Edit` and `Write` rules are matched as globs against the files that set
  what an agent trusts. A rule aimed at `.codex`, `AGENTS.md` or
  `.agents/skills`, such as `Edit(.cod*/**)` or `Edit(.*/**)`, is an error.
  A bare or broad rule such as `Edit`, `Write(*)` or `Edit(~/**)` warns that
  it can edit them. `Edit(.claude/worktrees/**)` is not reported.
- `permission rules` also reads `gcpAuthRefresh`, treats
  `CLAUDE_CODE_SHELL_PREFIX` as an error and checks its value, reads
  commands, servers and environment policy in Codex profile files, reads the
  MCP servers of enabled plugins, and reads Codex approval modes on every
  server, local or remote, per tool, for apps and `apps._default`, and for
  plugin servers, plus `destructive_enabled`. A file's `N flagged` now counts
  servers, not findings. `client hooks` reads hooks files a plugin manifest
  names and reports each wrong-typed hook entry on its own.
- The command check drops shell comments, so `curl … | sh # x` is an error
  again, and reads `/dev/fd/0` and `/proc/self/fd/0` as standard input. A
  variable used as the shell (`$SHELL <(curl …)`, `| $0`), an archive
  extracted from a pipe and then run (`| tar xz && ./install`), input piped
  into `at`, `crontab -`, `docker run -i … sh` or `script -qc sh`, and a write
  to a shell startup file such as `~/.zshrc` or `~/.bashrc` are errors.
  `cat ./scripts/check.sh | bash` warns like `bash ./scripts/check.sh`.
  `npx --no` is no longer called unpinned. A program in a temporary directory
  and a launcher whose path an upgrade removed each get their own message.
- Only the node doctor runs with, or the node of the npm prefix it is
  installed under, can start Mastermind's launcher script. A file that is
  merely called `node` is checked like any other program.
- Edit rules are matched as whole globs with `[]` classes and `{a,b}` lists,
  so `Edit(**/*.ts)` and `Edit(**/test/**)` are no longer errors, while
  `Edit(.[c]odex/**)` and `Edit({.codex,src}/**)` are. Syntax doctor does not
  read is treated as broad. The bare `Edit` warning now says it can edit
  `.codex` and other trust files without a prompt in default mode.
- `permission rules` reads the monitors and LSP servers of enabled plugins,
  and warns on Codex `open_world_enabled = true` and
  `approvals_reviewer = "auto_review"`.
- Only `$SHELL`, `$BASH`, `$0`, a default that names a shell, a variable
  given a shell in the same command and a lookup such as `$(which sh)` count
  as a shell in a pipe. `| $PAGER`, `| ${PAGER:-less}`, `| $EDITOR -` and
  `| "$VAR"` warn again instead of failing.
- Standard input paths are normalised, so `/dev//stdin`, `/dev/fd/00` and
  `/proc/<any>/fd/0` count, and a newline after `|# c` still joins the pipe.
- A `node.exe` beside the npm prefix is trusted only on Windows and only for a
  global prefix, never beside a project's `node_modules`. The system nodes in
  `/usr/bin`, `/usr/local/bin`, `/opt/homebrew/bin` and
  `C:\Program Files\nodejs` are trusted after their links are resolved, so a
  real registration with a system node stays ok. The launcher script must sit
  in an `@xcraftmind/mastermind/bin` install.
- A `MASTERMIND_*` variable in Claude Code `env` or Codex
  `shell_environment_policy.set` is an error.
- Writes to `.git/hooks`, `.husky`, `~/Library/LaunchAgents`,
  `~/.config/systemd/user` and `~/.config/autostart`, a crontab written and
  then installed, and a pipe into a shell over `ssh`, `chroot`, `nsenter`,
  `unshare` or `lxc exec` are errors. A `.envrc` and an extract into
  `~/.local/bin` warn.
- An escaped glob character in an edit rule is literal, so
  `Edit(src/\[id\].tsx)` and `Edit(docs/\*.md)` are not reported.
- `scripts/validate.py` now runs a line-by-line copy of doctor's command check
  in `scripts/command_check.py`. It loads doctor's word tables from
  `command_rules.json`, which a Rust test keeps equal to the Rust constants,
  and both copies must give the verdicts in `fetch_exec_corpus.json`, a corpus
  of every counter-example so far.
- Static detection cannot be complete: a command can build what it runs in
  ways no reader of its text can follow. A passing check is not proof that
  nothing runs without asking. Known gaps, such as a variable set outside the
  command or `| ssh host` with no remote command, are listed in the reference.
  A per-role `PreToolUse` guard hook that checks each command when it runs is
  the follow-up, in its own pull request.
- A package installed from a registry without an exact version, followed by
  any command, warns that it runs a package without an exact version, the
  same as `npx -y`, since its programs need not share its name:
  `npm i -D eslint && npx eslint .`, `pip install pytest && pytest`,
  `cargo install evil-crate && evil`. A pipe into a shell, a file written and
  then run, and decoded code run after the install are still errors. With an
  exact version, as in `npm i -g prettier@3.3.3 && prettier --check .`, it
  warns as shell text. A repository Go build such as `go build -o bin/tool ./cmd/tool && ./bin/tool` or
  `go install ./cmd/tool` warns like `cargo build` and `make`.
- `/proc/self/fd/../fd/0` reads as standard input, and `$(whence sh)`,
  `$ZSH_NAME` and `$ZSH_ARGZERO` count as a shell in a pipe.
- A hook copied or linked from the checkout into `.git/hooks` warns.
  Downloaded content written into a hook or into `.claude/settings*.json`,
  `.mcp.json`, `~/.claude.json` or `.codex/config.toml` is an error, and so is
  any write to `$GIT_DIR/hooks`, `/Library/LaunchDaemons`, `/etc/cron.d`, a
  crontab spool or `~/.local/share/systemd/user`. `.vscode/tasks.json`,
  `~/.ssh/rc`, `.git/config` and `core.hooksPath` are listed as known gaps.
- `scripts/validate.py` also runs doctor's persistence check, and fails a
  frontmatter hook that writes a shell startup file or a file that runs later
  on its own. The lookups, shell variables, remote shells, temporary folders,
  package managers, startup files and persistence paths now live in
  `command_rules.json`, and the corpus has a persistence section. The copy is
  kept by hand, so only corpus commands are sure to agree.
- Every shipped subagent declares its capability class in its workflow
  metadata: `mutability` plus `capabilities`, where `shell` and `web` each need
  a reason. `just validate` fails when a role's `tools` do not match: `Edit`,
  `Write`, `NotebookEdit` or `MultiEdit` in a read-only role, `Bash` or
  `PowerShell` without `shell`, `WebFetch` or `WebSearch` without `web`, or a
  declared capability no tool needs. `Agent` and `Task` need `delegate`, tools
  of MCP servers other than mmcg need `mcp`, and `BashOutput`, `KillShell`,
  `Monitor`, `REPL` and `TaskStop` count as shell. `Workflow`,
  `SendMessage`, `CronCreate` and `RemoteTrigger` need `delegate`, `Skill`
  needs `skills`, and a subagent with frontmatter `hooks` must declare
  `hooks`, and each hook command is checked for fetched, written or decoded
  code, read the way doctor reads it, so `curl -so a URL && sh a` and
  `| sudo bash` fail. `mcp_tool` hooks are valid, and one on a server other
  than mmcg needs `mcp`. Tools that publish, send or change the session, such
  as `Artifact`, `PushNotification`, `SendUserFile` and `EnterWorktree`, need
  the new `outbound` or `session` capability, which a read-only role can never
  hold. The MCP resource tools need `mcp` and `ScheduleWakeup` needs
  `delegate`. A read-only role may only hold known read tools, so a new tool
  fails until it is classified. A read-only role cannot ask for `bypassPermissions` or
  `acceptEdits`, a fixed deny list keeps the shell from the researcher and
  four other roles, and the comment and frontend auditors no longer get
  `Bash`. Six subagents that run commands declare `shell` with a reason.
- One untrusted-content paragraph in `agents/blocks/untrusted-content.md`.
  Ten subagents and 28 skills that read repository, pull request, report,
  transcript or tool content carry it verbatim. It names the person or the
  agent that started the role as the only source of instructions, and keeps
  the approved spec and declared checks part of the request. `just validate`
  fails when a covered file lacks it, has it twice, has a changed copy or
  holds it inside a code fence, an HTML comment or another paragraph.

- Five skills that are safe to point at an untrusted pull request:
  `mastermind-diff-review`, `mastermind-docs-authoring` and
  `mastermind-verify-change` in every profile, and
  `mastermind-dependency-upgrade` and `mastermind-scanner-alert-response` in
  the `security` and `full` profiles. They read repository instructions from
  the base branch, treat everything in the change as data, never run the
  change's scripts with credentials, and never fetch a secret's value. The
  `core` profile now installs 23 skills.

### Changed

- Every subagent description now opens with when to delegate, says
  `Not for ... (use X)` within its first 200 characters and stays within 300.
  Claude Code decides when to delegate from these descriptions, so the two
  roles that run only on request, the feedback collector and the prompt
  refiner, start with `Use only when`. `just validate` fails a subagent
  description that breaks this, says "proactively", "MUST BE USED" or another
  phrase that pushes delegation, or has a `Not for` boundary with no
  `(use X)` that resolves. In skill and subagent descriptions every hand-off
  must be a lowercase `(use <full-name>)` naming something shipped, and names
  may not appear outside one. Each subagent has a new patch version.

- Skill descriptions now pick one winner for four prompts that two skills
  both claimed. `mastermind-test-impact` hands running checks to
  `mastermind-verify-change`, `mastermind-architecture-review` hands a plan
  check before approval to `mastermind-critical-review`, and
  `mastermind-diff-review` hands React and Vue code to
  `mastermind-frontend-audit`. `mastermind-agent-security-review` already
  claims changes touching auth and secrets, and `mastermind-diff-review` is a
  core skill, so it does not name it. `mastermind-agent-handoff`,
  `mastermind-component-research` and `mastermind-product-intake` no longer
  hand off to skills that the core profile does not install.
  `mastermind-design-intake` now claims a mockup that comes with a ticket.

- `mastermind-dependency-upgrade` now does what its description promised for
  a new package. It checks the name against the registry and the project's
  documentation, looks for lookalike names, notes package and version age and
  maintainer changes, respects a release-age cooldown and stops for a person
  when a check fails. Registry lookups run outside the checkout and name the
  public registry, so a pull request's `.npmrc`, `pip.conf` or
  `.cargo/config.toml` cannot answer them, and any change to registry settings
  is reported. Three workflow eval cases cover a days-old lookalike, an
  established package that should pass and a registry override in a pull
  request.
- `mastermind-diff-review` has a "Hidden text" section. It lists where
  steering text hides: HTML comments, collapsed blocks, alt text, link titles,
  reference links, inline HTML or CSS that hides text, very long lines,
  minified or unrendered diffs, encoded strings, and invisible Unicode,
  including zero-width and word-joiner characters, the byte-order mark, soft
  hyphens, bidi controls, tag characters and variation selectors. It gives a
  command that prints `file:line` for each added line with one of those
  characters, with hooks, external diff tools and textconv off so nothing from
  the head runs. The command flags quoted file names and cannot be fooled by
  an added line that looks like a file header. Lookalike letters get a
  separate manual check. The skill also checks that the code keeps the promise
  in the title and description. Five workflow eval cases cover a bidi
  override, tag and zero-width characters, a hidden HTML comment, a fix
  applied to two of three templates and the safe search command.
- `mastermind-agent-security-review` and the security auditor check every
  agent context for the three legs of data theft: private data, text from
  others and a way to send data out. All three together is at least P1 even
  with a prompt guard, and the fix must remove one leg with a control the
  model cannot override. Two workflow eval cases cover a triage bot with all
  three legs and a local tool with none of the outbound ones.
- `mastermind-test-audit` and the test auditor have a fifth check. For each
  changed line that decides something, they name the most likely small slip,
  such as `<` for `<=` or a dropped branch, and the assertion that would fail
  on it. When none would, they report a `survivor`, at most one per symbol and
  called likely rather than proven. Survivors are advisory: they go in their
  own section and an `advisories` list in the structured tail, and do not
  count towards `flagged` or the verdict. Two workflow eval cases cover a surviving boundary slip
  and the clean case where both sides of the boundary are asserted.
- `mastermind-investigation-ledger` starts from a failing command and only
  then writes hypotheses. Every run, the bisect and the red/green proof
  included, goes through the `mastermind-verify-change` trust gate as written
  there. A regression with a known good version is bisected over a range that
  passed the gate, with `git -c core.hooksPath=/dev/null` on every git call,
  and the skill warns that `git bisect run npm test` runs each old commit's own
  scripts. An intermittent failure is measured as a rate, and a cause is
  confirmed only when removing it turns the failing command green. It ends
  with a fixed report. The investigator subagent gets the same gate and bisect
  rules and stays within its 3,500-character prompt. Four workflow eval cases
  cover the fork, bisect, hooks and one-green-run behaviour.
- Every skill description now opens with when to use the skill, then a
  `Not for` clause that names the sibling to use instead, for example
  `mastermind-verify-change` for a red CI run and
  `mastermind-investigation-ledger` for a cause that is still unknown, then a
  short line on what it does. Hosts pick skills from these descriptions alone.
  When Codex's skill list is over budget, Codex cuts every description to the
  same length from the end, so both clauses now start within the first 200
  characters. The 32 descriptions total 7,631 characters, down from 10,032,
  and the 23 in the core profile total 5,471, down from 7,110. `just validate`
  now fails a skill description that is longer than 300 characters, or has no
  `Use when`, `before`, `after`, `for` or `to`, or no `Not for` clause, in its
  first 200 characters. The skills README has a short guide to writing a skill.
- The covered skills and subagents use the shared untrusted-content paragraph
  in place of their own shorter wording, and each has a new patch version.
  The critic opens with its own short data-not-instructions rule instead. The
  FIND precondition notes and the legacy `VERIFY` forms moved from the planner
  to its spec template.

- An index written with a newer extractor or concept contract is refused by
  every open, the same as a newer schema version, and is never rebuilt.
- A symlink or other non-regular source no longer makes the index stale for MCP
  queries, `status` and `doctor`. It is still listed in `unindexable_sources`,
  and `index` and `ci` still exit 1 for it. The `index` error now lists such
  sources apart from retryable failures, which keep the index stale.
- When `index` skips more submodules than it lists, it prints the total.
- `enrich` takes the index writer lock for fact and SCIP imports.
- Project history, workflow status and history review use one rule for task
  folders, so a loose `*.md` file in `.mastermind/tasks` is never a task.
- `mastermind update` checks every selected client before it writes anything
  and lists each edited, linked or unowned workflow entry with a `Next:` step.
  `--move-aside` moves them to `<client root>/.mastermind-preserved/<time>/`
  and installs the bundled copies.
- Updating from 4.0.x: the first `mastermind update` still runs the 4.0.x
  updater, which has no `--move-aside` and stops at the first edited, linked or
  unowned entry. Move those entries out of `~/.claude/skills`, `~/.claude/agents` and
  `~/.codex/skills`, then run `mastermind update` again.

### Fixed

- The frontend audit checks props contracts with `mmcg_change_impact`, which
  includes uncommitted work. `mmcg_symbols_changed_since` only compares
  commits, so it missed a changed component before the commit step.
- The runtime, component and security research skills pass `max_depth` to
  `mmcg_impact`; it has no `depth` argument.
- The comment auditor no longer counts `could_not_verify` as a finding, so an
  unjudged comment cannot turn a clean result into `findings`.
- The comment, test and frontend audits have an `incomplete` verdict. A review
  that stopped early, for example with no baseline, no longer reads as
  `clean`.
- The test and frontend auditors no longer re-index the repository. A query
  refreshes a stale managed index once; if it is still stale they report
  `could_not_verify`.
- The router says the feedback collector writes to the private review inbox
  only when asked; it never edits the profile.
- The auditor's report tail uses a repository-relative spec path, and the
  report contract lists the auditor's full set of fields.
- The planner links its spec template, defect labels, design review packet and
  report schema references.
- `miner profile --deep` help says it saves a separate candidate file for
  review instead of replacing part of the profile.
- A 4.0 task is no longer refused because an `executor-report.md` or
  `audit.md` sits beside its spec. Only a task started before 4.0 gets the
  old-name refusal, and a spec named `audit.md` never counts as its own old
  audit.
- `review-task` with a missing spec or a `_draft` folder no longer leaves a
  controller lock or run-state directory behind.
- Closing the terminal stops a running verification check, and its receipt is
  `interrupted`.
- Terminal control characters from the executor report are escaped in its
  parse errors.
- `audit verify` and workflow status fail at once on a FIFO in place of a file,
  instead of hanging.
- An audit bundle `snapshot_changed` error names its reason, such as
  `snapshot_changed: worktree_not_clean`.
- `run-task` says why it stopped when the task state disappears or cannot be
  read after a review.
- `serve` no longer fails with `database is locked` when it opens the index
  right after an incremental `index`.
- `watch` waits for another index writer with full refreshes that never use up
  its three retries. It checks a removed path again under the writer lock and
  parses under the lock, so it never commits content older than another
  writer's.
- Outside Git, a directory the walk may not read is skipped when the index
  holds no source under it. Before, it failed the run.
- `mmcg_context` and `mastermind context preview` return
  `context_index_unavailable` for an index that exists but does not open, for
  example one a newer mmcg wrote. Before, they built the packet without it.
- `mmcg_unreferenced` hides only names that start with a literal `test_` in
  test files. `_` was a wildcard, so names such as `testing` were hidden too.
- `serve` prints a warning when it cannot remove its index snapshot on exit.
  On Windows a snapshot that SQLite still holds open stays behind.
- The hook journal no longer fills up with rows from ended sessions. Archiving
  drops their event receipts and shrinks intake and task-mining rows to a
  summary that `hooks show` still reads.
- A full hook journal records the `journal_full` gap, `hooks status` lists the
  steps to free space, and `hooks forget` works on a full journal.
- One hook delivery spends at most 2.5 s in total waiting for the journal
  lock, so a busy journal still leaves time to record the gap.
- `hooks recover --project-root PATH` works after the project folder was
  deleted or moved, and `doctor` prints that command for each such project.
- `doctor` shows why hook readiness is unavailable, such as a journal from a
  newer version.
- `.DS_Store`, `__pycache__` and `*.pyc` files in installed workflow folders no
  longer count as edits.
- `init` and `status` print the reason and the `Next:` steps when a workflow
  update fails, and `doctor --workflow` reports a linked or misplaced client
  folder as a layout error with a recovery step.

### Removed

- The `mmcg_scratchpad_append` and `mmcg_scratchpad_read` MCP tools. An
  existing index keeps the old table, and nothing reads it.
- `miner feedback supersede`, `miner habit supersede` and `miner habit renew`.
  To replace a claim, reject it and review a new one.
- The generation column in `habit list`, and the relation and generation lines
  in `habit show` and `feedback show`.
- `docs/README.md`, `docs/architecture.md`, the Claude Code and Codex
  integration pages and `docs/reference/persona-context.md`. Their content now
  lives in the README, getting started, effort routing and the persona
  reference. Eval narratives that only retold a retained JSON record are gone,
  and the scorecard links the JSON directly.

### Development

- `just bootstrap` and `just sync-templates` no longer run `just check` and
  `just validate` by accident: their closing hint quoted the command in
  backticks, which the shell executed.
- Every workflow installs Rust from `rust-toolchain.toml`, and the validator
  checks that the Docker Action builds with the same channel.
- `just release-tags` dispatches the npm and crates.io publish dry runs and
  waits for them and push CI. The publish verify job no longer repeats the
  tests in release mode.
- New `just release-check` and `just test-linux`. `just index`, `just outline`
  and `just recent` are gone.
- Dependencies: notify 9, cargo-deny-action 2.1.4 with `unmaintained = "all"`,
  PyYAML 6.0.4 and a lockfile refresh.

## [4.0.1] - 2026-11-26

### Fixed

- Mining and native runs with Claude Code work again. The `claude` executable
  is now hashed as a stream with a 2 GiB limit. It was read whole into memory
  with a 256 MiB limit, and Claude Code ships as one binary of more than
  350 MiB.
- Task mining with Claude no longer gives up when it reads the session
  transcript while Claude is still writing it. A transcript that changes during
  the read, reads slowly, or ends inside a character now counts as not ready,
  and the finalizer keeps waiting for the response. Before, the episode stopped
  with `finalizer_failed` until a later retry.
- A relative or empty `PATH` entry, such as the `~/.dotnet/tools` entry the
  .NET installer adds, no longer breaks `init`, setup or verification commands.
  Executable lookup skips those entries, since what they find depends on the
  working directory. `mastermind doctor` names them as a warning and explains
  that `~` is not expanded inside `PATH`.
- `mastermind doctor` accepts the `.mastermind/.gitignore` that `init` writes,
  so the project's own `.gitignore` no longer needs a `.mastermind/` line.
- `mastermind doctor` no longer fails a repository that has no source in a
  supported language, such as one with only scripts, YAML or Terraform. The
  empty index is reported as OK with the reason.
- `mastermind init` replaces an unmarked `## Mastermind workflow` section left
  by a 3.x `init` with the managed block, even when the section came from an
  older template or was edited. The previous file is kept as
  `CLAUDE.md.mastermind-backup` (or `AGENTS.md.mastermind-backup`). Before,
  `init` left the file unchanged and only recorded a warning in `--json`.
- `mastermind init` prints the project step's warnings in its normal output.
- `mastermind doctor` hints name commands that run as written:
  `miner hooks status` and `miner hooks recover` get `--client <client>`, and
  missing MCP config or ignore rules point at `mastermind init`.

## [4.0.0] - 2026-11-25

### Security

- `pulldown-cmark` is updated to 0.13.6 for RUSTSEC-2026-0433. Mastermind
  does not enable wikilinks, so the quadratic parse was not reachable.
- `rusqlite` is updated to 0.43.0 and the bundled SQLite to 3.54.1
  (`libsqlite3-sys` 0.40.0) for RUSTSEC-2026-0459. Older SQLite could return
  uninitialized memory when a database and its `-wal` file had different page
  sizes. `mmcg` opens the index with its `-wal` from the checkout, so a
  repository that ships a crafted pair could reach it.

### Added

- `mastermind init --worktree-max-total-bytes N` and
  `--snapshot-max-total-bytes N` (1 to 1,073,741,824 bytes) set the two total
  byte budgets that were fixed at 32 MiB. The worktree budget bounds the prior
  local files a task's initial worktree may hold; its first pre-flight records
  it in the manifest, so it does not change for an open task. The snapshot
  budget bounds the files the history, audit and strict workflow snapshots read,
  including the task's prior files, so it should cover the worktree budget plus
  the task's own changes;
  every pre-flight pins it in `state.json` and its approval record, and a new
  value reaches an open task through `run-task <spec> --pre-only`. A budget that
  runs out names its setting, limit, counted bytes, the next file and the flag,
  where the history snapshot used to report a small file as over a "limit" that
  was the bytes left. Defaults stay 32 MiB. A release that predates the setting
  rejects task state written with these fields and a `setup.json` that holds
  the keys.

### Changed

- **Breaking:** `mmcg index` exits 1 when a source file fails to index,
  including a symlink or other non-regular file, after it prints the failed
  paths. Scripts that treated a finished run as success need to handle the new
  exit code.
- **Breaking:** `facts adapt` requires `--report-revision OID`, the full
  commit the report was produced from, such as `$GITHUB_SHA`. Existing adapt
  scripts fail until they pass it. The adapter used to stamp the indexed HEAD
  on a report of unknown revision. The OID must equal the indexed HEAD, and a
  SARIF run that declares `versionControlProvenance` for this repository must
  name the same revision (`artifact_revision_mismatch`).
- **Breaking:** Setup accepts a project-local npm launcher only when it is an
  absolute path ending in `bin/mastermind.js` run by an absolute `node`. Other
  values fail with `npm_launcher_path_invalid`.
- **Breaking:** `state.json` records `format_version`. State without it is
  read as format 1. State with a newer format is refused with its version and
  a request to use the newer binary, where it used to look damaged and the
  advice was to restore it. A release before this one rejects all state this
  release writes, so finish or close open tasks before going back.
- **Breaking:** `mastermind ci` fails and names the files when a source file
  fails to index. Before, it audited the partial graph and could pass.
- **Breaking:** Legacy flat and other non-`spec.md` specs get their own
  executor report and audit, named after the spec, such as
  `050-a.executor-report.md`. Before, two such specs in one folder read and
  overwrote each other's report and audit, and a finished task could fail its
  audit with `task_mismatch`. When a task of such a spec already has state and
  an old `executor-report.md` or `audit.md` sits beside the spec, `run-task`
  stops before it changes the state and prints the `mv` command for each file,
  or `rm` when the new name already exists. A `.gitignore` that lists
  `executor-report.md` or `audit.md` for such specs needs the new names too.
- **Breaking:** Reapproval of a task approved before this change fails with
  `preflight_approval_schema_outdated`. Start a new task. The older parser
  could have dropped preconditions that the edited files can no longer prove.
- **Breaking:** A task started before this change fails preflight and
  postflight with `initial_worktree_schema_outdated`. Its initial worktree
  manifest missed the flagged edits, which would then be charged to the
  executor. Start a new task. Its state stays readable.
- **Breaking:** `audit-spec` and `ci` refuse a change list over 10,000 files
  or with a non-UTF-8 path (`change_inventory_incomplete`). They used to
  report Held for the files they could list.
- **Breaking:** `verify-spec` no longer evaluates index checks against an
  index that no longer matches the files. It reports `not_evaluated`, and
  `--require-index` fails. Standalone `audit-spec` refuses such an index
  instead of holding.
- **Breaking:** `mastermind ci` fails when `--changed-only`,
  `--require-executor-report` or `--bundle-dir` is set and no task contract
  exists. It used to pass when `.mastermind/tasks/` was missing.
- **Breaking:** The hidden compatibility flags are removed. `mastermind init`
  no longer accepts `--no-claude`, since drafting is already off unless
  `--draft-with claude` is set. `mastermind setup` no longer accepts
  `--dry-run` (run it without `--write`), `--write-mcp` (use `--write`) or
  `--project PATH` (use `--scope project --root PATH`).
- `mmcg_dependency_cycles` resolves an import only within its language family.
  JavaScript, TypeScript, TSX and Vue form one family. A name shared by a
  Python and a Rust file no longer closes a cycle between them.
- Index writes take an exclusive writer lock file next to the database, such as
  `.mastermind/mmcg.db-writer.lock`. A refresh that waits too long for it
  returns `index_busy` with `retryable: true` instead of
  `work_limit_exceeded`, and `mmcg index` fails with `the index is busy`.
- An index written by a newer mmcg is refused with an error that names both
  schema versions. Before, the older binary dropped and rebuilt its graph.
- Dependencies are on their latest releases: `rusqlite` 0.43,
  `tree-sitter-c-sharp` 0.24 and compatible updates of the other crates. The
  C# grammar reads C# 14 `extension` blocks, and their members are indexed as
  methods of the enclosing class. Some C# code parses differently, such as
  `(x) && y`, which is now a logical AND instead of a cast. The audit Docker
  Action builds with Rust 1.100.0, and the PR review workflow pins the current
  `dtolnay/rust-toolchain` commit.
- `MMCG_TASK_FINALIZER_WAIT_MS` sets how long the persona task finalizer waits
  after Claude's Stop hook for the transcript to name the model. The default
  stays 4 seconds. A waiting finalizer renews its 10 second lease, so a longer
  wait is not mistaken for a crashed finalizer. The finalizer tests set the
  wait themselves, so they no longer fail when a loaded machine runs them
  slower than that window.
- `init` keeps the workflow instructions in a managed block. It appends the
  block to an existing `CLAUDE.md` instead of skipping the file, refreshes an
  older block, and writes the same block to `AGENTS.md` for Codex.
  Text outside the block keeps its bytes, and the block follows CRLF files.
  `--workflow off` writes no block.
  `doctor` compares the block in each saved client's file with the current
  template.
- The `core` profile now includes `mastermind-runtime-research`,
  `mastermind-component-research`, `mastermind-frontend-audit` and
  `mastermind-browser-verification`. The workflow router names them, so a
  default install used to point at skills it did not have. `frontend` adds
  `mastermind-design-intake`.
- The planner names a Strict review that works without subagents, such as in
  Codex: `mastermind-critical-review` in a fresh child agent.
- `persona-events.db` and `style.db` record their format version in SQLite
  `user_version`. A store written by a newer mmcg is refused with an error that
  names both versions. The `style.db` feedback re-key runs once per format
  version instead of on every open.
- A hook delivery stops archiving after 250 ms once it has archived one
  episode, so it stays within its 3 s timeout. The journal waits up to 2 s for
  the writer lock instead of 0.5 s. `hooks archive` has no time bound.
- The PR audit example pins `xcrft/mastermind` to a commit instead of
  `uses: ./`, which only worked inside this repository and let a pull request
  change the audit code. The publication comment shows the envelope verdicts
  and marks only `held` with ✅.
- The `.mastermind/.gitignore` that `init` writes carries the lines that commit
  task contracts and executor reports, commented out. The GitHub audit Action
  needs them, and the old `!path/to/file` advice did not work.
- A relative path the runtime treats as repository-relative now always names a
  file from the root, whatever the working directory. Before, a command run in
  a subdirectory read `sub/.mastermind/...`, so `miner hooks bind-task` failed
  there even with absolute arguments and the hooks lost the task binding.
  Command-line paths that name files from the working directory, `--root`
  among them, are made absolute first, so `workflow audit` prints the absolute
  root.
- Entries under `.mastermind/tasks` that start with `_` or `.` are never tasks.
  `ci` no longer audits `_draft/spec.md`, which `status` did not show, and
  `run-task` refuses such a spec instead of creating state that `status` and
  `next` never list.
- `run-task` refuses a spec path that names the file differently from disk,
  such as `SPEC.md` for `spec.md` on macOS or Windows, and prints the path to
  use. Before, the second spelling got its own state, lock, iteration budget
  and artifacts for the same task. Refusing keeps one name per task without
  rewriting the path the user gave: the on-disk name comes from the same
  `canonicalize` that macOS and Windows use to report the stored case.
- `setup` keeps the key order of the client config files it edits, such as
  `~/.claude.json`. It used to rewrite them with the keys sorted.
- A stored semantic review record with a repeated JSON key at any depth fails
  with `semantic_review_record_invalid`.

### Fixed

- `run-task` with a spec path that does not exist fails before it takes the
  controller lock. Before, it left a lock file under `.mastermind/run-state/`.
- `mmcg serve` starts while `mmcg index --force` runs. Opening a current index
  no longer writes its schema, and serve no longer copies a snapshot just to
  read the index root. Before, most starts during a rebuild failed with
  `index changed while creating a read-only snapshot`.
- Two index runs no longer interleave their writes. The second waits for the
  writer lock.
- A source that gains a NUL byte after its first 8 KiB leaves the index.
  `mmcg watch` also drops a source that becomes binary or grows over 5 MiB.
  Before, its old symbols stayed and the index looked fresh.
- A symlinked source no longer keeps the extractor and concept contracts dirty
  and no longer stops `mmcg watch` after three retries.
- A source that fails to read keeps its indexed rows. Before, one transient
  read error purged its symbols.
- Outside Git, a directory the walk cannot read fails the index. Before, its
  sources were purged as if deleted.
- Git submodules are counted as skipped submodules in `mmcg index` and
  `mmcg_status`, instead of as unsupported paths.
- `mmcg watch` ignores renames under skipped directories such as `target` and
  `node_modules`. Before, each one ran a full refresh.
- `mmcg watch` purges a removed path only when it is still gone after the
  debounce, so an editor's atomic save keeps the file's rows.
- A schema migration runs in one transaction and fails the open when a column
  cannot be added. Before, failed `ALTER TABLE` statements were ignored and an
  index without the column passed as current.
- A failed MCP refresh that finished its run reports `unindexable_sources`
  instead of `index_rebuild_required` when it used up the budget.
- `mmcg serve` removes its private index snapshots when the watchdog exits the
  process.
- The context packet retries a read-only index snapshot that a concurrent write
  invalidated, then reports `context_index_snapshot_unavailable`. Before, it
  was built without the index and delivery failed as changed sources.
- A long turn no longer stops capture. Once an episode record reaches 320 KiB,
  text of later events is dropped with an `episode_size_limit` gap, and more
  than 128 open tool calls record `episode_tool_limit`.
- `hooks recover` tolerates markers that other deliveries clear during its
  listing. When it cannot remove every marker, it keeps the old generation and
  stops capture with a `capture_recovery_incomplete` gap.
- `hooks status` and doctor report the capture marker count and the oldest
  marker age. A marker with no journal gap gets its own next action that names
  `mastermind miner hooks recover`. Doctor's `capture fence markers` check
  counts markers in every project and the prefixes with no known grant.
- An unexpected entry such as `.DS_Store` in `~/.mastermind/.persona-capture`
  is named before a marker is created. Markers no longer pile up, recovery
  keeps the generation, and status no longer reports the journal as
  unavailable.
- Another session's delivery in flight no longer degrades prompt intake or
  skips a task-mining offer. The hook waits up to 1.5 s for it, and fails
  closed at once when the other delivery's marker is older than any hook can
  run.
- A `pending` intake whose processor timeout plus 3 s has passed reads as
  `degraded` with reason `processor_interrupted`.
- Doctor accepts the `node bin/mastermind.js serve` registration that setup
  writes for a project-local npm install. Setup and doctor share one rule.
- When native setup has removed the old registration and the new add fails,
  setup adds the old entry back and reports `native_add_failed_restored` or
  `native_add_failed_not_restored`.
- The npm wrapper forwards SIGTERM, SIGINT and SIGHUP to `mmcg` and exits with
  its status, so stopping an MCP server no longer leaves `mmcg` running.
- Persona CLI tests clear the inherited environment, so an exported
  `MMCG_INPUT_ORIGIN` or `CLAUDE_CONFIG_DIR` no longer breaks them.
- Prompt intake no longer degrades because another project or client finished
  a capture at the same moment. The capture-pending check treats a marker
  cleared during its read as absent and rereads a listing that changed, instead
  of reporting the project as pending.
- Post-flight no longer asks to acknowledge removals that are not API
  removals. A function declared inside a function or method body in Python,
  Rust or C# is local, and a file moved with its bytes unchanged to indexed
  source keeps its declarations, so a pure `git mv` of a component with local
  helpers passes without `removed_symbols`. Each destination covers one deleted
  file. A move with content changes, or to a path the index skips such as
  `api.py.orig`, still counts the old path's declarations as removed. Nested
  declarations that callers can reach still need an acknowledgement: methods of
  an object literal or class expression a JS or TS function returns, nested PHP
  functions and members of a type declared in a function.
- The observed verification snapshot now reads files up to its 512 MiB pass
  budget, so a tracked file over 128 MiB no longer stops pre-flight. When the
  budget runs out, the error names the file, its size and the bytes left. A
  read failure also names the file. An uncommitted file over 5 MiB still stops
  pre-flight, and the error now says how to clear it.
- An index that lacks a table the query tools read now counts as outdated, so
  `mmcg_docs` and `mmcg_project_profile` answer `schema_incompatible` for an
  old read-only index instead of an internal error.
- `doctor` looks for the Claude MCP registration where `setup` writes it, under
  `CLAUDE_CONFIG_DIR` or an existing `.config.json`. Before, it reported
  `mmcg_registration_missing` as an error for such installs.
- An indexed file that becomes ignored now counts as stale, so MCP refresh drops
  its symbols. Before, `mmcg_status` reported fresh and search kept serving it.
- The 100,000-file auto refresh cap counts only files in a supported language.
  Before, a repository with many data or text files hit `refresh_limit_exceeded`
  on every MCP call, and `mastermind index .` did not clear it.
- `run-task`, `review-task`, `verification run`, `acceptance status`,
  `verify-spec` and `audit-spec` accept an absolute spec path that reaches the
  repository through a link, such as `$PWD` in a linked directory or `/var` on
  macOS, or through `..`, as `../spec.md` does in a subdirectory. Before,
  `audit-spec --bundle` refused such a spec and a failed audit dropped its
  lesson candidate without an error. `audit-spec --executor-report` resolves
  its path the same way as the spec.
- `audit-spec --json` reports a lesson candidate it could not write on stderr.
  It used to drop the error.
- `init` counts a legacy flat spec once. Its own `050-a.audit.md` and
  `050-a.executor-report.md` were counted as more specs, and the migration
  example could name one of them.
- Error messages that name a file from Git, such as a snapshot or initial
  worktree file over its limit, escape terminal control characters in the name.
- `status` names the real report or audit file of a failed post-flight, such
  as `a.executor-report.md`, instead of `executor-report.md`.
- The executor skill, its agent and the report contract tell the executor to
  write the report at the path `run-task --pre-only` prints. They named
  `<task>/executor-report.md`, which post-flight does not read for a spec with
  another name.
- `miner hooks show` no longer marks a fresh analysis as not current when the
  task finalizer binds the model and records the analysis while `show` reads.
  The episode revision and the analysis receipts are now read together.
- `facts adapt` no longer rejects a whole report as "not all mapped" because
  of entries that carry no file fact, such as an OTLP span without a code path
  or a coverage line repeated across methods. Entries without a path or outside
  the index are named with counts (`report_entry_without_path`,
  `report_entry_outside_index`).
- `facts adapt` turns control characters in titles, messages and labels into
  spaces and cuts text over the manifest limits with `…`. A multi-line SARIF
  message or a JUnit failure trace used to fail the import.
- `facts adapt` refuses a cut report: JUnit or Cobertura XML without its
  closing elements, an LCOV record without `end_of_record`, and a report with
  no facts. Only an empty SARIF scan clears a dataset. Before, a truncated CI
  upload could replace a dataset with part of it or with nothing.
- A JUnit suite that lists fewer cases than its `tests` total, or declares
  failures without a failed case, is partial in Lens and refused by the
  adapter (`junit_totals_mismatch`).
- `query facts` and `mmcg_facts` return error annotations before warnings and
  notes. When `--top` cuts a source, that source is `partial` with a
  `normalized_fact_limit` diagnostic, instead of `loaded`.
- Lens marks a fact source partial when its findings are dropped at the
  per-file or global cap, and review export no longer attests such a source as
  producer-attested.
- Persona capture hooks no longer fail when parallel tool calls deliver
  events at once. Another hook's commit changes SQLite's rollback journal while
  a hook opens the journal, and the open refused it with `file identity changed
  during read`. Claude Code showed this as a hook that gave no usable answer,
  and each failure left a pending capture marker until `miner hooks recover`.
- `verify-spec` and `audit-spec` refuse spec lines they cannot read instead of
  dropping them. A frontmatter block after a BOM or blank line, or with a
  trailing space, fails as `frontmatter_not_at_start`. A snapshot line that
  states a caller count or signature the parser cannot read, such as `~40
  callers`, or states claims for several symbols, fails as `unparsed_line`, and
  so does an upper-case FIND with text after its colon or inline code, plain
  words above its fence, an empty fence or no fence, an empty upper-case
  `VERIFY:`, a `VERIFY` line with several commands, and a snapshot heading that
  is not a top-level `##` section. Snapshot prose without a count or signature
  stays a note. Every top-level `##` heading that starts with `pre edit` and
  names a snapshot is read. FIND, VERIFY and File markers are read inside list
  items, and a `**File**:` marker no longer leaves FIND checked against the
  previous file. A `- [ ] VERIFY: commands are runnable` checklist entry is no
  longer a declared command.
- A heading indented past a list item's content no longer splits spec sections
  or resets the FIND target.
- The audit change list now includes edits that `assume-unchanged` or
  `skip-worktree` hide from `git diff`. Git diffs those files again on a copy
  of the index with both flags cleared, so its own attribute, mode, symlink
  and removal rules apply. In a sparse checkout, an absent `skip-worktree`
  file is not read as a deletion. Standalone `audit-spec`, `ci` and the
  task history binding use the same list.
- Git reads for audits and inventories no longer run repository hooks. Git
  runs `post-index-change` on any index write, including the refresh `diff`
  may do.
- CLI `query`, `map`, `repo-map` and `impact` warn on stderr when the index is
  not current. MCP tools use the same freshness rule.
- `mmcg index`, automatic refresh and the freshness check share one source
  inventory: Git inside a worktree, a walk outside it or when `git` is not on
  `PATH`. Before, `.ignore` files,
  parent `.gitignore` files and nested repositories made the manual index
  differ, so it never became current. `.ignore` files are no longer read.
- A directory outside Git can now be checked for freshness and refreshed by MCP
  tools. It used to report `index_stale` on every structural query.
- A source symlink or other non-regular source file is now named. `mmcg index`
  prints it as `failed source:`, `index_stale` lists it in
  `unindexable_sources`, and `run-task` names it when its refresh fails. Before,
  the index stayed stale with only a failure count.
- MCP tools return `index_rebuild_required` instead of `work_limit_exceeded`
  when a forced full rebuild does not fit the request budget.
- `serve` with an external `--index` returns `index_replaced` after the file
  is rebuilt, instead of a `snapshot_changed` that retrying never cleared.
- `verify-spec`, `audit-spec` and `run-task` read `## Pre-edit Snapshot`, the
  heading the spec templates write. The section was ignored before, so symbol,
  caller and signature checks did not run on it. Existing specs with a filled
  snapshot now get these checks, and an unfilled `<symbol>` line is an error.
- Git commands from Mastermind no longer run the `core.fsmonitor` program from
  a repository's config.
- `team map` keeps a lock valid after a member starts `mmcg serve`. Indexing
  leaves the path classification current, and an empty WAL no longer changes
  the index digest. Locks made while an empty WAL existed need a new `team lock`.
- Persona capture no longer fills its journal with tool receipts. Archiving now
  drops them for ended sessions, and `pipeline.retention` reports `database_bytes`.
- A failed journal write records one `capture_delivery_failed` gap and stops
  capture for that client and project until `miner hooks recover`. It used to
  leave a marker for every later event.
- The 1,024 capture marker limit now applies per client and project, so one
  failing project no longer stops capture everywhere.
- `mastermind uninstall --force` stops each client's worker, removes its hooks
  and capture grant and revokes its profile access before it removes
  `.mastermind/`. Prompts were still captured after uninstall before.
- `mastermind install` refuses to replace an edited Mastermind skill or agent
  and one it does not own. It used to replace them and delete the backup.

## [3.4.2] - 2026-10-26

### Added

- `mastermind init --exec-max-turns N`, `--exec-timeout N` and
  `--review-timeout N` store the native executor turns, executor timeout and
  reviewer timeout as project defaults in `.mastermind/setup.json`, like
  `--review-max-turns`. `run-task --exec`, `review-task run` and
  `run-task --auto-review` use them when the command line passes no flag; an
  explicit flag wins, and without a setting the limits stay 40 turns, 1800
  seconds and 600 seconds.

### Fixed

- `review-task submit` names the field and rule that reject a report, for
  example `criterion:<id>: semantic_review_evidence_unknown` or
  `invalid field or value at line L column C`, without quoting report text.
  `review-task prepare` lists the allowed statuses, reviewer kinds and history
  decisions. Accepted and rejected reports are unchanged.
- Refresh repository-owned Windows indexes through default CLI and MCP queries,
  including concept queries. External custom indexes remain read-only.
- Subscribe before the initial watcher scan, retain failed refresh work and retry
  without another file edit. Queue overflow and exhausted retries are reported.
- Reject unsupported native execution and review on Windows before changing task
  approval, source pins or iteration.
- Report optional Git profile refresh failures in background worker status and
  onboarding output while preserving otherwise successful project setup.

## [3.4.1] - 2026-10-22

### Fixed

- Declare file removals with `scope: delete`, including tests and re-export
  barrels without indexed declarations. Repeated preflight accepts proven
  deletions against the original task inputs. API removals and required docs
  remain checked before Held and semantic review.
- Retry task-mining finalization after missing transcripts, interrupted workers
  or failed Git refreshes without overlapping finalizers.
- Share query freshness checks across CLI and MCP, retain the original deadline
  and reject changed review source bindings.
- Clean up owned deep-capture process groups on Unix and report failed Claude
  registration removal instead of claiming successful uninstall.

## [3.4.0] - 2026-10-21

### Added

- Source-bound concept review through CLI and MCP, with operation witnesses,
  explicit request constraints and separate lexical candidates.
- Explicit `scope: file` with a reason for existing-file changes that do not
  use symbol snapshots. File admission and postflight checks remain required.

### Fixed

- Evaluate dependency-cycle risk for declared task files without materializing
  unrelated graph components. Preserve external cycle paths and reject incomplete
  searches instead of reporting zero risk.
- Keep CLI command construction within the Windows stack budget and report
  guard-runner preparation failures with specific reasons.
- Bind task verification kits and declared ignored inputs to observed check receipts.
- Preserve initial local work across task retries and audit only subsequent changes.
- Reapprove revised specs after implementation while retaining validated preconditions
  and checking new requirements.
- Support contained tracked symlinks on Unix and up to 100,000 observed files;
  reject unavailable snapshot inputs before executor handoff.
- Exclude plural test filenames from production maps and refresh old classification
  flags without replacing graph rows.

## [3.3.1] - 2026-10-15

### Added

- Opt-in graph ranking for repo-maps with `--ranking graph`. Lexical ranking
  remains the default; graph ranking is an experiment without a quality claim.
- Offline source-quote binding audits for judge and producer assessments against
  pinned snapshots, including raw and rendered Markdown framing diagnostics.

### Fixed

- Native review (`review-task run`, `run-task --auto-review`) is no longer lost
  when the model fences its JSON or adds a field: Claude Code validates the
  answer against the reviewer schema and the same model corrects it in the run.

### Changed

- The native reviewer needs Claude Code 2.1.290 or later in the 2.1 series and
  records review
  policy v2 (Read, Grep, Glob and StructuredOutput). Completed reviews recorded
  under policy v1 stay valid.
- The reviewer has 30 turns by default, configurable per project with
  `mastermind init --review-max-turns N`. A run that stops at its turn or
  schema-retry limit records `invocation_native_max_turns` or
  `invocation_native_structured_output_retries`.

## [3.3.0] - 2026-10-13

### Added

- Bounded source-range reads with source hashes and task-bound receipts for
  extending previously delivered ranges.
- Question-specific lexical repo-maps with bounded payloads and source/index
  bindings. Separate map-only and instruction-only modes support controlled
  comparisons. Claims still require source reads.
- Shared task and agent effort routing through CLI and MCP, with native client
  launch arguments and ordinary Codex spawn fields. The core agent-handoff
  skill applies routing before delegated research, execution and review.
- Seeded judge calibration, blind review exports and pre-registered
  qualification decisions with simulation-calibrated critical values.

### Changed

- Research and evaluation workflows retain source coverage, lookup usage,
  failed attempts and unknown measurements. Human reference labels remain
  distinct from model reviews.
- Missing effort context and duties outside the bounded research pilot select
  high. Explicit human effort choices take precedence; unsupported hosts retain
  their settings with an application limitation recorded.
- Update tree-sitter, TOML and pinned CI action dependencies.

### Fixed

- Keep repo-map CLI parsing within the Windows stack budget.
- Align native worker test waits with the worker's runtime budget.

## [3.2.4] - 2026-10-09

### Added

- Batch exact-name lookup in `mmcg_search`: query up to eight distinct names
  from one index snapshot, retaining each query's counts and truncation metadata.

### Changed

- Deliver reviewed feedback and habits before Git diagnostics. Admit whole
  claims without cutting their conditions or exceptions, report partial-list
  omissions and include reviewed habits in `doctor`'s profile-budget advice.
- Use Sonnet for bounded workflow research and Opus for cause investigation.
  Executor and auditor instructions preserve relevant preference conditions
  and check their application against the actual change.
- Organize deterministic eval harnesses by responsibility and use module
  commands. Benchmark accounting retains failed trials, unknown measurements,
  source coverage and stage-specific usage separately.

### Fixed

- Accept plain hyphenated terms in documentation, history, task and project
  context searches while preserving explicit FTS operators.
- Git pipe completion shares the operation's deadline and cancellation checks,
  avoiding false timeouts when pipe workers are scheduled after Git exits.
- Benchmark supervision keeps the child leader's PID reserved until process
  group cleanup and handles macOS zombie-only groups, avoiding signals to a
  recycled group and lost trial results.
- npm publication waits longer for accepted native archives to become public.
  Recovery dispatches from the immutable release tag while using the current
  publication helper from `main`.

## [3.2.3] - 2026-10-08

### Fixed

- Claude transcript collection no longer rejects a whole session because an
  assistant, attachment or tool-result record carries a different `cwd`
  (another directory, a worktree, a deleted folder). Only a cited human
  turn's `cwd` is checked, and it now counts inside the project tree, the
  project's Claude folder or the system temp directory, including after that
  directory is deleted.
- Resolve directory aliases for missing Claude memory and scratchpad paths,
  including macOS `/var` and `/private/var`. Skip recorded paths containing `..`
  even when the directory exists.
- `mastermind doctor` no longer reports a false "subagent MCP scoping"
  diagnostic for the `mmcg` registration `mastermind setup` itself writes,
  whose `env` is exactly `{"MMCG_PROFILE_CLIENT": <client id>}`.

## [3.2.2] - 2026-10-08

### Added

- `mastermind init --profile-budget <256-8000>` to set the project's default
  `mmcg_profile` delivery budget, saved in `.mastermind/setup.json` as
  `profile_budget_tokens` (default 4000) and omitted while it equals the
  default. Older binaries reject the key once it is written.
- `mastermind doctor` "profile budget" check: warns when the configured
  budget cannot deliver every eligible, source-verified rule of the
  whole-profile selection, with how many rules fit in ranked order, the
  tokens the full ranked set needs, the shortfall and an
  `init --profile-budget` hint.

### Changed

- Rank eligible feedback (active, not superseded) by scope — global, path,
  language, repo/project, role, workflow, newest stored `last_at` first
  within a tier — instead of alphabetical key order. The old fixed 12-item
  display cap is gone; the existing 64-rule source-verification limit now
  applies to eligible rules after ranking, instead of to whatever alphabetical
  key order selected first.
- Raise the default profile delivery budget and derive it from the project
  setting everywhere: `mmcg_profile`, the `UserPromptSubmit` hook (bounded by
  its 8 KiB context budget) and the `mmcg_context` / `run-task --exec` person
  layer (sized to the context budget left after the other layers).
- No shipped agent or skill instruction passes a fixed `budget_tokens` to
  `mmcg_profile` anymore; the project's configured budget applies.

### Fixed

- Deliver accepted preference rules again at the default budget: over
  budget, feedback is now trimmed one rule at a time from the least
  important end instead of the whole list being dropped, and `feedback_total`
  reports the eligible count before any cap. The old fixed 12-item cap on
  feedback (applied in alphabetical key order) no longer silently hides
  accepted rules beyond the 12th key.

## [3.2.1] - 2026-10-06

### Changed

- Require a result for each task-mining ticket, including empty results. Ask the
  current task agent for one continuation when a submission is missing.
- Distinguish missing reports, explicit no-signal results and legacy completions
  in hook diagnostics, and reserve prompt context for mining.

### Fixed

- Keep mining continuations in the original episode and exclude their generated
  prompts from personal evidence.
- Preserve completed source bindings across empty session-end receipts and late
  tool results.
- Bound large native tool payloads and retained traces without dropping valid capture.
- Report doctor failures and failed native runtime probes correctly.
- Fix concurrent capture-marker deletion on Windows, stale worker status during
  completion and unstable native-script tests on Linux.

## [3.2.0] - 2026-10-05

### Changed

- Use the current task agent for preference proposals on first native client
  setup. Capture runs locally and drafts require review. Separate background
  analysis remains available through `--mining on`.
- Configure profile delivery and task mining automatically on first `init`.
  Repeated setup preserves saved choices, including explicit opt-outs.
- Use the episode's native client and captured model for separate mining and
  prompt refinement. Delayed Claude transcripts can supply the actual response model.
- Update dependencies and consolidate the mining setup guide and reference.

### Fixed

- Preserve multiline conditions in local preference candidates, retry local
  analysis after a store outage, and retain the chosen profile client on completion.
- Archive closed episodes when the capture journal fills, and refresh candidates
  after session closure without manual replay.
- Keep mining active across tasks and native client updates while preserving
  explicit stops, retry budgets and completed checkpoints.
- Fix Windows CLI stack use and profile paths.
- Stop foreground mining cleanly when Ctrl-C interrupts processor hashing.

## [3.1.0] - 2026-09-28

### Fixed

- Align automatic refresh and status freshness checks for large repositories:
  100,000 candidates, 512 MiB of source, and a 30-second status scan.
- Handle binary files and unsupported file paths consistently across indexing,
  freshness checks, and Lens.
- Avoid a panic on Unicode whitespace in secret-like Markdown assignments.

## [3.0.1] - 2026-09-28

### Changed

- Give each Mastermind native hook an event-specific label in the client review
  screen. Re-running hook setup updates previous labels without duplicate hooks.

[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v5.0.0...HEAD
[5.0.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v5.0.0
[4.0.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v4.0.1
[4.0.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v4.0.0
[3.4.2]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.4.2
[3.4.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.4.1
[3.4.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.4.0
[3.3.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.3.1
[3.3.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.3.0
[3.2.4]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.4
[3.2.3]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.3
[3.2.2]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.2
[3.2.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.1
[3.2.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.0
[3.1.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.1.0
[3.0.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.0.1
