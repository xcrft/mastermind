"""The `mastermind doctor` check for commands that run fetched or generated code.

This is a line-by-line Python copy of `detect` in
`mcp/servers/mmcg/src/permission_lint/launch.rs`, so `just validate` can check
subagent hooks without building mmcg. Its word lists come from
`command_rules.json` beside the Rust code, which a Rust test keeps equal to the
Rust constants, and both copies must give the verdicts in
`fetch_exec_corpus.json`. Change all three together.
"""

from __future__ import annotations

import base64
import binascii
import json
import os
import tempfile
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath

RULES_DIR = Path(__file__).resolve().parents[1] / "mcp" / "servers" / "mmcg" / "src" / "permission_lint"
RULES = json.loads((RULES_DIR / "command_rules.json").read_text(encoding="utf-8"))
CORPUS_PATH = RULES_DIR / "fetch_exec_corpus.json"

SHELLS = set(RULES["shells"])
INTERPRETERS = set(RULES["interpreters"])
NETWORK_TOOLS = set(RULES["network_tools"])
EVALUATORS = set(RULES["evaluators"])
CODE_OPTIONS = set(RULES["code_options"])
ENCODED_OPTIONS = set(RULES["encoded_options"])
DECODE_CODE = RULES["decode_code"]
NETWORK_CODE = RULES["network_code"]
EVAL_CODE = RULES["eval_code"]
WRAPPERS = {entry["name"]: (set(entry["values"]), entry["skip"]) for entry in RULES["wrappers"]}

HOLE = "\u0001"
CLONED = "\u0002clone"
INSTALLED = "\u0002install"
EXTRACTED = "\u0002extract"
TEMPORARY_DIRECTORIES = tuple(RULES["temporary_directories"])
LOOKUPS = set(RULES["lookups"])
SHELL_VARIABLES = set(RULES["shell_variables"])
REMOTE_SHELLS = set(RULES["remote_shells"])


def _ascii_lower(text: str) -> str:
    return "".join(chr(ord(c) + 32) if "A" <= c <= "Z" else c for c in text)


def _normalise(word: str) -> str:
    base = _ascii_lower(word.replace("\\", "/").rsplit("/", 1)[-1])
    base = base.removesuffix(".exe")
    trimmed = base.rstrip("0123456789.-")
    return trimmed or base


def program_name(word: str) -> str:
    if word.startswith("$"):
        return _ascii_lower(word)
    name = _normalise(word)
    for suffix in (".cmd", ".bat", ".ps1"):
        if name.endswith(suffix):
            return name[: -len(suffix)]
    return name


def _ident(c: str) -> bool:
    return c.isascii() and (c.isalnum() or c == "_")


def is_named_shell(name: str) -> bool:
    return name in SHELLS or name in {"powershell", "pwsh"}


def variable_shell(word: str) -> bool:
    if not word.startswith("$"):
        return False
    rest = word[1:].rstrip("}")
    end = next((index for index, c in enumerate(rest) if not _ident(c)), len(rest))
    name, tail = rest[:end], rest[end:]
    if name in SHELL_VARIABLES:
        return True
    for prefix in (":-", ":=", "-", "="):
        if tail.startswith(prefix):
            return is_named_shell(program_name(tail[len(prefix) :]))
    return False


def is_shell(name: str) -> bool:
    return is_named_shell(name) or variable_shell(name)


def is_interpreter(name: str) -> bool:
    return name in INTERPRETERS or name == "python"


def is_option(word: str) -> bool:
    return word.startswith("-") and len(word) > 1


def is_assignment(word: str) -> bool:
    name, sep, _ = word.partition("=")
    return bool(sep) and bool(name) and all(_ident(c) for c in name)


