use std::process::Command;

fn help_output(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_pennyprompt"))
        .args(args)
        .output()
        .expect("run pennyprompt help");

    assert!(
        output.status.success(),
        "help command failed: status={:?}, stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).expect("help output is utf-8")
}

fn assert_contains_all(output: &str, expected: &[&str]) {
    for text in expected {
        assert!(
            output.contains(text),
            "expected help output to contain `{text}`\n\n{output}"
        );
    }
}

#[test]
fn root_help_lists_descriptions_for_top_level_commands() {
    let output = help_output(&["--help"]);

    assert_contains_all(
        &output,
        &[
            "initialize local config, database, budgets, and pricebooks",
            "check config, database, provider keys, pricebook freshness, and binds",
            "print resolved configuration",
            "inspect or update local model pricebooks",
            "list, set, or reset local budget guardrails",
            "estimate token and cost range before running work",
            "inspect or resume detector-paused sessions",
            "run a local agent command through PennyPrompt proxy wiring",
            "start proxy and admin planes",
            "stream admin events from the local control plane",
            "summarize persisted request cost data",
            "render a compact local cost dashboard",
        ],
    );
}

#[test]
fn nested_help_lists_descriptions_for_command_groups() {
    let cases = [
        (
            ["prices", "--help"],
            &[
                "show active model prices",
                "import bundled local pricebooks",
            ][..],
        ),
        (
            ["budget", "--help"],
            &[
                "list configured runtime budgets",
                "set or update a runtime budget",
                "remove a runtime budget",
            ][..],
        ),
        (
            ["detect", "--help"],
            &[
                "show detector status, active alerts, and paused sessions",
                "resume a paused session",
            ][..],
        ),
        (
            ["report", "--help"],
            &[
                "summarize requests by project, model, or session",
                "show highest-cost requests",
            ][..],
        ),
        (
            ["run", "--help"],
            &[
                "Use the bundled mock provider",
                "Arguments passed to the agent command",
            ][..],
        ),
        (
            ["serve", "--help"],
            &[
                "Start serve in the background",
                "Show background serve status",
                "Stop background serve using the pid file",
            ][..],
        ),
    ];

    for (args, expected) in cases {
        let output = help_output(&args);
        assert_contains_all(&output, expected);
    }
}
