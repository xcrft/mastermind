pub fn run() {
    println!("mastermind tour — guided workflow walkthrough\n");

    println!("  1/6  Index your project");
    println!("       mastermind init");
    println!("       Builds the codegraph, scaffolds .mastermind/, drafts CONTEXT.md.\n");

    println!("  2/6  See what mechanical verification catches (no Claude needed)");
    println!("       mastermind demo hallucinated-symbol");
    println!("       mastermind demo scope-creep");
    println!("       mastermind demo stale-find-block");
    println!("       mastermind demo vacuous-test");
    println!("       mastermind demo signature-drift\n");

    println!("  3/6  Connect your editor");
    println!("       mastermind setup claude --write-mcp   # Claude Code");
    println!("       Cursor / Continue / Codex: see docs/integrations/");
    println!("       Restart your editor after setup.\n");

    println!("  4/6  Create a task spec");
    println!("       mastermind new-spec \"add user session count accessor\"");
    println!("       Fill in the scaffolded spec, then run the workflow.\n");

    println!("  5/6  Run the mechanical gates");
    println!("       mastermind run-task .mastermind/tasks/001-session-count/spec.md");
    println!("       Pre-flight checks the spec. Post-flight audits the executor's report.\n");

    println!("  6/6  Track and resume work");
    println!("       mastermind status          # all tasks + index health");
    println!("       mastermind next            # single recommended next action");
    println!("       mastermind resume <task>   # ready-to-paste Claude prompt\n");

    println!("  Full docs: https://github.com/xcrft/mastermind");
}
