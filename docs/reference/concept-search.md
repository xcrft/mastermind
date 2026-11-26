# Concept search

Concept search finds symbols by plain-language terms, such as "payment retry
handler", with no model, embeddings or network access. `mastermind concept` and
`mmcg_concept` return lexical candidates. For a question about behavior, use the
[source review](#review-operation-relevance) (`concept-review prepare` and
`finish`, or `mmcg_concept_prepare`, `mmcg_read` and `mmcg_concept_finish`). It
adds candidates from operation bodies, and the calling agent judges each one
against source.

## Search by concept

```sh
mastermind concept "payment retry handler" --top 10 --format json
mastermind concept "payment retry handler" --include-tests --format json
```

The search runs over a derived corpus of normalized symbol names, paths,
declaration shapes and owned documentation. Source bodies and raw comments are
never stored or returned, and string, numeric and default-value content is
removed from declaration shapes.

| Language | Documentation that counts |
|---|---|
| Rust | Consecutive outer `///` or `/** */` docs on the item below them, also across attributes. Inner `//!` docs belong to their module |
| Python | The first plain string statement in a module, class or function body |
| TypeScript, TSX, JavaScript | One `/** ... */` block right before the declaration, also through an export wrapper |
| C, C++, C#, Go, Java, PHP, Vue | None. Name, path and declaration shape only |

| Corpus limit | Value |
|---|---|
| Documentation candidate | At most 2 KiB of raw UTF-8, stripped without rendering Markdown or HTML |
| Stored tokens | 512 bytes per symbol, 64 KiB per file |
| Secret-like documentation | Private-key headers, AWS key shapes, bearer values and api-key, secret, token or password assignments drop the whole candidate |

### Query rules

| Rule | Contract |
|---|---|
| Query | Plain text, at most 256 UTF-8 bytes |
| Terms | Lowercased and split on punctuation, paths, snake, kebab and camel case and letter-digit boundaries. At most 16 terms of 64 bytes. Compound names also stay whole, so `parse_JUnit` matches `junit` |
| Matching | SQLite FTS5 with Porter stemming, an English heuristic. Terms with the same tokens count once |
| Escaping | Every term is quoted, so callers cannot pass FTS syntax |
| `top` | 1-50 |
| Rust tests | Left out by default: `test`, `rstest` or `::test` attributes and items in a `mod` whose only attribute is `#[cfg(test)]`. `--include-tests` or `include_tests: true` brings them back. Other languages have no test filter |

### Match modes

Let `n` be the number of query terms after deduplication.

| `match_mode` | Selection | `minimum_matched_terms` |
|---|---|---|
| `all_terms` | Fewer than three terms need every term. Also reported when full matches exist and no partial candidate qualifies | `n` |
| `all_and_partial_terms` | Full matches first, then qualifying partial candidates | `max(2, ceil(n / 2))` |
| `partial_terms` | The full query has no match and `n >= 3` | `max(2, ceil(n / 2))` |

Articles, prepositions, pronouns, copulas and conjunctions get no ranking weight
while other terms remain. `or`, `not` and `without` keep theirs. A partial
candidate also needs `minimum_ranking_terms = min(3, minimum_matched_terms,
ranking_terms.len())` matches among `ranking_terms`, so grammar words alone
cannot qualify it. The rule does not read negation.

Results sort by matched `ranking_terms`, then all matched terms, then BM25 with
field weights name 10, path 4, declaration shape 2, documentation 1. Ties sort
by lowercase path, line, kind, name and ID. Scores are local to one query, lower
is better, and they are not confidence values.

Each candidate has `name`, `kind`, `language`, `path`, `line`, `citation`,
`signature_shape` (at most 256 bytes), `matched_fields`, `matched_terms` and
`score`. The envelope reports `indexed_total`, `count`, `result_truncated`,
`unsafe_candidates_omitted`, `truncated`, `truncation_reason`, `query_terms`,
`ranking_terms`, `match_mode`, both minimums, freshness and limits. Repository
strings are untrusted, and the text format renders syntax characters as
Unicode escapes.

### Freshness and errors

The corpus contract is `mmcg-concepts-v5`. A managed query may run one refresh
when the extractor or concept contract has drifted. A custom external index
opens read-only, so rebuild it yourself:

```bash
mastermind --index /path/to/index.db index /path/to/repository --force
```

| Code | Meaning |
|---|---|
| `index_stale`, `schema_incompatible` | Corpus missing or out of date |
| `index_busy` | Another process was writing. `retryable: true` |
| `invalid_arguments` | Bad query, `top` or flags |
| `snapshot_changed`, `index_replaced` | The index changed during the call |
| `index_rebuild_required` | The rebuild did not fit the request budget |
| `work_limit_exceeded`, `cancelled`, `internal_error` | Budget, cancel or internal failure |