def substitute_variables(text: str) -> str:
    values: list[tuple[str, str]] = []
    word = []
    words = []
    for c in text:
        if c.isspace() or c in ";&|()`":
            words.append("".join(word))
            word = []
        else:
            word.append(c)
    words.append("".join(word))
    for item in words:
        name, sep, value = item.partition("=")
        if not sep:
            continue
        valid = bool(name) and (name[0].isascii() and (name[0].isalpha() or name[0] == "_")) and all(_ident(c) for c in name)
        if valid and value and len(values) < 32:
            values.append((name, value))
    out = text
    for _ in range(2):
        for name, value in values:
            out = out.replace("${" + name + "}", value)
            pattern = "$" + name
            replaced = []
            rest = out
            while (index := rest.find(pattern)) >= 0:
                after = rest[index + len(pattern) :]
                replaced.append(rest[:index])
                replaced.append(pattern if after and _ident(after[0]) else value)
                rest = after
            replaced.append(rest)
            out = "".join(replaced)
    return out


def strip_comments(text: str) -> str:
    out: list[str] = []
    quote = None
    word_start = True
    index = 0
    while index < len(text):
        c = text[index]
        index += 1
        if quote is not None and c == quote:
            quote = None
            out.append(c)
        elif quote in (None, '"') and c == "\\":
            out.append(c)
            if index < len(text):
                out.append(text[index])
                index += 1
            word_start = False
            continue
        elif quote is not None:
            out.append(c)
        elif c in "'\"":
            quote = c
            out.append(c)
        elif c == "#" and word_start:
            end = text.find("\n", index)
            if end >= 0:
                out.append("\n")
                index = end + 1
            else:
                index = len(text)
            word_start = True
            continue
        else:
            out.append(c)
        word_start = quote is None and (c.isspace() or c in ";&|()")
    return "".join(out)


def deobfuscate_word(text: str) -> str:
    out: list[str] = []
    index = 0
    while index < len(text):
        c = text[index]
        index += 1
        if c == "\\":
            if index < len(text) and text[index] == "\n":
                index += 1
        elif c in "'\"":
            pass
        elif c in "\n\r":
            out.append(" " if "".join(out).rstrip().endswith(("|", "&")) else ";")
        else:
            out.append(c)
    joined = "".join(out).replace("${IFS}", " ").replace("$IFS", " ")
    return _ascii_lower(" ".join(substitute_variables(joined).split()))


def deobfuscate(text: str) -> str:
    return deobfuscate_word(strip_comments(text))


def split_script(outer: str) -> list[list[list[str]]]:
    text = outer.replace("${", "$")
    for old, new in ((">&", " > "), ("&>", " > "), ("<&", " < "), ("|&", "|"), ("&&", ";"), ("||", ";"), ("&", ";"), (">", " > "), ("<", " < ")):
        text = text.replace(old, new)
    script = []
    for command in text.split(";"):
        stages = []
        for stage in command.split("|"):
            words, word = [], []
            for c in stage:
                if c.isspace() or c in "(){}":
                    if word:
                        words.append("".join(word))
                    word = []
                else:
                    word.append(c)
            if word:
                words.append("".join(word))
            if words:
                stages.append(words)
        if stages:
            script.append(stages)
    return script


@dataclass
class Call:
    word: str
    name: str
    args: list[str]
    via_xargs: bool = False
    assignments: list[str] = field(default_factory=list)

    def positional(self) -> list[str]:
        return [arg for arg in self.args if not is_option(arg)]

    def code_index(self) -> int | None:
        return next((index for index, arg in enumerate(self.args) if arg in CODE_OPTIONS), None)

    def shell_code(self) -> list[str] | None:
        if not is_shell(self.name):
            return None
        index = self.code_index()
        if index is None:
            return None
        code = self.args[index + 1 :]
        return code or None


def resolve(words: list[str]) -> Call | None:
    assignments: list[str] = []
    via_xargs = False
    index = 0
    for _ in range(8):
        while index < len(words) and is_assignment(words[index]):
            assignments.append(words[index])
            index += 1
        if index >= len(words):
            return None
        word = words[index]
        name = program_name(word)
        if name in WRAPPERS:
            values, skip = WRAPPERS[name]
            via_xargs |= name == "xargs"
            index += 1
            skipped = 0
            while index < len(words):
                arg = words[index]
                if is_assignment(arg):
                    assignments.append(arg)
                    index += 1
                elif is_option(arg):
                    index += 2 if arg in values else 1
                elif skipped < skip:
                    skipped += 1
                    index += 1
                else:
                    break
            continue
        call = Call(word, name, words[index + 1 :], via_xargs, list(assignments))
        code = call.shell_code()
        if code:
            script = split_script(" ".join(code))
            hole = any(HOLE in item for item in code)
            if not hole and len(script) == 1 and len(script[0]) == 1:
                inner = resolve(script[0][0])
                if inner:
                    inner.via_xargs |= via_xargs
                    inner.assignments.extend(assignments)
                    return inner
        return call
    return None


