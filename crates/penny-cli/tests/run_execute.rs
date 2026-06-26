use std::{path::Path, process::Command};

use tempfile::tempdir;

#[test]
fn run_execute_mock_launches_agent_with_proxy_env() {
    let home = tempdir().expect("home temp dir");
    let workspace = tempdir().expect("workspace temp dir");
    let db_path = home.path().join("pennyprompt.db");
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = r#"test "$PENNY_PROJECT_ID" = run-smoke &&
test "$PENNY_SESSION_ID" = session-smoke &&
case "$PENNY_PROXY_URL" in http://127.0.0.1:*) ;; *) exit 2 ;; esac &&
case "$OPENAI_BASE_URL" in http://127.0.0.1:*/v1) ;; *) exit 3 ;; esac &&
test "$OPENAI_API_BASE" = "$OPENAI_BASE_URL" &&
test "$OPENAI_API_KEY" = pennyprompt-local-proxy"#;

    let output = Command::new(env!("CARGO_BIN_EXE_penny-cli"))
        .current_dir(repo_root)
        .env("HOME", home.path())
        .env_remove("PENNY_CONFIG")
        .env_remove("OPENAI_API_KEY")
        .arg("--database")
        .arg(&db_path)
        .arg("run")
        .arg("sh")
        .arg("--execute")
        .arg("--mock")
        .arg("--project-id")
        .arg("run-smoke")
        .arg("--session-id")
        .arg("session-smoke")
        .arg("--cwd")
        .arg(workspace.path())
        .arg("--")
        .arg("-c")
        .arg(script)
        .output()
        .expect("run penny-cli execute smoke");

    assert!(
        output.status.success(),
        "run execute smoke failed: status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