## Review operation relevance

Lexical candidates can miss a coordinator with weak vocabulary. The review packet
keeps the lexical page, adds candidates from operation bodies and expands up to
16 functions and methods inside each of up to two nominated file owners (the
`owned_scope` channel). The calling agent then checks the requested action,
object and constraints in source.

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

   If preparation reports stale or changed source, rebuild the index and prepare
   again.

3. Read each candidate's excerpts, and the exact line range when an excerpt is
   incomplete. Over MCP, use `mmcg_read`.
4. Fill every assessment entry with `relevant`, `irrelevant` or `unknown` and a
   concrete reason. Relevant entries also need a role, citations and a witness.
5. Finish. Over MCP, pass the unchanged packet and the assessment to
   `mmcg_concept_finish`:

   ```sh
   mastermind concept-review finish --packet packet.json --assessment assessment.json
   ```

| Request field | Rule |
|---|---|
| `operation` | At most 256 UTF-8 bytes. Without a contract, the whole query is the operation with no constraints |
| `constraints` | Up to 8, each with a unique `id` (64 bytes), `text` that is an exact substring of the query (256 bytes) and `kind`: `input`, `state`, `scope` or `result` |
| CLI / MCP | `--request-contract PATH` / `request_contract` |
| Hints | `--hint` or `hints`: at most 8 of 64 bytes, 16 extra terms. Hints nominate candidates, and only the query defines relevance |

When a user clarifies the scope, prepare a new packet whose query states it.

### Roles

`role_contract: "operation-roles-v3"` requires each relevant entry to declare one
role and cite its own declaration in `citations`.

| Role | Required source proof | `witness.relation` |
|---|---|---|
| `implementation` | Performs the requested operation or its defining decision | `performs_operation` |
| `entrypoint` | Exposes the result through an inspected implementation path | `exposes_result` |
| `contract` | Operation-specific input or output that the implementation consumes or produces | `operation_contract` |
| `essential_helper` | Performs a necessary domain step, with its caller or consumer inspected | `domain_contribution` |
| `supporting_context` | Inspected navigation or explanation for an existing requested operation that meets every constraint | `operation_navigation` or `generic_prerequisite` |

Nearby code, opposite behavior, a narrower scope, or a place where the behavior
could be added is `irrelevant`. Use `unknown` when a required constraint was not
inspected. A generic readiness gate is context unless it enforces a defining
query condition, and sharing a file or delegating does not give a role.

| Witness field | Rule |
|---|---|
| `witness.behavior` | The entry's own observed contribution, at most 2,048 bytes |
| `witness.constraint_checks` | One check per constraint with `constraint_id`, `status: "matched"` and 1-16 source ranges |
| `supporting_evidence` | Every role except `implementation`. At most 16 ranges with `path`, `source_sha256`, `start_line` and `end_line` from `packet.source_files`. Entrypoints, contracts and helpers need one range outside their own declaration |
| `role` on irrelevant or unknown entries | Null or omitted |
| `query_ambiguities` | Up to 8 strings of 256 bytes. With ambiguities, relevant entries can only be `supporting_context` |

### Finish and read the result

Useful lexical entries keep their order, and extra relevant entries follow in
the order you give. Finish rechecks the packet and source bindings and rejects
changed source. It does not check that a judgment is right.

| Output | Meaning |
|---|---|
| `candidates` | The useful page, navigation included |
| `semantic_candidates`, `supporting_context` | A split of that page |
| `relevant_total`, `semantic_total`, `context_total` | Counts before the `top` cut |
| `assessment_complete` | Every supplied entry was judged |
| `coverage_complete` | Separate. Omitted source and candidates outside the pool were not inspected |

Unknown judgments and unresolved ambiguities make the review incomplete, and an
empty result does not prove the capability is missing. The
[scorecard](../../evals/scorecard.md) lists the retained review runs.

| Source review bound | Limit |
|---|---|
| Indexed inventory | 512 files, 20,000 declarations, 32 MiB of source |
| Source per file / per operation | 1 MiB / 32 KiB |
| Candidate pool | 50 lexical, 64 body and 2 × 16 `owned_scope` entries |
| Excerpts / packet | 1,536 bytes per entry / 512 KiB |
| Useful page | `top`, 1-50 |

Body recall uses a temporary in-memory FTS corpus and stores no source bodies in
the index.