def looked_up_program(inner: str) -> str | None:
    script = split_script(inner)
    if len(script) != 1 or len(script[0]) != 1:
        return None
    call = resolve(script[0][0])
    if call is None:
        return None

    def named(word: str) -> str | None:
        name = program_name(word)
        return name if is_named_shell(name) or is_interpreter(name) else None

    if call.name in LOOKUPS:
        return next((found for arg in call.positional() if (found := named(arg))), None)
    if not call.args and not call.assignments:
        return named(call.word)
    return None


def extract_substitutions(text: str) -> tuple[str, list[str]]:
    outer: list[str] = []
    inner: list[str] = []
    index = 0
    while index < len(text):
        c = text[index]
        if c in "$<>" and text[index + 1 : index + 2] == "(":
            depth = 1
            end = index + 2
            while end < len(text) and depth > 0:
                if text[end] == "(":
                    depth += 1
                elif text[end] == ")":
                    depth -= 1
                end += 1
            stop = end - 1 if depth == 0 else end
            body = text[index + 2 : stop]
            outer.append(_substitution_word(c, body))
            inner.append(body)
            index = end
        elif c == "`":
            found = text.find("`", index + 1)
            end = found if found >= 0 else len(text)
            body = text[index + 1 : min(end, len(text))]
            outer.append(_substitution_word("$", body))
            inner.append(body)
            index = end + 1
        else:
            outer.append(c)
            index += 1
    return "".join(outer), inner


def _substitution_word(open_char: str, body: str) -> str:
    program = looked_up_program(body)
    if program and open_char == "$":
        return f" {program} "
    return f" {HOLE} "


def produces_code(call: Call) -> bool:
    args = call.args
    return (
        call.name in NETWORK_TOOLS
        or (call.name == "openssl" and "s_client" in args)
        or (call.name == "openssl" and ("base64" in args or "enc" in args) and "-d" in args)
        or (call.word.rsplit("/", 1)[-1] == "base64" and any(arg in {"-d", "-D", "--decode"} for arg in args))
        or (call.name == "xxd" and "-r" in args)
        or (call.name == "certutil" and "-decode" in args)
        or any("/dev/tcp/" in arg for arg in args)
    )


def is_stdin(arg: str) -> bool:
    if arg == "-":
        return True
    if not arg.startswith("/"):
        return False
    parts: list[str] = []
    for part in arg.split("/"):
        if part == "..":
            if parts:
                parts.pop()
        elif part and part != ".":
            parts.append(part)
    zero = lambda fd: bool(fd) and set(fd) == {"0"}  # noqa: E731
    if parts == ["dev", "stdin"]:
        return True
    if len(parts) == 3 and parts[:2] == ["dev", "fd"]:
        return zero(parts[2])
    if len(parts) == 4 and parts[0] == "proc" and parts[2] == "fd":
        return zero(parts[3])
    return False


def runs_stdin(call: Call) -> bool:
    name = call.name
    has = lambda options: any(arg in options for arg in call.args)  # noqa: E731

    def runs_shell() -> bool:
        return any(is_named_shell(program_name(arg)) or is_interpreter(program_name(arg)) for arg in call.args)

    positional = call.positional()
    if name in {"at", "batch"}:
        return not has({"-f", "-l", "-r", "-d", "-c"})
    if name == "crontab":
        return not has({"-l", "-r", "-e"}) and (not positional or is_stdin(positional[0]))
    if name in {"docker", "podman", "nerdctl", "kubectl"}:
        sub = positional[0] if positional else None
        interactive = any(
            arg in {"--interactive", "--interactive=true", "--stdin"}
            or (arg.startswith("-") and not arg.startswith("--") and all(c.isascii() and c.isalpha() for c in arg[1:]) and "i" in arg)
            for arg in call.args
        )
        return sub in {"run", "exec", "container"} and interactive and runs_shell()
    if name == "script" or name in REMOTE_SHELLS:
        return runs_shell()
    if name in EVALUATORS:
        return name in {"eval", "iex", "invoke-expression"} or not positional or is_stdin(positional[0])
    if not is_shell(name) and not is_interpreter(name):
        return False
    if call.via_xargs:
        return True
    if "-s" in call.args and is_shell(name):
        return True
    index = call.code_index()
    if index is not None:
        return index + 1 < len(call.args) and is_stdin(call.args[index + 1])
    if "-m" in call.args:
        return False
    return not positional or is_stdin(positional[0])


