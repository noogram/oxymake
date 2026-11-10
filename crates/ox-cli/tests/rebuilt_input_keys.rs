//! Cache records must describe the inputs consumed after an upstream rebuild.
use assert_cmd::Command;
use serde_json::Value;
use std::{fs, path::Path, time::Duration};

const WORKFLOW: &str = r#"ox_version = "0.1"
[config]
greeting = "hello"
[rule.first]
input = ["input"]
output = ["first"]
shell = "echo {config.greeting} > first; cat input >> first"
[rule.second]
input = ["first"]
output = ["second"]
shell = "cat first > second"
"#;

fn checkout(root: &Path) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("Oxymakefile.toml"), WORKFLOW).unwrap();
    fs::write(root.join("input"), "data\n").unwrap();
}

fn run(root: &Path, remote: Option<&Path>, greeting: &str) {
    let mut cmd = Command::cargo_bin("ox").unwrap();
    cmd.current_dir(root)
        .args(["run", "second", "--set"])
        .arg(format!("greeting={greeting}"))
        .timeout(Duration::from_secs(30));
    if let Some(remote) = remote {
        cmd.arg("--cache-remote").arg(remote);
    }
    cmd.assert().success();
    let expected = format!("{greeting}\ndata\n");
    for name in ["first", "second"] {
        assert_eq!(fs::read(root.join(name)).unwrap(), expected.as_bytes());
    }
}

fn consumer_records(root: &Path) -> Vec<(String, Value, String)> {
    let db = rusqlite::Connection::open(root.join(".oxymake/cache/cache.db")).unwrap();
    let mut query = db
        .prepare(
            "SELECT e.cache_key, e.input_hashes_json, o.content_hash
             FROM cache_entries e JOIN output_records o USING (cache_key)
             WHERE o.path = 'second' ORDER BY e.cache_key",
        )
        .unwrap();
    query
        .query_map([], |row| {
            let provenance: String = row.get(1)?;
            Ok((
                row.get(0)?,
                serde_json::from_str(&provenance).unwrap(),
                row.get(2)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn rebuilt_input_keys(remote_enabled: bool) {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("writer");
    let reference = temp.path().join("reference");
    let remote_dir = temp.path().join("remote");
    let remote = remote_enabled.then_some(remote_dir.as_path());
    checkout(&a);
    checkout(&reference);
    run(&a, remote, "hello");
    let initial = consumer_records(&a);
    assert_eq!(initial.len(), 1);
    run(&a, remote, "bye");
    // A cold execution has no pre-rebuild input memo. Its key must agree with
    // the rebuilt execution, and provenance is checked against literal bytes.
    run(&reference, None, "bye");
    let expected = consumer_records(&reference);
    assert_eq!(expected.len(), 1);
    let input_hash = blake3::hash(b"bye\ndata\n").to_hex().to_string();
    assert_eq!(expected[0].1, serde_json::json!([["first", input_hash]]));
    assert_eq!(expected[0].2, input_hash);
    let rebuilt = consumer_records(&a);
    assert!(
        rebuilt.contains(&expected[0]),
        "rebuilt: {rebuilt:?}; expected: {expected:?}"
    );
    assert!(
        rebuilt.contains(&initial[0]),
        "old computation was overwritten"
    );
    // Revisit the original computation in the same checkout. Remote hits may
    // rewrite intermediates here too, and must refresh the input memo.
    run(&a, remote, "hello");
    assert!(consumer_records(&a).contains(&initial[0]));

    if let Some(remote) = remote {
        for record in [&initial[0], &expected[0]] {
            let key = &record.0;
            let path = remote
                .join("entries")
                .join(&key[..2])
                .join(format!("{key}.json"));
            let entry: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert_eq!(entry["cache_key"], *key);
            assert_eq!(entry["provenance"]["input_hashes"], record.1);
            assert_eq!(entry["outputs"][0]["content_hash"], record.2);
        }
        for greeting in ["hello", "bye"] {
            let fresh = temp.path().join(format!("fresh-{greeting}"));
            checkout(&fresh);
            run(&fresh, Some(remote), greeting);
            assert_eq!(consumer_records(&fresh).len(), 1);
        }
    }
}

#[test]
fn local_record_uses_rebuilt_input_hash() {
    rebuilt_input_keys(false);
}

#[test]
fn remote_record_uses_rebuilt_input_hash_and_restores_coherent_chain() {
    rebuilt_input_keys(true);
}
