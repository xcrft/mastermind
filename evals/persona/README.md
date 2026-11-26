# Persona evaluations

These commands check persona mining and profile delivery without a model call. Mining accuracy, delivery and whether a model applies the rules are separate questions, and neither command answers the last one. The mining contract is in the [persona evidence reference](../../docs/reference/persona-evidence.md). Run the commands from the repository root and keep every output private, because it holds repository metadata or rule IDs.

## Replay a Git corpus

`replay` runs a pinned local Git history through isolated persona stores and checks mining predicates, provenance and store behavior. It uses no network, checkout hook or real user profile.

```sh
python3 -m evals.persona --repo /absolute/path/to/repo --binary /absolute/path/to/mmcg \
  --output /private/path/new-replay
```

`--anchor` picks the ref to replay and defaults to `origin/main`. To repeat a replay on the exact same refs, pass `--manifest` with the earlier replay's manifest.

## Check a retained delivery

`delivery` compares the rules you expected with a kept `mmcg_profile` response.

1. Write `expected.json` as an array of `{"key": "claim-id", "statement": "complete rule including exceptions"}`, with the rules that fit this task's paths, role, workflow and audience. Keep the original response as `packet.json` and never edit it.
2. Run:

   ```sh
   python3 -m evals.persona.delivery --expected /private/path/expected.json \
     --packet /private/path/packet.json --output /private/path/new-delivery.json
   ```

The output binds both input hashes, keeps the review, view and store revisions and lists returned, missing, changed and unexpected rule IDs without the statement text. Source verification is what the packet reports, because the command doesn't reopen the original evidence. If the inputs are malformed, fix them and use a new output path. Keep denied access and budget omissions in the record, and don't fill a missing profile from `style.md` or the private inbox.

To measure whether a model applies the rules, run matched arms with the same rule text, runtime and tools in the [research benchmark](../benchmark/README.md), and judge the actual answer or diff per rule.