def _starts_with(path: PurePosixPath, prefix: PurePosixPath) -> bool:
    return path.parts[: len(prefix.parts)] == prefix.parts


def in_temporary_directory(path: str) -> bool:
    candidate = PurePosixPath(path)
    prefixes = [PurePosixPath(prefix) for prefix in TEMPORARY_DIRECTORIES]
    prefixes.append(PurePosixPath(os.path.realpath(tempfile.gettempdir())))
    return any(_starts_with(candidate, prefix) for prefix in prefixes)


def fixed_path(call: Call) -> bool:
    word = call.word
    if "/" not in word and "\\" not in word:
        return True
    parts = word.replace("\\", "/").split("/")
    if not word.startswith("/") or "." in parts or ".." in parts or in_temporary_directory(word):
        return False
    return not (os.path.exists(word) and in_temporary_directory(os.path.realpath(word)))


def cats_checkout_files(call: Call) -> bool:
    return (
        call.name == "cat"
        and fixed_path(call)
        and not call.assignments
        and bool(call.args)
        and all(
            not file.startswith(("/", "~", "$", "-", "\\")) and HOLE not in file and ":" not in file and ".." not in file.split("/")
            for file in call.args
        )
    )


def runs_hole_text(call: Call) -> bool:
    name = call.name
    if name in {"eval", "iex", "invoke-expression"}:
        return any(HOLE in arg for arg in call.args)
    if not is_shell(name) and not is_interpreter(name) and name not in EVALUATORS:
        return False
    args = call.args
    here_string = any(args[i : i + 3] == ["<", "<", "<"] and HOLE in args[i + 3] for i in range(len(args) - 3))
    index = call.code_index()
    return here_string or (index is not None and index + 1 < len(args) and HOLE in args[index + 1])


def runs_hole(call: Call) -> bool:
    name = call.name
    if name in EVALUATORS:
        return any(HOLE in arg for arg in call.args)
    if not is_shell(name) and not is_interpreter(name):
        return False
    index = call.code_index()
    if index is not None:
        return index + 1 < len(call.args) and HOLE in call.args[index + 1]
    positional = call.positional()
    first = positional[0] if positional else None
    redirected = any(call.args[i] == "<" and HOLE in call.args[i + 1] for i in range(len(call.args) - 1))
    return (first is not None and HOLE in first) or (redirected and first == "<")


def _base(word: str) -> str:
    return word.rsplit("/", 1)[-1]


def downloaded_files(call: Call) -> list[str]:
    files: list[str] = []

    def url_name() -> str | None:
        url = next((arg for arg in call.args if "://" in arg), None)
        if url is None:
            return None
        url = url.replace("#", "?").split("?", 1)[0]
        name = url.rsplit("/", 1)[-1]
        return name or None

    explicit = False
    for index, arg in enumerate(call.args):
        following = call.args[index + 1] if index + 1 < len(call.args) else None
        target = None
        if arg == ">":
            target = following
        elif call.name in {"curl", "wget"} and arg in {"--output", "--output-document", "--remote-name"}:
            target = following if following is not None else url_name()
        elif call.name in {"curl", "wget"} and arg.startswith("-") and not arg.startswith("--") and arg.endswith("o"):
            target = following
        elif arg.startswith("--output-document="):
            target = arg[len("--output-document=") :]
        elif arg.startswith("--output="):
            target = arg[len("--output=") :]
        if target is not None:
            explicit = True
            if target not in {"-", "/dev/null"}:
                files.append(target)
    if not explicit and call.name == "wget":
        name = url_name()
        if name:
            files.append(name)
    return [name for name in (_base(file) for file in files) if name and name != HOLE]


