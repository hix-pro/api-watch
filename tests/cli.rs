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

#[test]
fn cold_check_without_targets_exits_one() {
    let dir = std::env::temp_dir().join(format!("apiwatch-cli-cold-{}", std::process::id()));
    fs::remove_dir_all(&dir).ok();
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("test.db").to_str().unwrap().to_string();

    let output = apiwatch().args(["--db", &db, "check"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no targets yet"), "{stderr}");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn readding_the_same_identity_keeps_the_stored_baseline() {
    let dir = std::env::temp_dir().join(format!("apiwatch-cli-idem-{}", std::process::id()));
    fs::remove_dir_all(&dir).ok();
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("test.db").to_str().unwrap().to_string();
    let spec = dir.join("spec.json").to_str().unwrap().to_string();

    fs::write(
        &spec,
        r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},"paths":{"/pets":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#,
    )
    .unwrap();

    let first = apiwatch()
        .args(["--db", &db, "add", &spec, "--name", "t"])
        .output()
        .unwrap();
    assert!(first.status.success(), "{first:?}");
    assert!(String::from_utf8_lossy(&first.stdout).contains("Added t"));

    let baseline = apiwatch()
        .args(["--db", &db, "check", "t"])
        .output()
        .unwrap();
    assert_eq!(baseline.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&baseline.stdout).contains("baseline saved"));

    // Same source again: a no-op that exits 0 and keeps rows and baseline.
    let again = apiwatch()
        .args(["--db", &db, "add", &spec, "--name", "t"])
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(0), "{again:?}");
    let stdout = String::from_utf8_lossy(&again.stdout);
    assert!(stdout.contains("Already tracked"), "{stdout}");
    assert!(stdout.contains("Keeping the stored baseline"), "{stdout}");

    let history = apiwatch()
        .args(["--db", &db, "history", "t", "--json"])
        .output()
        .unwrap();
    assert!(history.status.success());
    let stdout = String::from_utf8_lossy(&history.stdout);
    assert!(stdout.contains("\"total\": 1"), "{stdout}");

    let check = apiwatch()
        .args(["--db", &db, "check", "t"])
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&check.stdout);
    assert!(stdout.contains("no contract changes"), "{stdout}");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn readding_a_different_url_exits_one_with_guidance() {
    let dir = std::env::temp_dir().join(format!("apiwatch-cli-dup-{}", std::process::id()));
    fs::remove_dir_all(&dir).ok();
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("test.db").to_str().unwrap().to_string();
    let spec_a = dir.join("a.json").to_str().unwrap().to_string();
    let spec_b = dir.join("b.json").to_str().unwrap().to_string();

    fs::write(
        &spec_a,
        r#"{"openapi":"3.0.0","info":{"title":"A","version":"1"},"paths":{"/a":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#,
    )
    .unwrap();
    fs::write(
        &spec_b,
        r#"{"openapi":"3.0.0","info":{"title":"B","version":"1"},"paths":{"/b":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#,
    )
    .unwrap();

    let first = apiwatch()
        .args(["--db", &db, "add", &spec_a, "--name", "t"])
        .output()
        .unwrap();
    assert!(first.status.success(), "{first:?}");

    let again = apiwatch()
        .args(["--db", &db, "add", &spec_b, "--name", "t"])
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(1), "{again:?}");
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert!(stderr.contains("different URL or headers"), "{stderr}");
    assert!(stderr.contains("apiwatch remove t"), "{stderr}");

    fs::remove_dir_all(&dir).ok();
}
