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
fn dry_run_preserves_cache_rows_and_output_metadata() {
    for missing in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(
            root.join("Oxymakefile.toml"),
            "[rule.a]\noutput=['out']\nshell='echo original > out'\n",
        )
        .unwrap();
        run(root, "out", None, &[]);
        let db = Connection::open(root.join(".oxymake/cache/cache.db")).unwrap();
        let snapshot = || {
            let mut stmt = db.prepare("SELECT cache_key, content_hash, mtime_secs, size FROM output_records ORDER BY cache_key, path").unwrap();
            stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
        };
        let before = snapshot();
        if missing {
            fs::remove_file(root.join("out")).unwrap();
        } else {
            fs::write(root.join("out"), "different bytes").unwrap();
        }
        run(root, "out", None, &["--dry-run"]);
        assert_eq!(
            snapshot(),
            before,
            "dry-run changed cache rows (missing={missing})"
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM cache_entries", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