def written_files(call: Call) -> list[str]:
    files = downloaded_files(call) if call.name in NETWORK_TOOLS else []
    positional = call.positional()

    def push(file: str) -> None:
        file = file.replace(":", "/").rsplit("/", 1)[-1]
        if file and file not in {HOLE, "-", "."}:
            files.append(file)

    builds_local = call.name == "go" and bool(positional) and positional[0] in {"build", "test"}
    for index, arg in enumerate(call.args):
        following = call.args[index + 1] if index + 1 < len(call.args) else None
        if builds_local and arg != ">":
            continue
        if arg in {">", "-o", "--output", "--out"}:
            if following is not None:
                push(following)
        elif arg.startswith("--output="):
            push(arg[len("--output=") :])

    def package(spec: str) -> str:
        spec = spec.rsplit("/", 1)[-1]
        for sep in "@=<>[~":
            spec = spec.split(sep, 1)[0]
        return spec

    name = call.name
    sub = positional[0] if positional else None
    if name == "tee" or name in {"scp", "rsync", "sftp"}:
        for file in positional:
            push(file)
    elif name == "git" and sub == "clone":
        rest = positional[1:]
        if len(rest) >= 2:
            push(rest[1])
        elif len(rest) == 1:
            url = rest[0].rstrip("/")
            while url.endswith(".git"):
                url = url[: -len(".git")]
            push(url)
        files.append(CLONED)
    elif name == "brew" and sub == "install":
        files.extend(package(spec) for spec in positional[1:])
    else:
        specs = installed_specs(call)
        if any(not pinned(spec) for spec in specs):
            files.extend(package(spec) for spec in specs)
            files.append(INSTALLED)
    return files


PACKAGE_INSTALLERS = {
    "bun": {"add", "i", "install"},
    "cargo": {"binstall", "install"},
    "gem": {"install"},
    "go": {"install"},
    "npm": {"add", "i", "install"},
    "pip": {"install"},
    "pipx": {"install"},
    "pnpm": {"add", "i", "install"},
    "uv": {"tool"},
    "yarn": {"add", "global"},
}

INSTALL_VALUE_OPTIONS = {
    "-c", "-e", "-i", "-r", "-t", "--bin", "--branch", "--constraint", "--editable",
    "--extra-index-url", "--features", "--index", "--index-url", "--manifest-path",
    "--path", "--prefix", "--profile", "--python", "--registry", "--requirement",
    "--rev", "--root", "--tag", "--target", "--target-dir", "--vers", "--version",
}


def installed_specs(call: Call) -> list[str]:
    positional = call.positional()
    if not positional or positional[0] not in PACKAGE_INSTALLERS.get(call.name, set()):
        return []
    second = positional[1] if len(positional) > 1 else None
    if (call.name, second) in {("uv", "install"), ("yarn", "add")}:
        skip = 2
    elif call.name == "uv":
        return []
    else:
        skip = 1
    version = next(
        (call.args[i + 1] for i in range(len(call.args) - 1) if call.args[i] in {"--version", "--vers"}),
        None,
    )
    specs: list[str] = []
    words = 0
    index = 0
    while index < len(call.args):
        arg = call.args[index]
        index += 1
        if is_option(arg):
            if arg in INSTALL_VALUE_OPTIONS:
                index += 1
            continue
        words += 1
        if words <= skip:
            continue
        if arg.startswith((".", "/", "~", "$")) or HOLE in arg or arg in {"tool", "add", "install", "global"}:
            continue
        specs.append(f"{arg}@{version}" if version is not None and "@" not in arg and "=" not in arg else arg)
    return specs


def pinned(spec: str) -> bool:
    for prefix in ("npm:", "jsr:"):
        if spec.startswith(prefix):
            spec = spec[len(prefix) :]
            break
    if "==" in spec:
        version = spec.split("==", 1)[1]
    else:
        at = spec.rfind("@")
        if at <= 0:
            return False
        version = spec[at + 1 :]
    if version.startswith("v"):
        version = version[1:]
    core = version.replace("+", "-").split("-", 1)[0]
    parts = core.split(".")
    return len(parts) >= 2 and all(part and all(c in "0123456789" for c in part) for part in parts)


