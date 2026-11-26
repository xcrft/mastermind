# Concept search

Concept search finds symbols by plain-language terms, such as "payment retry
handler", without a model, embeddings or network access. `mastermind concept`
and `mmcg_concept` return lexical candidates. For a question about behavior, use
the source review in `mastermind concept-review` or `mmcg_concept_prepare` and
`mmcg_concept_finish`: it adds candidates from operation bodies, and the calling
agent judges each one against source.

| Use | CLI | MCP |
|---|---|---|
| Ranked lexical candidates | `mastermind concept "payment retry handler" --top 10 --format json` | `mmcg_concept` |
| Source review of operation relevance | `mastermind concept-review prepare`, then `finish` | `mmcg_concept_prepare`, `mmcg_read`, `mmcg_concept_finish` |

## Search by concept

```sh
mastermind concept "payment retry handler" --top 10 --format json
mastermind concept "payment retry handler" --include-tests --format json
```

The search runs over a private derived corpus with normalized symbol names,
repository-relative paths, bounded declaration shapes and owned documentation.
Source bodies and raw comments are never stored or returned. Quoted,
character, raw, template, regex, numeric and default-value content is removed
from declaration shapes before they are stored.

| Language | Documentation that counts |
|---|---|
| Rust | Consecutive outer `///` or `/** */` docs on the item right below them, also across attributes. Inner `//!` docs belong only to their module |
| Python | The first plain string statement in a module, class, function or async function body |
| TypeScript, TSX, JavaScript | One `/** ... */` JSDoc block right before the declaration, also through an export wrapper |
| C, C++, C#, Go, Java, PHP, Vue | None. Name, path and declaration shape only |

Blank-line gaps, ordinary and trailing comments, file and license headers,
Python f-strings, later or assigned strings, and JSDoc before another statement
are not attached.

| Corpus limit | Value |
|---|---|
| Documentation candidate | Stripped without rendering Markdown or HTML, at most 2 KiB of raw UTF-8 |
| Stored tokens | 512 bytes per symbol, 64 KiB per file, in stable symbol order |
| Secret-like documentation | Private-key headers, AWS access-key shapes, bearer values and non-placeholder api-key, secret, token or password assignments drop the whole candidate |
| Index stats | Supported languages, document count, credential omissions, size omissions and unsupported-language file count. Never matched text |

### Query rules

| Rule | Contract |
|---|---|
| Query | Plain text, at most 256 UTF-8 bytes |
| Terms | Unicode lowercase, split on punctuation, paths, snake, kebab, camel and acronym transitions and letter-digit boundaries. At most 16 terms of 64 bytes |
| Matching | SQLite FTS5 with Porter stemming (`unicode61 remove_diacritics 0 tokenchars '_'`) on corpus and query. Stemming is an English heuristic |
| Duplicates | Terms with the same FTS tokens count once, so `concept concepts` cannot supply two matches. The first spelling stays in `query_terms` |
| Compound names | Kept as whole tokens beside their parts. `parse_JUnit` also matches `junit` |
| Escaping | Every term is quoted, so callers cannot pass FTS syntax, column selectors, operators or prefix wildcards |
| `top` | 1 through 50 |
| Rust tests | Excluded by default: `test`, `rstest` or `::test` attributes, and items inside a `mod` whose only attribute is `#[cfg(test)]`. Compound `cfg` expressions are not recognized. `--include-tests` or MCP `include_tests: true` brings them back |

The test filter runs before the full-match decision, the count and the page
limit. Other languages have no test scope filter.

### Match modes

Let `n` be the number of query terms after deduplication.

| `match_mode` | Selection | `minimum_matched_terms` |
|---|---|---|
| `all_terms` | Fewer than three terms need every term. Also reported when full matches exist and no partial candidate qualifies | `n` |
| `all_and_partial_terms` | Full matches first, then qualifying partial candidates, before the page limit | `max(2, ceil(n / 2))` |
| `partial_terms` | The full query has no match and `n >= 3` | `max(2, ceil(n / 2))` |

Articles, demonstratives, common prepositions, pronouns, copulas and
conjunctions get no ranking weight while other terms remain. Action and polarity
words (`or`, `not`, `without`) keep their weight. If every term is a function
word, all keep weight. A partial candidate also needs
`minimum_ranking_terms = min(3, minimum_matched_terms, ranking_terms.len())`
matches among `ranking_terms`, so grammar words alone cannot qualify it. The rule
does not read intent or negation and can still return unrelated operations that
share terms.

