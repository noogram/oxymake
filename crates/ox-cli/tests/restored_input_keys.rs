//! Regressions for remote restoration and read-only run planning.
use assert_cmd::Command;
use rusqlite::Connection;
use std::{fs, path::Path, time::Duration};

fn run(root: &Path, target: &str, remote: Option<&Path>, extra: &[&str]) {
    let mut command = Command::cargo_bin("ox").unwrap();
    command.current_dir(root).args(["run", target]).args(extra);
    if let Some(remote) = remote {
        command.arg("--cache-remote").arg(remote);
    }
    command.timeout(Duration::from_secs(30)).assert().success();
}

#[test]
fn restoring_existing_row_refreshes_downstream_input_hashes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("writer");
    let remote = temp.path().join("remote");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("Oxymakefile.toml"), "[rule.a]\noutput=['a']\nshell='echo A > a'\n[rule.b]\ninput=['a']\noutput=['b']\nshell='echo B > b; cat a >> b'\n[rule.c]\ninput=['b']\noutput=['c']\nshell='cat b > c'\n").unwrap();
    run(&root, "c", Some(&remote), &[]);
    let db = Connection::open(root.join(".oxymake/cache/cache.db")).unwrap();
    let hash_a: String = db
        .query_row(
            "SELECT content_hash FROM output_records WHERE path='a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    fs::remove_file(root.join("a")).unwrap();
    fs::remove_file(remote.join(&hash_a[..2]).join(&hash_a)).unwrap();
    fs::write(root.join("b"), "CORRUPTED\n").unwrap();
    run(&root, "c", Some(&remote), &[]);
    assert_eq!(fs::read(root.join("b")).unwrap(), b"B\nA\n");
    assert_eq!(fs::read(root.join("c")).unwrap(), b"B\nA\n");
    let expected = ox_cache::hash_file(&root.join("b")).unwrap().to_string();
    let mut stmt = db.prepare("SELECT input_hashes_json FROM cache_entries JOIN output_records USING(cache_key) WHERE path='c'").unwrap();
    for row in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        let inputs: Vec<(String, String)> = serde_json::from_str(&row.unwrap()).unwrap();
        assert_eq!(
            inputs,
            vec![("b".into(), expected.clone())],
            "recorded a key for bytes the job never consumed"
        );
    }
    let reader = temp.path().join("reader");
    fs::create_dir(&reader).unwrap();
    fs::write(
        reader.join("Oxymakefile.toml"),
        "[rule.c]\ninput=['b']\noutput=['c']\nshell='cat b > c'\n",
    )
    .unwrap();
    fs::write(reader.join("b"), "CORRUPTED\n").unwrap();
    run(&reader, "c", Some(&remote), &[]);
    assert_eq!(
        fs::read(reader.join("c")).unwrap(),
        b"CORRUPTED\n",
        "fresh reader accepted a poisoned remote key"
    );
}
