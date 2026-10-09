# Persona evaluations

Keep mining accuracy, profile delivery and model application separate.

| Module | Purpose | Evidence |
|---|---|---|
| `replay` | Replay a frozen Git corpus through isolated stores | Mining predicates, provenance and deterministic store behavior |
| `delivery` | Compare selected rule keys/statements with a retained `mmcg_profile` packet | Returned, missing, changed and unexpected rules; reported revisions and omissions |

## Inspect retained delivery

Prepare `expected.json` as an array of `{ "key": "claim-id", "statement": "complete rule including exceptions" }`.
Select the expected rules for this task's paths, role, workflow and audience.
Retain the original response as `packet.json`; do not rewrite its selection or revisions.

```sh
python3 -m evals.persona.delivery --expected /private/path/expected.json \
  --packet /private/path/packet.json --output /private/path/new-delivery.json
```

The output binds both input hashes, retains review/view/store revisions and lists
missing or changed IDs. It omits statement fields, but rule IDs may contain
readable wording; keep the result private. Existing output files are never
replaced. Source verification is the packet's reported state; this command
does not reopen the original evidence or prove authorship. Model application and
semantic quality remain unknown even when every statement matches.

For a failed inspection, correct malformed inputs and use a new output path.
Keep denied access and budget omissions in the retained record. Do not fill a
missing profile by reading `style.md` or private inbox candidates.

## Measure application

| Comparison control | Requirement |
|---|---|
| Rule content | Same complete statements and exceptions in the inline and profile arms |
| Runtime | Same model/effort, client, tools, permissions, memory and source |
| Execution | Separate scratch directories and fresh sessions; preserve every attempt |
| Delivery | Exact offered packet, selection, revisions, omissions and failures |
| Application | Inspect the actual answer/diff per applicable rule; retain justified exceptions and unknown judgments |
| Correctness | Original-request acceptance, real defects, protected behavior and clean negative cases |
| Resources | All generation, retrieval, review, repair and failure costs; unknown billing stays unknown |

Use [matched instruction trials](../benchmark/README.md) for a portable read-only
prompt experiment. A synthetic profile-shaped instruction does not establish
native hook activation or mining accuracy. Applied fixes require an observed
diff and checks. More lines or repeated judges of one result do not add accepted
tasks or independent task samples. Personal preferences do not grant actions or
replace the task's acceptance criteria.

For changes to locking, ownership, resource lifetime, cancellation or evidence,
the executor records a small preference checklist in report prose. Ordinary
preferences need no separate checklist. The auditor checks its evidence against
the diff. Self-reported compliance
is a claim to review, not a semantic pass or a new controller completion guard.

```sh
python3 -m unittest discover -s tests -t .
python3 scripts/validate.py
```