def extracted_archive(call: Call) -> str | None:
    def cluster(arg: str) -> bool:
        letters = arg[1:] if arg.startswith("-") else arg
        return not arg.startswith("--") and bool(letters) and all(c.isascii() and c.isalpha() for c in letters)

    args = call.args
    if call.name in {"tar", "bsdtar", "gtar"}:
        extracts = any(
            arg in {"--extract", "--get"} or (cluster(arg) and "x" in arg and (index == 0 or arg.startswith("-")))
            for index, arg in enumerate(args)
        )
        if not extracts:
            return None
        for index, arg in enumerate(args):
            if arg == "--file" or (cluster(arg) and "f" in arg and (index == 0 or arg.startswith("-"))):
                if index + 1 < len(args):
                    return args[index + 1]
                break
        return "-"
    positional = call.positional()
    if call.name == "unzip":
        return positional[0] if positional else None
    if call.name in {"7z", "7za"} and positional and positional[0] == "x":
        return positional[1] if len(positional) > 1 else None
    return None


def builds_project(call: Call) -> bool:
    positional = call.positional()
    sub = positional[0] if positional else None
    name = call.name
    return (
        name in {"make", "cmake", "ninja", "just", "meson", "configure", "bazel", "gradle", "gradlew", "mvn"}
        or (name == "cargo" and sub in {"build", "run", "test", "install"})
        or (name == "go" and sub in {"build", "run", "test", "generate", "install"})
        or (name in {"npm", "pnpm", "yarn", "bun"} and sub in {"i", "install", "ci", "run", "test", "start"})
        or (name in {"pip", "pip3", "uv"} and sub in {"install", "sync", "run"})
        or name in {"python", "python3"}
        or call.word.startswith("./")
    )


def xargs_runs_code(stage: list[str]) -> bool:
    index = next((i for i, word in enumerate(stage) if program_name(word) == "xargs"), None)
    if index is None:
        return False
    return any(
        is_shell(program_name(word)) or is_interpreter(program_name(word)) or program_name(word) in EVALUATORS
        for word in stage[index + 1 :]
    )


def runs_commands(call: Call, text: str) -> bool:
    if call.name == "find":
        return any(
            call.args[i] in {"-exec", "-execdir", "-ok", "-okdir"}
            and (
                is_shell(program_name(call.args[i + 1]))
                or is_interpreter(program_name(call.args[i + 1]))
                or program_name(call.args[i + 1]) in EVALUATORS
            )
            for i in range(len(call.args) - 1)
        )
    if call.name in {"awk", "gawk", "mawk", "nawk"}:
        compact = "".join(c for c in text if not c.isspace())
        return "system(" in compact or "|getline" in compact or '|"' in compact
    return False


def is_remote_spec(spec: str) -> bool:
    return "://" in spec or spec.startswith(("git+", "github:", "gitlab:", "bitbucket:", "git:", "gist:"))


def remote_code(call: Call) -> bool:
    positional = call.positional()
    name = call.name

    def value_of(option: str) -> str | None:
        if option in call.args:
            index = call.args.index(option)
            if index + 1 < len(call.args):
                return call.args[index + 1]
        return None

    if name in {"npx", "bunx", "pnpx", "uvx"}:
        specs = positional[:1]
        extra = value_of("-p") or value_of("--package") or value_of("--from")
        if extra is not None:
            specs.append(extra)
        runs_or_installs = True
    else:
        if name in {"npm", "pnpm", "yarn", "bun", "pip", "pipx", "uv", "deno", "gem", "cargo", "go"}:
            specs = positional[1:]
        elif is_interpreter(name) and call.args[:1] == ["-m"]:
            specs = positional[1:]
        else:
            specs = []
        runs_or_installs = bool(positional) and positional[0] in {"add", "dlx", "exec", "i", "install", "run", "x", "tool", "pip"}
    return runs_or_installs and any(is_remote_spec(spec) for spec in specs)


