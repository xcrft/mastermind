"""Deterministic checks for the skill-selection corpus and catalogue.

Hosts pick a skill or subagent from its name and description alone. These
checks never ask which choice a prompt should get, so they cannot show that a
model routes well. They check the case list and the catalogue: every label
names a shipped choice, words in the prompt do not contradict the label, each
conflict case cites the description clause that settles it in one direction,
known neighbours each have a conflict case, prompts do not copy the expected
description, and the catalogue matches the descriptions. The model-backed
`selection` suite in evals/behavior is the only thing that measures picks.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from itertools import combinations
import math
from pathlib import Path
import re

from evals.behavior import runner
from scripts import validate

REPO_ROOT = validate.REPO_ROOT
CATALOGUE = runner.SUITES["selection"]["catalogue"]
CASES = runner.SUITES["selection"]["cases"]
INSTALLER = REPO_ROOT / "npm/mastermind/bin/install.js"
SUBAGENTS_DIR = REPO_ROOT / "agents/subagents"

SKILL_ANSWER = "Skill: "
AGENT_ANSWER = "Agent: "
ANSWER_PREFIX = SKILL_ANSWER
AGENT_KEY = "agent:"
NONE = "none"
CONFLICT_PREFIX = "sel-conflict-"
PARAPHRASE_SUFFIX = "-paraphrase"
CORE_MIN_POSITIVE = 2
CORE_MIN_NEGATIVE = 2
OTHER_MIN_POSITIVE = 1
OTHER_MIN_NEGATIVE = 1

# A subagent that runs a skill's job. Claude Code may delegate to it for the
# same prompt, so a case that expects the skill also accepts the subagent and
# the pair counts as one choice, not a tie.
COMPANIONS = {
    "mastermind-comment-auditor": "mastermind-comment-audit",
    "mastermind-critic": "mastermind-critical-review",
    "mastermind-frontend-auditor": "mastermind-frontend-audit",
    "mastermind-investigator": "mastermind-investigation-ledger",
    "mastermind-prompt-refiner": "mastermind-prompt-refiner",
    "mastermind-security-auditor": "mastermind-agent-security-review",
    "mastermind-task-executor": "mastermind-task-executor",
    "mastermind-test-auditor": "mastermind-test-audit",
}

# Pairs a person can confuse even when their triggers share no words. Lexical
# overlap only hints at a conflict, so this list is maintained by hand, and
# each pair needs a `sel-conflict-` case that expects one and rejects the other.
ADJACENT_PAIRS = frozenset(
    frozenset(("mastermind-" + left if left != "no-ai-slop-comments" else left,
               "mastermind-" + right if right != "no-ai-slop-comments" else right))
    for left, right in (
        ("investigation-ledger", "verify-change"),
        ("critical-review", "diff-review"),
        ("codegraph-research", "project-history"),
        ("project-map", "codegraph-research"),
        ("task-planning", "task-executor"),
        ("architecture-review", "critical-review"),
        ("dependency-upgrade", "scanner-alert-response"),
        ("diff-review", "agent-security-review"),
        ("comment-audit", "no-ai-slop-comments"),
        ("test-audit", "test-impact"),
        ("test-impact", "verify-change"),
        ("browser-verification", "frontend-audit"),
        ("diff-review", "frontend-audit"),
        ("agent-security-review", "security-research"),
        ("design-intake", "product-intake"),
        ("architecture-review", "runtime-research"),
        ("task-executor", "structured-report-contract"),
    )
)

# Shared trigger wording is a coarse sign that two skills compete for the same
# prompt. The threshold was set by reading the pairs: at 0.2 every pair flagged
# is one a person could plausibly confuse. It finds pairs that need a prompt. It
# says nothing about how a model routes, and a link between the descriptions no
# longer excuses a pair.
OVERLAP_THRESHOLD = 0.2
_STOPWORDS = frozenset(
    "a an and any are as asked at be before by for from how in into is it its "
    "not of on one or that the their there this to use used when whether which "
    "while who with without what your after".split()
)
# Words that carry no topic, dropped from the lexical gate on top of the above.
_LEXICAL_STOPWORDS = _STOPWORDS | frozenset(
    "i me my we our us you can do does did done this these those have has had was were will would "
    "should could all some just so if then than also only very more most get give tell make let "
    "need want where why about over under again still yet no nothing new old here now it's i'm "
    "out up down off them they he she his her every find show add goes another something between build "
    "look ask make".split()
)
# Real requests carry at least one function word. "asdf qwer" does not.
_FUNCTION_WORDS = _STOPWORDS | frozenset("i my we our me you is are was were do does did can should".split())
MIN_PROMPT_WORDS = 5

# Lexical plausibility gate. The expected choice must score above every
# rejected one on shared description words, weighted by rarity. A case whose
# label rests on meaning the words do not show carries a reviewed
# `lexical_exception` instead. A `none` case fails when any choice scores at or
# above NONE_MAX_SCORE: positive cases that pass the gate score 4 to 17, and
# requests that fit nothing score 0 to 3.5.
NONE_MAX_SCORE = 4.0

# Corpus leak metric. A rare word appears in at most RARE_MAX_DF descriptions.
# A prompt that shares two or more rare words with the expected description and
# none of the rejected ones can be answered by word matching alone. Some of
# those cases are fine, because people do say "Dependabot" or "Figma". The
# review that added this check counted 32 of 75 positive cases (43%) like that.
# The limit is one third, in the whole corpus and in the held-out split, so
# most cases need more than word matching and the corpus cannot drift back.
RARE_MAX_DF = 2
LEAK_MIN_RARE_WORDS = 2
LEAK_MAX_SHARE = 1 / 3
LEAK_MAX_SHARE_HELD_OUT = 1 / 3

HELD_OUT_MIN_SHARE = 0.30
HELD_OUT_MAX_SHARE = 0.50

CATALOGUE_HEADER = """\
# Skill selection probe

