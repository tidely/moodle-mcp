//! Runs the `moodle` binary against the mock Moodle from `sync`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;

use serde_json::Value;

struct Run {
    code: i32,
    stdout: Value,
    stderr: String,
}

async fn cli(base: &str, home: &Path, cwd: &Path, args: &[&str]) -> Run {
    let (base, home, cwd) = (base.to_owned(), home.to_owned(), cwd.to_owned());
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let out = Command::new(env!("CARGO_BIN_EXE_moodle"))
            .args(&args)
            .current_dir(&cwd)
            .env("MOODLE_URL", &base)
            .env("MOODLE_TOKEN", mock::TOKEN)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .output()
            .unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: serde_json::from_str(&stdout).unwrap_or(Value::Null),
            stderr: String::from_utf8(out.stderr).unwrap(),
        }
    })
    .await
    .unwrap()
}

#[test]
fn command_name() {
    for flag in ["--help", "--version"] {
        let out = Command::new(env!("CARGO_BIN_EXE_moodle"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(stdout.contains("moodle"), "{stdout}");
        assert!(!stdout.contains("moodle-cli"), "{stdout}");
        if flag == "--help" {
            assert!(stdout.contains("Usage: moodle"), "{stdout}");
        } else {
            assert!(stdout.starts_with("moodle "), "{stdout}");
        }
    }
}

#[tokio::test]
async fn agent_workflow() {
    let mock = mock::mock_moodle().await;
    let base = mock.base.as_str();
    let tmp = std::env::temp_dir().join(format!("moodle-cli-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Current courses only by default; compact JSON when piped.
    let r = cli(base, &tmp, &tmp, &["courses"]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.as_array().unwrap().len(), 1, "{}", r.stdout);
    assert_eq!(r.stdout[0]["id"], 42);
    let r = cli(base, &tmp, &tmp, &["courses", "--all"]).await;
    assert_eq!(r.stdout.as_array().unwrap().len(), 2);

    // Mirror commands outside a mirror fail with a code and a hint.
    let r = cli(base, &tmp, &tmp, &["status"]).await;
    assert_eq!(r.code, 1);
    let err: Value = serde_json::from_str(r.stderr.trim()).unwrap();
    assert_eq!(err["error"]["code"], "not_a_mirror");
    assert!(
        err["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("moodle clone")
    );

    // Clone with a small size limit, git-style into ./<shortname> (<id>).
    let r = cli(base, &tmp, &tmp, &["clone", "42", "--max-file-mb", "0"]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let root = PathBuf::from(r.stdout["root"].as_str().unwrap());
    assert_eq!(root, std::path::absolute(tmp.join("DB_2026 (42)")).unwrap());
    assert!(root.join(".moodle/mirror.json").is_file());
    assert!(root.join(".moodle/manifest.json").is_file());
    assert!(
        root.join("Slides (1400)/big.mp4").is_file(),
        "no limit with 0"
    );

    // Cloning again points at pull.
    let r = cli(base, &tmp, &tmp, &["clone", "42"]).await;
    let err: Value = serde_json::from_str(r.stderr.trim()).unwrap();
    assert_eq!(err["error"]["code"], "already_cloned");

    // Lower the limit on pull; work from a subdirectory like git.
    std::fs::remove_file(root.join("Slides (1400)/big.mp4")).unwrap();
    let sub = root.join("Project 2 (1337)");
    let r = cli(base, &tmp, &sub, &["pull", "--max-file-mb", "0"]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout["downloaded"],
        serde_json::json!(["Slides (1400)/big.mp4"])
    );

    // status is offline and reports the manifest.
    let r = cli(base, &tmp, &sub, &["status"]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout["course_id"], 42);
    assert!(r.stdout["synced_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(r.stdout["skipped"], serde_json::json!([]));

    // fetch by file path relative to the working directory.
    std::fs::remove_file(root.join("Slides (1400)/small.pdf")).unwrap();
    let r = cli(base, &tmp, &root, &["fetch", "Slides (1400)/small.pdf"]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout["downloaded"],
        serde_json::json!(["Slides (1400)/small.pdf"])
    );
    let r = cli(base, &tmp, &root, &["fetch", "nope.pdf"]).await;
    let err: Value = serde_json::from_str(r.stderr.trim()).unwrap();
    assert_eq!(err["error"]["code"], "unknown_target");

    // todo links to the local index when run inside the course's mirror.
    let r = cli(base, &tmp, &root, &["todo"]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let item = &r.stdout[0];
    assert_eq!(item["cmid"], 1337);
    assert_eq!(item["action"], "Add submission");
    assert_eq!(item["overdue"], true);
    assert_eq!(
        item["index"].as_str().unwrap(),
        root.join("Project 2 (1337)/index.md").to_str().unwrap()
    );

    // Removed upstream → removed locally on pull.
    mock.slides_removed.store(true, Ordering::SeqCst);
    let r = cli(base, &tmp, &root, &["pull"]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout["removed"].as_array().unwrap().len() >= 3,
        "{}",
        r.stdout
    );
    assert!(!root.join("Slides (1400)").exists());

    // The token never shows up in output.
    assert!(!r.stderr.contains(mock::TOKEN));
    assert!(!r.stdout.to_string().contains(mock::TOKEN));

    std::fs::remove_dir_all(&tmp).unwrap();
}
