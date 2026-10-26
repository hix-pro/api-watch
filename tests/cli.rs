use std::fs;
use std::process::Command;

fn apiwatch() -> Command {
    Command::new(env!("CARGO_BIN_EXE_apiwatch"))
}

#[test]
fn usage_errors_exit_one_instead_of_colliding_with_breaking_changes() {
    let output = apiwatch().arg("--bogus").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument"), "{stderr}");
}

#[test]
fn help_and_version_exit_zero() {
    for flag in ["--help", "--version"] {
        let output = apiwatch().arg(flag).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{flag}");
    }
}

#[test]
fn breaking_changes_exit_two_with_fail_on_breaking() {
    let dir = std::env::temp_dir().join(format!("apiwatch-cli-test-{}", std::process::id()));
    fs::remove_dir_all(&dir).ok();
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("test.db");
    let spec = dir.join("spec.json");
    let db = db.to_str().unwrap();
    let spec_path = spec.to_str().unwrap();

    fs::write(
        &spec,
        r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},"paths":{"/pets":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#,
    )
    .unwrap();
    let added = apiwatch()
        .args(["--db", db, "add", spec_path, "--name", "t"])
        .output()
        .unwrap();
    assert!(added.status.success(), "{added:?}");
    let baseline = apiwatch()
        .args(["--db", db, "check", "t"])
        .output()
        .unwrap();
    assert_eq!(baseline.status.code(), Some(0));

    fs::write(
        &spec,
        r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},"paths":{}}"#,
    )
    .unwrap();
    let breaking = apiwatch()
        .args(["--db", db, "check", "t", "--fail-on-breaking"])
        .output()
        .unwrap();
    assert_eq!(breaking.status.code(), Some(2), "{breaking:?}");
    let stdout = String::from_utf8_lossy(&breaking.stdout);
    assert!(stdout.contains("endpoint removed"), "{stdout}");

    fs::remove_dir_all(&dir).ok();
}