Generated by `python -m evals.selection --write` from every SKILL.md and
subagent. Do not edit by hand.

You are choosing what to load for the user's message. A host gives you only
these names and descriptions. Do not carry out the request and do not ask a
question. Reply with exactly one line: `Skill: <name>` for the one skill whose
description fits the message best, `Agent: <name>` when a subagent fits better
than any skill, or `Skill: none` when nothing fits.
"""


@dataclass(frozen=True)
class Skill:
    """A skill or subagent as a host sees it: a name and a description."""

    name: str
    description: str
    path: Path
    kind: str = "skill"

    @property
    def key(self) -> str:
        return self.name if self.kind == "skill" else AGENT_KEY + self.name

    @property
    def answer(self) -> str:
        return (SKILL_ANSWER if self.kind == "skill" else AGENT_ANSWER) + self.name

    @property
    def job(self) -> str:
        """The skill whose work this choice does. A subagent without a companion is its own job."""
        return self.name if self.kind == "skill" else COMPANIONS.get(self.name, self.key)

    @property
    def trigger(self) -> str:
        return self.description.split("Not for", 1)[0]

    @property
    def referrals(self) -> tuple[str, ...]:
        return tuple(validate.description_referrals(self.description))

    @property
    def boundaries(self) -> list[validate.Referral]:
        return validate.description_referral_clauses(self.description)

    @property
    def own_text(self) -> str:
        """What the choice claims for itself: its name and description without the `Not for` hand-offs."""
        text = self.description
        boundary = validate.SKILL_BOUNDARY_RE.search(text)
        if boundary is not None:
            rest = text[boundary.start():]
            clauses = self.boundaries
            end = max([r.end + 1 - boundary.start() for r in clauses if r.start >= boundary.start()], default=0)
            stop = re.search(r"\.(?:\s|$)", rest[end:])
            cut = boundary.start() + end + (stop.end() if stop else len(rest) - end)
            text = text[: boundary.start()] + " " + text[cut:]
        short = self.name.removeprefix("mastermind-").replace("-", " ")
        return f"{short} {text}"


@dataclass(frozen=True)
class Case:
    id: str
    prompt: str
    accept: frozenset[str]
    reject: frozenset[str]
    lexical_exception: str | None = None
    tie_break: str | None = None

    @property
    def conflict(self) -> bool:
        return self.id.startswith(CONFLICT_PREFIX)

    @property
    def tie(self) -> bool:
        return len(jobs(self.accept)) > 1


@dataclass
class Report:
    problems: list[str] = field(default_factory=list)
    ties: list[Case] = field(default_factory=list)
    overlaps: list[tuple[float, str, str, str]] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    leaks: list[tuple[str, tuple[str, ...]]] = field(default_factory=list)
    splits: dict[str, int] = field(default_factory=dict)


def job_of(key: str) -> str:
    if key.startswith(AGENT_KEY):
        name = key[len(AGENT_KEY):]
        return COMPANIONS.get(name, key)
    return key


def jobs(keys: frozenset[str]) -> set[str]:
    return {job_of(key) for key in keys}


def load_skills() -> dict[str, Skill]:
    skills: dict[str, Skill] = {}
    for artifact in validate.find_artifacts():
        if artifact.path.name != "SKILL.md" or artifact.name is None:
            continue
        description = artifact.frontmatter.get("description")
        if isinstance(description, str):
            skills[artifact.name] = Skill(artifact.name, description, artifact.path)
    return dict(sorted(skills.items()))


def load_agents() -> dict[str, Skill]:
    agents: dict[str, Skill] = {}
    for artifact in validate.find_artifacts():
        if artifact.path.parent != SUBAGENTS_DIR or artifact.name is None:
            continue
        description = artifact.frontmatter.get("description")
        if isinstance(description, str):
            agent = Skill(artifact.name, description, artifact.path, kind="agent")
            agents[agent.key] = agent
    return dict(sorted(agents.items()))


def load_choices() -> dict[str, Skill]:
    return {**load_skills(), **load_agents()}


def core_skills(installer: Path = INSTALLER) -> frozenset[str]:
    text = installer.read_text(encoding="utf-8")
    match = re.search(r"const CORE_SKILLS = \[(.*?)\];", text, re.S)
    if match is None:
        raise ValueError(f"CORE_SKILLS not found in {installer}")
    return frozenset(re.findall(r'"([a-z0-9-]+)"', match.group(1)))


def render_catalogue(choices: dict[str, Skill]) -> str:
    skills = [c for c in choices.values() if c.kind == "skill"]
    agents = [c for c in choices.values() if c.kind == "agent"]
    lines = [CATALOGUE_HEADER, "## Skills", ""]
    lines += [f"- `{c.name}`: {c.description}" for c in skills]
    if agents:
        lines += ["", "## Subagents", ""]
        lines += [f"- `{c.name}`: {c.description}" for c in agents]
    return "\n".join(lines) + "\n"


def _answer_keys(phrases: list[str], label: str, case_id: str, problems: list[str]) -> set[str]:
    keys: set[str] = set()
    for phrase in phrases:
        if phrase.startswith(SKILL_ANSWER):
            keys.add(phrase[len(SKILL_ANSWER):])
        elif phrase.startswith(AGENT_ANSWER):
            keys.add(AGENT_KEY + phrase[len(AGENT_ANSWER):])
        else:
            problems.append(f"{case_id}: {label} phrase {phrase!r} must start with {SKILL_ANSWER!r} or {AGENT_ANSWER!r}")
    return keys


def load_cases(path: Path = CASES, problems: list[str] | None = None) -> list[Case]:
    problems = [] if problems is None else problems
    cases: list[Case] = []
    for record in runner.load_case_records(path, suite_name="selection"):
        case_id = record["id"]
        expect = record["expect"]
        groups = expect.get("contains_any", [])
        if ("contains" in expect) == bool(groups) or len(expect.get("contains", [None])) != 1 or len(groups) > 1:
            problems.append(f"{case_id}: name the expected choice with one `contains` phrase or one `contains_any` group")
            continue
        accept = _answer_keys(expect.get("contains", []) or groups[0], "expected", case_id, problems)
        reject = _answer_keys(expect.get("not_contains", []), "rejected", case_id, problems)
        cases.append(
            Case(
                case_id,
                record["input"]["prompt"],
                frozenset(accept),
                frozenset(reject),
                record.get("lexical_exception"),
                record.get("tie_break"),
            )
        )
    return cases


def _stem(word: str) -> str:
    if len(word) > 4 and word.endswith("ies"):
        word = word[:-3] + "y"
    elif len(word) > 3 and word.endswith("s") and not word.endswith("ss"):
        word = word[:-1]
    for suffix in ("ing", "ed", "er"):
        if len(word) > len(suffix) + 2 and word.endswith(suffix):
            word = word[: -len(suffix)]
            if len(word) > 2 and word[-1] == word[-2] and word[-1] not in "aeiouls":
                word = word[:-1]
            break
    if len(word) > 3 and word.endswith("e"):
        word = word[:-1]
    return word


def _tokens(text: str) -> set[str]:
    words = re.findall(r"[a-z][a-z0-9+-]*", text.lower())
    return {word[:-1] if len(word) > 3 and word.endswith("s") else word for word in words if word not in _STOPWORDS}


def content_words(text: str) -> set[str]:
    """Stemmed topic words for the lexical gate, the leak metric and tie-break clauses."""
    return {_stem(word) for word in re.findall(r"[a-z][a-z0-9]*", text.lower()) if word not in _LEXICAL_STOPWORDS}


def trigger_overlap(a: Skill, b: Skill) -> tuple[float, set[str]]:
    left, right = _tokens(a.trigger), _tokens(b.trigger)
    union = left | right
    shared = left & right
    return (len(shared) / len(union) if union else 0.0), shared


@dataclass(frozen=True)
class Lexicon:
    words: dict[str, frozenset[str]]
    weight: dict[str, float]
    frequency: dict[str, int]

    @classmethod
    def build(cls, choices: dict[str, Skill]) -> "Lexicon":
        words = {key: frozenset(content_words(choice.own_text)) for key, choice in choices.items()}
        frequency: dict[str, int] = {}
        for vocabulary in words.values():
            for word in vocabulary:
                frequency[word] = frequency.get(word, 0) + 1
        total = max(len(words), 1)
        weight = {word: math.log(total / count) + 0.1 for word, count in frequency.items()}
        return cls(words, weight, frequency)

    def score(self, prompt_words: set[str], key: str) -> float:
        return sum(self.weight[word] for word in prompt_words & self.words.get(key, frozenset()))

    def rare(self, word: str) -> bool:
        return self.frequency.get(word, 0) <= RARE_MAX_DF


def plausibility_problem(case: Case, lexicon: Lexicon, choices: dict[str, Skill]) -> str | None:
    """Why the words in the prompt contradict the label, or None."""
    words = content_words(case.prompt)
    if NONE in case.accept:
        best = max(choices, key=lambda key: lexicon.score(words, key), default=None)
        if best is not None and lexicon.score(words, best) >= NONE_MAX_SCORE:
            return f"expects none, but it shares words with {best} (score {lexicon.score(words, best):.1f})"
        return None
    by_job: dict[str, float] = {}
    for key in case.accept:
        by_job[job_of(key)] = max(by_job.get(job_of(key), 0.0), lexicon.score(words, key))
    expected = min(by_job.values(), default=0.0)
    rejected = max((lexicon.score(words, key) for key in case.reject), default=0.0)
    if expected <= 0:
        return "shares no description words with the expected choice"
    if expected <= rejected:
        return f"the expected choice scores {expected:.1f}, not above a rejected one at {rejected:.1f}"
    return None


def nonsense_problem(prompt: str) -> str | None:
    words = re.findall(r"[A-Za-z][A-Za-z']*", prompt)
    if len(words) < MIN_PROMPT_WORDS:
        return f"has {len(words)} word(s); write a request of at least {MIN_PROMPT_WORDS} words"
    if not any(word.lower() in _FUNCTION_WORDS for word in words):
        return "has no function words, so it is not a request a person would type"
    return None


def name_variants(key: str) -> set[str]:
    name = key[len(AGENT_KEY):] if key.startswith(AGENT_KEY) else key
    short = name.removeprefix("mastermind-")
    variants = {name, short}
    return variants | {variant.replace("-", " ") for variant in variants}


def leaked_name(prompt: str, key: str) -> str | None:
    for variant in sorted(name_variants(key), key=len, reverse=True):
        if re.search(rf"(?<![\w-]){re.escape(variant)}(?![\w-])", prompt, re.IGNORECASE):
            return variant
    return None


def exclusive_rare_words(case: Case, lexicon: Lexicon) -> set[str]:
    words = content_words(case.prompt)
    expected = set().union(*(lexicon.words.get(key, frozenset()) for key in case.accept)) if case.accept else set()
    rejected = set().union(*(lexicon.words.get(key, frozenset()) for key in case.reject)) if case.reject else set()
    return {word for word in words & expected if lexicon.rare(word) and word not in rejected}


def tie_break_problem(case: Case, choices: dict[str, Skill]) -> str | None:
    """A conflict case cites the clause that picks its winner, and only in that direction."""
    (job,) = jobs(case.accept)
    winner = choices.get(job)
    if winner is None:
        return None
    cited = case.tie_break
    if not cited:
        return "a conflict case with one winner needs a `tie_break` quoting the clause that settles it"
    needle = cited.casefold()
    losers = [choices[key] for key in sorted(case.reject) if key in choices]
    loser_side = [
        loser for loser in losers
        if any(winner.name in r.targets and needle in r.clause.casefold() for r in loser.boundaries)
    ]
    winner_side = needle in winner.trigger.casefold() and not any(needle in loser.trigger.casefold() for loser in losers)
    if not loser_side and not winner_side:
        return (
            f"tie_break {cited!r} is neither a rejected choice's 'Not for ... (use {winner.name})' clause "
            f"nor a phrase only {winner.name}'s trigger claims"
        )
    prompt_words = content_words(case.prompt)
    cited_hits = len(prompt_words & content_words(cited))
    for loser in losers:
        for referral in winner.boundaries:
            if loser.name not in referral.targets:
                continue
            if needle in referral.clause.casefold():
                return f"{winner.name} sends the cited trigger back to {loser.name}"
            back_hits = len(prompt_words & content_words(referral.clause))
            if back_hits >= cited_hits:
                return (
                    f"the prompt matches {winner.name}'s hand-off to {loser.name} ({referral.clause!r}) "
                    f"as well as the cited clause, so the descriptions do not pick one winner"
                )
    return None


def _cycles(graph: dict[str, tuple[str, ...]]) -> list[list[str]]:
    """Simple referral cycles of three or more skills.

    A pair that names each other draws one boundary from both sides, which is
    the point. A longer loop means each hands the prompt on and none keeps it.
    """
    found: set[tuple[str, ...]] = set()

    def walk(start: str, node: str, path: list[str]) -> None:
        for nxt in graph.get(node, ()):
            if nxt == start and len(path) >= 3:
                pivot = path.index(min(path))
                found.add(tuple(path[pivot:] + path[:pivot]))
            elif nxt not in path and nxt > start:
                walk(start, nxt, path + [nxt])

    for start in sorted(graph):
        walk(start, start, [start])
    return [list(cycle) for cycle in sorted(found)]


def check(
    skills: dict[str, Skill] | None = None,
    cases: list[Case] | None = None,
    core: frozenset[str] | None = None,
    catalogue: Path | None = CATALOGUE,
    adjacent: frozenset[frozenset[str]] | None = None,
    corpus_gates: bool = True,
) -> Report:
    """Check the cases against the choices.

    `corpus_gates` turns on the split share and the leak rate, which only make
    sense for a corpus of realistic size.
    """
    report = Report()
    problems = report.problems
    choices = load_choices() if skills is None else skills
    cases = load_cases(problems=problems) if cases is None else cases
    core = core_skills() if core is None else core
    adjacent = ADJACENT_PAIRS if adjacent is None else adjacent
    known = set(choices)
    skill_names = {key for key, choice in choices.items() if choice.kind == "skill"}
    lexicon = Lexicon.build(choices)

    if catalogue is not None:
        current = catalogue.read_text(encoding="utf-8") if catalogue.is_file() else ""
        if current != render_catalogue(choices):
            problems.append(f"{catalogue.relative_to(REPO_ROOT)} is stale; run `python -m evals.selection --write`")

    for left, right in combinations(sorted(skill_names), 2):
        if right.startswith(left) or left.startswith(right):
            problems.append(f"skill names {left} and {right} share a prefix, so answer phrases cannot tell them apart")

    for name in sorted(core - skill_names):
        problems.append(f"core profile names {name}, which has no SKILL.md")

    positives = {name: 0 for name in skill_names}
    negatives = {name: 0 for name in skill_names}
    separated: set[frozenset[str]] = set()
    conflict_pairs: set[frozenset[str]] = set()
    positive_cases: list[Case] = []
    for case in cases:
        for key in sorted((case.accept | case.reject) - known - {NONE}):
            problems.append(f"{case.id}: {key} is not a shipped skill or subagent")
        if NONE in case.accept and len(case.accept) > 1:
            problems.append(f"{case.id}: 'none' cannot share a tie with a skill")
        if case.accept & case.reject:
            problems.append(f"{case.id}: {sorted(case.accept & case.reject)} both expected and rejected")
        if not case.reject:
            problems.append(f"{case.id}: name at least one near miss to reject")
        for key in sorted(case.accept - {NONE}):
            variant = leaked_name(case.prompt, key)
            if variant is not None:
                problems.append(f"{case.id}: the prompt names {variant!r}, so the case tests nothing")
        for key in sorted(case.accept & known):
            choice = choices[key]
            for agent_key, agent in choices.items():
                if agent.kind == "agent" and agent.job == choice.job and agent_key not in case.accept and choice.kind == "skill":
                    problems.append(f"{case.id}: {agent.name} does {key}'s job, so expect {AGENT_ANSWER}{agent.name} as well")
        nonsense = nonsense_problem(case.prompt)
        if nonsense:
            problems.append(f"{case.id}: the prompt {nonsense}")
        if (case.accept | case.reject) <= known | {NONE}:
            implausible = plausibility_problem(case, lexicon, choices)
            if implausible and not case.lexical_exception:
                problems.append(
                    f"{case.id}: the prompt {implausible}; fix the label or prompt, or add a reviewed `lexical_exception`"
                )
            elif case.lexical_exception and not implausible:
                problems.append(f"{case.id}: `lexical_exception` is not needed, the words already support the label; remove it")
        if NONE not in case.accept and case.accept <= known:
            positive_cases.append(case)
            if case.id.endswith(PARAPHRASE_SUFFIX):
                shared = sorted(
                    word for word in content_words(case.prompt)
                    if lexicon.rare(word) and any(word in lexicon.words[key] for key in case.accept)
                )
                if shared:
                    problems.append(f"{case.id}: a paraphrase must share no rare words with the expected description, found {shared}")
        for key in case.accept & skill_names:
            positives[key] += 1
        for key in case.reject & skill_names:
            negatives[key] += 1
        for winner in case.accept & skill_names:
            for loser in case.reject & skill_names:
                separated.add(frozenset((winner, loser)))
                if case.conflict:
                    conflict_pairs.add(frozenset((winner, loser)))
        if case.tie:
            report.ties.append(case)
        elif case.conflict and case.accept <= known and case.reject <= known:
            problem = tie_break_problem(case, choices)
            if problem:
                problems.append(f"{case.id}: {problem}")

    for name in sorted(skill_names):
        need_pos, need_neg = (
            (CORE_MIN_POSITIVE, CORE_MIN_NEGATIVE) if name in core else (OTHER_MIN_POSITIVE, OTHER_MIN_NEGATIVE)
        )
        if positives[name] < need_pos:
            problems.append(f"{name}: {positives[name]} prompt(s) expect it; need {need_pos}")
        if negatives[name] < need_neg:
            problems.append(f"{name}: {negatives[name]} prompt(s) reject it; need {need_neg}")

    graph = {name: choices[name].referrals for name in sorted(skill_names)}
    for name, targets in graph.items():
        for target in targets:
            if name in core and target in skill_names and target not in core:
                problems.append(
                    f"{name} is in core but its description sends the host to {target}, which core does not install"
                )
    for cycle in _cycles(graph):
        problems.append("referral loop with no owner: " + " -> ".join(cycle + [cycle[0]]))

    for pair in sorted(adjacent, key=sorted):
        left, right = sorted(pair)
        if not pair <= skill_names:
            problems.append(f"adjacent pair {left} / {right} names a skill that is not shipped")
        elif pair not in conflict_pairs:
            problems.append(f"{left} and {right} are known neighbours; add a `sel-conflict-` case that expects one and rejects the other")

    for left, right in combinations(sorted(skill_names), 2):
        score, shared = trigger_overlap(choices[left], choices[right])
        if score < OVERLAP_THRESHOLD:
            continue
        report.overlaps.append((score, left, right, ", ".join(sorted(shared))))
        if frozenset((left, right)) not in separated:
            problems.append(
                f"{left} and {right} share trigger words ({', '.join(sorted(shared))}) and no prompt "
                "separates them; add a case that expects one and rejects the other"
            )
    report.overlaps.sort(reverse=True)

    splits = {"train": [], "held-out": []}
    for case in cases:
        splits[runner.selection_split(case.id)].append(case)
    report.splits = {name: len(members) for name, members in splits.items()}
    if cases and corpus_gates:
        held_out_share = len(splits["held-out"]) / len(cases)
        if not HELD_OUT_MIN_SHARE <= held_out_share <= HELD_OUT_MAX_SHARE:
            problems.append(
                f"the held-out split has {held_out_share:.0%} of the cases; keep it between "
                f"{HELD_OUT_MIN_SHARE:.0%} and {HELD_OUT_MAX_SHARE:.0%} or change SELECTION_SPLIT_SALT"
            )

    for case in positive_cases:
        rare = exclusive_rare_words(case, lexicon)
        if len(rare) >= LEAK_MIN_RARE_WORDS:
            report.leaks.append((case.id, tuple(sorted(rare))))
    leaked = {case_id for case_id, _ in report.leaks}
    for label, members, limit in (
        ("corpus", positive_cases, LEAK_MAX_SHARE),
        ("held-out split", [c for c in positive_cases if runner.selection_split(c.id) == "held-out"], LEAK_MAX_SHARE_HELD_OUT),
    ):
        if not members or not corpus_gates:
            continue
        share = sum(case.id in leaked for case in members) / len(members)
        report.notes.append(f"leak rate in the {label}: {share:.0%} of {len(members)} positive cases")
        if share > limit:
            problems.append(
                f"{share:.0%} of the {label}'s positive cases share {LEAK_MIN_RARE_WORDS}+ rare words only with the "
                f"expected description (limit {limit:.0%}); add paraphrased prompts"
            )
    return report
