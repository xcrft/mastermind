# Research evidence fixture

A small Python/Rust repository for bounded factual research. The current tree
contains duplicate function names, a Rust callback registration, a dynamic
Python registry, conflicting documentation, and an ADR supersession chain.
The newest ADR is proposed, while an older accepted ADR matches current
configuration. The baseline preserves the previous configuration and ADR state.

Each case asks an ordinary factual question and checks citations against unique
source-line anchors. No fixture prompt supplies a synthetic graph answer or a
tool sequence. The corpus is a starter regression benchmark; creating or
statically validating it does not measure model quality or production execution.

The fixture has no build requirement. Source parsing and the existing mmcg
binary can inspect it without compiling or executing its application code.
