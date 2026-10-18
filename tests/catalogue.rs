use std::{fs, process::Command};
use sverka::{create_snapshot, diff, load};
#[test]
fn snapshot_resume_diff_verify() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("skip")).unwrap();
    fs::write(root.join("a.txt"), "alpha").unwrap();
    fs::write(root.join("b.txt"), "beta").unwrap();
    fs::write(root.join("skip/secret"), "ignored").unwrap();
    let out = dir.path().join("one.json");
    let checkpoint = dir.path().join("checkpoint.jsonl");
    let one = create_snapshot(
        &root,
        Some(&out),
        Some(&checkpoint),
        false,
        2,
        vec!["skip".into()],
    )
    .unwrap();
    assert_eq!(one.files.len(), 2);
    let resumed =
        create_snapshot(&root, None, Some(&checkpoint), true, 2, vec!["skip".into()]).unwrap();
    assert!(!diff(&one, &resumed).has_changes());
    assert_eq!(load(&out).unwrap().files.len(), 2);
    fs::rename(root.join("a.txt"), root.join("moved.txt")).unwrap();
    fs::write(root.join("b.txt"), "corrupt").unwrap();
    fs::write(root.join("new.txt"), "new").unwrap();
    let two = create_snapshot(&root, None, None, false, 2, vec!["skip".into()]).unwrap();
    let changes = diff(&one, &two);
    assert_eq!(changes.moved.len(), 1);
    assert_eq!(changes.changed, vec!["b.txt"]);
    assert_eq!(changes.added, vec!["new.txt"]);
    let result = Command::new(env!("CARGO_BIN_EXE_sverka"))
        .args(["verify", out.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(json["integrity_ok"], false);
}
#[test]
fn missing_and_atomic_previous_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a"), "abc").unwrap();
    let output = dir.path().join("snapshot.json");
    let old = create_snapshot(&root, Some(&output), None, false, 1, vec![]).unwrap();
    let bytes = fs::read(&output).unwrap();
    assert!(
        create_snapshot(
            &dir.path().join("absent"),
            Some(&output),
            None,
            false,
            1,
            vec![]
        )
        .is_err()
    );
    assert_eq!(fs::read(&output).unwrap(), bytes);
    fs::remove_file(root.join("a")).unwrap();
    let new = create_snapshot(&root, None, None, false, 1, vec![]).unwrap();
    assert_eq!(diff(&old, &new).missing, vec!["a"]);
}
#[test]
fn help_and_bad_input() {
    let output = Command::new(env!("CARGO_BIN_EXE_sverka"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("snapshot"));
    let output = Command::new(env!("CARGO_BIN_EXE_sverka"))
        .args(["diff", "missing-one", "missing-two", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap()["error"].is_string()
    );
}

#[test]
fn partial_checkpoint_and_output_safety() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a"), "alpha").unwrap();
    let out = dir.path().join("snapshot.json");
    let cp = dir.path().join("progress.jsonl");
    let one = create_snapshot(&root, Some(&out), Some(&cp), false, 1, vec![]).unwrap();
    let mut f = fs::OpenOptions::new().append(true).open(&cp).unwrap();
    f.write_all(b"{\"path\":\"unfinished").unwrap();
    drop(f);
    let two = create_snapshot(&root, Some(&out), Some(&cp), true, 1, vec![]).unwrap();
    assert!(!diff(&one, &two).has_changes());
    let user_file = dir.path().join("important.txt");
    fs::write(&user_file, "do not overwrite").unwrap();
    assert!(create_snapshot(&root, Some(&user_file), None, false, 1, vec![]).is_err());
    assert!(create_snapshot(&root, None, Some(&user_file), false, 1, vec![]).is_err());
    assert_eq!(fs::read_to_string(&user_file).unwrap(), "do not overwrite");
    assert!(create_snapshot(&root, Some(&out), Some(&out), true, 1, vec![]).is_err());
}