Results sort by matched `ranking_terms`, then by all matched terms, then by FTS5
BM25 with field weights name 10, path 4, declaration shape 2, documentation 1.
Full matches therefore come first. The final tie order is lowercase path, line,
kind, name and symbol ID. Scores are local to one query, lower is better, and
they are not confidence values. The count and the page come from one SQLite
snapshot.

| Candidate field | Content |
|---|---|
| `name`, `kind`, `language`, `path`, `line`, `citation` | Declaration identity |
| `signature_shape` | Declaration shape, at most 256 bytes |
| `matched_fields`, `matched_terms` | Which fields matched, with the retained query spellings |
| `score` | Query-local BM25 |

The envelope reports `indexed_total`, `count`, `result_truncated`,
`unsafe_candidates_omitted`, `truncated` and `truncation_reason`, plus
`query_terms`, `ranking_terms`, `match_mode`, both minimums, `include_tests`,
the requested top, freshness, limits and precision notes. The text format shows
the same policy, coverage and omissions before the candidates. Returned
repository strings are untrusted: JSON keeps them as strings, and text renders
syntax characters as Unicode escapes.

### Freshness and errors

The corpus lives in the index next to the symbols, and documentation rows change
in the same file transaction as symbols and edges. The active normalization
contract is `mmcg-concepts-v5`. A managed query may run one full refresh and
retry when the extractor or concept contract has drifted. A custom external index
opens read-only and returns `index_stale` or `schema_incompatible` when the corpus
is unavailable. Rebuild it with its path and root:

```bash
mastermind --index /path/to/index.db index /path/to/repository --force
```

| Code | Meaning |
|---|---|
| `index_stale`, `schema_incompatible` | Corpus missing or out of date |
| `index_busy` | Another process was writing, so the refresh did not run. `retryable: true`, retry the same query |
| `invalid_arguments` | Bad query, `top` or flags |
| `snapshot_changed`, `index_replaced` | The index changed during the call |
| `index_rebuild_required` | The rebuild did not fit the request budget |
| `work_limit_exceeded`, `cancelled`, `internal_error` | Budget, cancel or internal failure |

## Review operation relevance

Lexical candidates can miss a coordinator with weak vocabulary. The review packet
keeps the lexical page, adds candidates retrieved from operation bodies, and
expands functions and methods inside up to two nominated file owners (the
`owned_scope` channel, at most sixteen extra declarations per owner). The
calling agent then checks the requested action, object and constraints in
source. Topic mentions, dispatchers and nearby operations are not enough.

1. Write `request-contract.json` from the query alone, before you see any
   candidate:

   ```json
   {"operation":"check current source preconditions","constraints":[{"id":"fresh","text":"current source","kind":"state"}]}
   ```

2. Prepare the packet from the indexed repository:

   ```sh
   mastermind concept-review prepare "check current source preconditions" --top 5 \
     --request-contract request-contract.json > packet.json
   jq '.assessment_template' packet.json > assessment.json
   ```

   For an external index, pass `--index` and keep its root and source hashes
   current. If preparation reports stale or changed source, rebuild the index and
   prepare again.

3. Read each candidate's excerpts, and the exact path and line range when an
   excerpt is incomplete. Over MCP, use `mmcg_read`.
4. Fill every assessment entry with `relevant`, `irrelevant` or `unknown` and a
   concrete reason. Relevant entries also need a role, citations and a witness.
5. Finish:

   ```sh
   mastermind concept-review finish --packet packet.json --assessment assessment.json
   ```

   Over MCP, pass the unchanged packet and the complete assessment to
   `mmcg_concept_finish`.

### Request contract and hints

| Field | Rule |
|---|---|
| `operation` | At most 256 UTF-8 bytes. The default ledger keeps the whole query as the operation with no constraints |
| `constraints` | Up to eight, each with a unique `id` (64 bytes), `text` that is an exact nonblank substring of the query (256 bytes) and `kind`: `input`, `state`, `scope` or `result` |
| CLI / MCP | `--request-contract PATH` / `request_contract` |
| Hints | Repeated `--hint` or MCP `hints`: at most eight of 64 UTF-8 bytes, sixteen normalized extra terms and thirty-two combined recall terms. The packet stores them apart from the query. Hints nominate candidates, the original query defines relevance, and the server has no synonym list of its own |