def runs_file(call: Call, files: list[str]) -> bool:
    if _base(call.word) in files:
        return True
    if CLONED in files and builds_project(call):
        return True
    if EXTRACTED in files:
        relative = lambda word: not word.startswith(("/", "~", "$")) and HOLE not in word  # noqa: E731
        positional = call.positional()
        script = bool(positional) and relative(positional[0])
        if ("/" in call.word and relative(call.word)) or builds_project(call) or ((is_shell(call.name) or is_interpreter(call.name)) and script):
            return True
    if call.name == "crontab" and any(_base(arg) in files for arg in call.positional()):
        return True
    if not (is_shell(call.name) or is_interpreter(call.name) or call.name in EVALUATORS):
        return False
    positional = call.positional()
    return (bool(positional) and _base(positional[0]) in files) or any(
        call.args[i] == "<" and _base(call.args[i + 1]) in files for i in range(len(call.args) - 1)
    )


def _decode_powershell(text: str) -> str | None:
    try:
        data = base64.b64decode(text.strip(), validate=True)
    except (binascii.Error, ValueError):
        return None
    try:
        if not data or len(data) % 2:
            return data.decode("utf-8")
        return data.decode("utf-16-le")
    except UnicodeDecodeError:
        return None


def encoded_commands(text: str) -> list[str | None]:
    words = [word for word in "".join(" " if c.isspace() or c in "'\"" else c for c in text).split(" ") if word]
    found: list[str | None] = []
    in_powershell = False
    for index, word in enumerate(words):
        if program_name(word) in {"pwsh", "powershell"}:
            in_powershell = True
            continue
        if in_powershell and _ascii_lower(word) in ENCODED_OPTIONS:
            found.append(_decode_powershell(words[index + 1]) if index + 1 < len(words) else None)
    return found


def detect(text: str, depth: int = 0, shell: bool = True) -> bool:
    if depth > 3:
        return False
    if any(decoded is not None and detect(decoded, depth + 1, True) for decoded in encoded_commands(text)):
        return True
    norm = deobfuscate(text) if shell else deobfuscate_word(text)
    outer, holes = extract_substitutions(norm)
    holes_fetch = any(
        produces_code(call)
        for hole in holes
        for command in split_script(hole)
        for stage in command
        if (call := resolve(stage))
    )
    if any(detect(hole, depth + 1, True) for hole in holes):
        return True
    words = norm.split(" ")
    inline_code = any(
        is_interpreter(program_name(word)) and any(option in CODE_OPTIONS for option in words[index + 1 : index + 5])
        for index, word in enumerate(words)
    )
    if (
        inline_code
        and (any(word in norm for word in NETWORK_CODE) or any(word in norm for word in DECODE_CODE))
        and any(word in norm for word in EVAL_CODE)
    ):
        return True
    written: list[str] = []
    for command in split_script(outer):
        if INSTALLED in written:
            return True
        if any(xargs_runs_code(stage) for stage in command[1:]):
            return True
        calls = [call for call in (resolve(stage) for stage in command) if call]
        for index, call in enumerate(calls):
            if written and runs_file(call, written):
                return True
            from_checkout = index == 1 and cats_checkout_files(calls[0])
            if index > 0 and runs_stdin(call) and not from_checkout:
                return True
            if produces_code(call) and any(runs_stdin(later) for later in calls[index + 1 :]):
                return True
            if runs_hole_text(call) or (holes_fetch and runs_hole(call)):
                return True
            if runs_commands(call, norm):
                return True
            if call.name in {"iex", "invoke-expression"} and any(arg in NETWORK_TOOLS for arg in call.args):
                return True
            if call.name in NETWORK_TOOLS and any(
                call.args[i] in {"-e", "-c", "--exec", "--sh-exec"} and is_shell(program_name(call.args[i + 1]))
                for i in range(len(call.args) - 1)
            ):
                return True
            if is_shell(call.name) and any("/dev/tcp/" in arg for arg in call.args):
                return True
            if remote_code(call):
                return True
            archive = extracted_archive(call)
            if archive is not None:
                if (index > 0 and is_stdin(archive)) or _base(archive) in written:
                    written.append(EXTRACTED)
            written.extend(written_files(call))
    return False


def fetch_and_exec(text: str) -> bool:
    """Whether `text` downloads, writes or decodes code and runs it, as doctor reads it."""
    return detect(text, 0, True)
