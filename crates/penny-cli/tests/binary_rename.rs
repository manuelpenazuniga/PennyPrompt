// The shipped binary is `pennyprompt`. A `penny-cli` compatibility symlink ships
// for one release train and prints a deprecation notice when invoked. These tests
// cover the code side of the rename (#236); the installer's asset-name fallback is
// exercised separately by the release/installer smoke checks.

use std::process::Command;

#[test]
fn pennyprompt_version_runs_under_new_name() {
    let output = Command::new(env!("CARGO_BIN_EXE_pennyprompt"))
        .arg("--version")
        .output()
        .expect("run pennyprompt --version");
    assert!(
        output.status.success(),
        "pennyprompt --version failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(
        stdout.contains("pennyprompt"),
        "version output should name the binary: {stdout}"
    );
    // No deprecation notice when invoked under the current name.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("deprecated"),
        "unexpected deprecation notice under new name: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn legacy_penny_cli_name_prints_deprecation_notice() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().expect("temp dir");
    let legacy = dir.path().join("penny-cli");
    symlink(env!("CARGO_BIN_EXE_pennyprompt"), &legacy).expect("create penny-cli symlink");

    let output = Command::new(&legacy)
        .arg("--version")
        .output()
        .expect("run penny-cli --version via symlink");
    assert!(
        output.status.success(),
        "penny-cli --version via symlink failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`penny-cli` is deprecated"),
        "expected deprecation notice on legacy invocation, got: {stderr}"
    );
}