The default ledger does not mean the query has no constraints. When a user
clarifies the scope, prepare a new packet whose query states it, and keep the
original review and the clarification.

### Roles

The packet's `role_contract: "operation-roles-v3"` requires each relevant entry
to declare one role and cite its own declaration in `citations`.

| Role | Required source proof | `witness.relation` |
|---|---|---|
| `implementation` | Performs the requested operation or its defining decision. A direct file owner may cite its contained implementation | `performs_operation` |
| `entrypoint` | Exposes or produces the result through an inspected implementation path, including a specific cached-result path | `exposes_result` |
| `contract` | Operation-specific input or output that the inspected implementation consumes or produces | `operation_contract` |
| `essential_helper` | Performs a necessary domain step, with its caller or consumer inspected. It need not implement the whole workflow | `domain_contribution` |
| `supporting_context` | Specific inspected navigation or explanation for an existing requested operation that meets every explicit constraint | `operation_navigation` or `generic_prerequisite` |

Establish the requested operation before you admit context for it. Nearby code,
opposite behavior, an explicitly narrower scope, or a place where the behavior
could be added is irrelevant, and a source link alone is not enough. Use
`unknown` when a required constraint was not inspected. Prefer a semantic role
for a declaration that exposes the result or enforces an essential step. Type
dispatch and inspected consumers can show that path. Context must explain a
concrete selection, fallback, dispatch point, projection or direct caller or
consumer relationship. Generic call-graph membership is not enough.

| Witness or evidence field | Rule |
|---|---|
| `witness.behavior` | The entry's own observed contribution, at most 2,048 UTF-8 bytes |
| `witness.constraint_checks` | Exactly one check per declared constraint, with `constraint_id`, `status: "matched"` and 1-16 source ranges in the `supporting_evidence` format. A contradicted or uninspected constraint cannot support a relevant judgment |
| `supporting_evidence` | Every role except `implementation`. At most sixteen objects with `path`, `source_sha256`, `start_line` and `end_line`, copied from `packet.source_files`. Entrypoints, contracts and essential helpers need at least one range outside their own declaration |
| `role` on irrelevant or unknown entries | Null or omitted |
| `query_ambiguities` | Unresolved query terms or intent, up to eight nonblank strings of 256 bytes without control characters. With ambiguities, relevant entries can only be `supporting_context` |

A generic readiness gate is context unless its own behavior enforces a defining
query condition. A broad handler can qualify through its inspected
query-specific branch, but sharing a file or delegating does not give it a role.
Ambiguity does not make opposite or missing behavior useful. A new role-contract
version needs a new packet and a new assessment.

### Finish and read the result

Useful lexical entries keep their order. Put extra relevant entries in the order
you want, then finish. The finish step checks the packet, declaration and source
bindings again and rejects changed source. It does not check that your judgment
or role is right.

| Output | Meaning |
|---|---|
| `candidates` | The full useful page, navigation included |
| `semantic_candidates`, `supporting_context` | A split of that page |
| `relevant_total`, `semantic_total`, `context_total` | Assessed counts before the `top` cut |
| `assessment_complete` | Every supplied entry was judged |
| `coverage_complete` | Separate. Judging every entry does not inspect omitted source or candidates outside the pool |

Unknown judgments and unresolved ambiguities make the review incomplete. An
empty result does not prove the repository lacks the capability. Measure
semantic quality on the semantic split and useful-result preservation on the
full page, and compare earlier results by exact declaration identity. The
[scorecard](../../evals/scorecard.md) lists the retained review runs.

| Source review bound | Limit |
|---|---|
| Indexed inventory | 512 files, 20,000 declarations, 32 MiB of source |
| Source per file / per operation | 1 MiB / 32 KiB |
| Candidate pool | Up to 50 lexical, 64 body and 2 × 16 `owned_scope` entries, deduplicated |
| Excerpts / packet | 1,536 bytes per entry / 512 KiB serialized |
| Useful page | Requested `top`, 1 through 50, shared by the semantic and context splits |

Body recall uses a temporary in-memory FTS corpus and never stores source bodies
in the index. Container evidence leaves out indexed child declaration spans, and
the Rust test filter from lexical search applies. The packet reports source
omissions, candidate truncation and evidence truncation.
