use assert_cmd::Command;
#[cfg(unix)]
use serde_json::json;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
#[cfg(unix)]
use wiremock::matchers::{method, path as path_match};
#[cfg(unix)]
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ox() -> Command {
    Command::cargo_bin("ox").unwrap()
}

fn workflow(dir: &Path, output: &str) {
    fs::write(
        dir.join("Oxymakefile.toml"),
        format!(
            "format_version='2'\nox_version='>=0.7.0'\n[rule.work]\noutput=['{output}']\nshell='echo ran > submitted; echo new > {output}'\n"
        ),
    )
    .unwrap();
}

fn run(dir: &Path, target: &str, executor: &str, extra: &[&str]) -> std::process::Output {
    ox().current_dir(dir)
        .args(["run", target, "--executor", executor])
        .args(extra)
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(unix)]
fn executable(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(
        &path,
        format!("#!/bin/sh\necho called >> \"$CALL_MARKER\"\n{body}\n"),
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn rejects_outside_outputs_before_dispatch_with_one_diagnostic_for_every_executor() {
    let project = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let output = outside.path().join("protected.txt");
    let declared = output.display().to_string();
    fs::write(&output, "keep me").unwrap();
    workflow(project.path(), &declared);

    let mut diagnostics = Vec::new();
    for executor in ["local", "ray", "slurm"] {
        let result = run(project.path(), &declared, executor, &["--dry-run"]);
        assert_eq!(
            result.status.code(),
            Some(1),
            "stdout={} stderr={}",
            text(&result.stdout),
            text(&result.stderr)
        );
        let stderr = text(&result.stderr);
        for required in [
            "work",
            &declared,
            &project.path().display().to_string(),
            "--set",
            "{config.key}",
        ] {
            assert!(
                stderr.contains(required),
                "missing {required:?} in {stderr:?}"
            );
        }
        assert!(!project.path().join("submitted").exists());
        assert!(!project.path().join(".oxymake/runs").exists());
        assert_eq!(fs::read_to_string(&output).unwrap(), "keep me");
        diagnostics.push(stderr.trim().to_owned());
    }
    assert!(
        diagnostics.windows(2).all(|pair| pair[0] == pair[1]),
        "{diagnostics:#?}"
    );
}

#[cfg(unix)]
#[test]
fn actual_runs_refuse_before_ray_http_or_slurm_command_dispatch() {
    let project = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let output = outside.path().join("protected.txt");
    let declared = output.display().to_string();
    fs::write(&output, "keep me").unwrap();
    workflow(project.path(), &declared);
    let bin = TempDir::new().unwrap();
    let marker = project.path().join("dispatch.marker");
    for name in ["sinfo", "sbatch", "sacct", "squeue", "scancel"] {
        executable(bin.path(), name, "exit 19");
    }
    let path = std::env::join_paths(
        std::iter::once(bin.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();

    for executor in ["local", "ray", "slurm"] {
        let mut command = ox();
        command.current_dir(project.path()).args([
            "run",
            &declared,
            "--executor",
            executor,
            "--no-cache",
        ]);
        if executor == "ray" {
            command.args(["--ray-address", "http://127.0.0.1:1"]);
        }
        let result = command
            .env("PATH", &path)
            .env("CALL_MARKER", &marker)
            .timeout(Duration::from_secs(30))
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(1),
            "{executor}: {}",
            text(&result.stderr)
        );
        let stderr = text(&result.stderr);
        for required in [
            "work",
            &declared,
            &project.path().display().to_string(),
            "--set",
            "{config.key}",
        ] {
            assert!(
                stderr.contains(required),
                "{executor}: missing {required:?} in {stderr:?}"
            );
        }
        assert!(
            !marker.exists(),
            "{executor} dispatched a scheduler command"
        );
        assert!(!project.path().join("submitted").exists());
        assert!(!project.path().join(".oxymake/runs").exists());
        assert_eq!(fs::read_to_string(&output).unwrap(), "keep me");
    }
}

#[test]
fn accepts_absolute_outputs_inside_project_for_all_executors() {
    let project = TempDir::new().unwrap();
    let project_root = fs::canonicalize(project.path()).unwrap();
    let output = project_root.join("absolute.txt");
    workflow(&project_root, &output.display().to_string());

    for executor in ["local", "ray", "slurm"] {
        let result = run(
            &project_root,
            &output.display().to_string(),
            executor,
            &["--dry-run"],
        );
        assert!(
            result.status.success(),
            "{executor}: {}",
            text(&result.stderr)
        );
    }
    let result = run(
        &project_root,
        &output.display().to_string(),
        "local",
        &["--no-cache"],
    );
    assert!(result.status.success(), "{}", text(&result.stderr));
    assert_eq!(fs::read_to_string(output).unwrap(), "new\n");
    assert!(project_root.join("submitted").exists());
}

#[test]
fn validates_unselected_jobs_and_is_independent_of_cache_mode() {
    let base = TempDir::new().unwrap();
    let project = base.path().join("project");
    let outside = base.path().join("outside");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let invalid = "../outside/invalid.txt";
    fs::write(project.join("Oxymakefile.toml"), format!(
        "format_version='2'\nox_version='>=0.7.0'\n[rule.selected]\noutput=['selected.txt']\nshell='echo selected > selected.txt'\n[rule.unselected]\ninput=['selected.txt']\noutput=['{}']\nshell='echo ran > submitted'\n[rule.final]\ninput=['{}']\noutput=['final.txt']\nshell='echo final > final.txt'\n",
        invalid, invalid
    )).unwrap();

    for args in [
        vec!["--dry-run", "--until", "selected.txt"],
        vec!["--dry-run", "--omit-from", invalid],
        vec!["--dry-run", "--until", "selected.txt", "--no-cache"],
    ] {
        let result = run(&project, "final.txt", "local", &args);
        assert_eq!(
            result.status.code(),
            Some(1),
            "args={args:?}: {}",
            text(&result.stderr)
        );
        assert!(
            text(&result.stderr).contains("unselected"),
            "{}",
            text(&result.stderr)
        );
        assert!(!project.join("selected.txt").exists());
        assert!(!project.join("submitted").exists());
        assert!(!outside.join("invalid.txt").exists());
    }
}

#[test]
fn config_roots_from_set_allow_external_output_and_relative_traversal() {
    let base = TempDir::new().unwrap();
    let base_root = fs::canonicalize(base.path()).unwrap();
    let project = base_root.join("project");
    let trusted = base_root.join("trusted");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&trusted).unwrap();

    workflow(&project, "{config.outdir}/from_set.txt");
    let result = ox()
        .current_dir(&project)
        .args([
            "run",
            trusted.join("from_set.txt").to_str().unwrap(),
            "--executor",
            "local",
            "--no-cache",
            "--set",
        ])
        .arg(format!("outdir={}", trusted.display()))
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", text(&result.stderr));
    assert_eq!(
        fs::read_to_string(trusted.join("from_set.txt")).unwrap(),
        "new\n"
    );

    let profile_output = trusted.join("from_profile.txt");
    fs::write(
        project.join("Oxymakefile.toml"),
        format!(
            "format_version='2'\nox_version='>=0.7.0'\n[profile.external]\nset={{outdir='{}'}}\n[rule.work]\noutput=['{{config.outdir}}/from_profile.txt']\nshell='echo new > {{config.outdir}}/from_profile.txt'\n",
            trusted.display()
        ),
    )
    .unwrap();
    let result = ox()
        .current_dir(&project)
        .args([
            "run",
            profile_output.to_str().unwrap(),
            "--executor",
            "local",
            "--profile",
            "external",
            "--no-cache",
        ])
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", text(&result.stderr));
    assert_eq!(fs::read_to_string(profile_output).unwrap(), "new\n");

    // A relative path may reach the same declared root through lexical `..`.
    workflow(&project, "../trusted/traversal.txt");
    let result = ox()
        .current_dir(&project)
        .args([
            "run",
            "../trusted/traversal.txt",
            "--executor",
            "local",
            "--dry-run",
            "--set",
        ])
        .arg(format!("outdir={}", trusted.display()))
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", text(&result.stderr));
}

#[test]
fn cached_output_cannot_bypass_the_root_check() {
    let project = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let output = outside.path().join("cached.txt");
    let mut source = format!(
        "format_version='2'\nox_version='>=0.7.0'\n[config]\nunused='{}'\n[rule.work]\noutput=['{}']\nshell='echo new > {{output}}'\n",
        outside.path().display(),
        output.display()
    );
    fs::write(project.path().join("Oxymakefile.toml"), &source).unwrap();
    let remote = TempDir::new().unwrap();
    let cache_args = ["--cache-remote", remote.path().to_str().unwrap()];
    let first = run(
        project.path(),
        &output.display().to_string(),
        "local",
        &cache_args,
    );
    assert!(first.status.success(), "{}", text(&first.stderr));
    assert_eq!(fs::read_to_string(&output).unwrap(), "new\n");

    let cached = run(
        project.path(),
        &output.display().to_string(),
        "local",
        &cache_args,
    );
    assert!(cached.status.success(), "{}", text(&cached.stderr));
    assert!(
        text(&cached.stdout).contains("1 of 1 job(s) up-to-date"),
        "{}",
        text(&cached.stdout)
    );

    source = source.replace(
        &format!("[config]\nunused='{}'\n", outside.path().display()),
        "",
    );
    fs::write(project.path().join("Oxymakefile.toml"), source).unwrap();
    let second = run(
        project.path(),
        &output.display().to_string(),
        "local",
        &cache_args,
    );
    assert_eq!(second.status.code(), Some(1), "{}", text(&second.stderr));
    assert!(text(&second.stderr).contains("escapes declared roots"));
    assert_eq!(fs::read_to_string(&output).unwrap(), "new\n");
    fs::remove_file(&output).unwrap();
    let second = run(
        project.path(),
        &output.display().to_string(),
        "local",
        &cache_args,
    );
    assert_eq!(second.status.code(), Some(1), "{}", text(&second.stderr));
    assert!(text(&second.stderr).contains("escapes declared roots"));
    assert!(!output.exists(), "cache restore ran before validation");
}

#[cfg(unix)]
const SYNC_RAY_STUB: &str = r#"
class _Context:
    def get_node_id(self): return 'stub-node'
def get_runtime_context(): return _Context()
def init(): pass
class _Ref:
    def __init__(self, value=None, error=None): self.value, self.error = value, error
class _Task:
    def __init__(self, fn): self.fn = fn
    def options(self, **_): return self
    def remote(self, *args):
        try: return _Ref(value=self.fn(*args))
        except Exception as e: return _Ref(error=e)
def remote(fn): return _Task(fn)
def get(ref):
    if ref.error is not None: raise ref.error
    return ref.value
"#;

#[cfg(unix)]
#[tokio::test]
async fn ray_driver_stub_writes_an_absolute_project_output() {
    let project = TempDir::new().unwrap();
    let project_root = fs::canonicalize(project.path()).unwrap();
    let output = project_root.join("ray-output.txt");
    workflow(&project_root, &output.display().to_string());

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_match("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ray_version":"2.40.0"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_match("/api/jobs/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"submission_id":"driver"})))
        .mount(&server)
        .await;

    let submitted = ox()
        .current_dir(&project_root)
        .args([
            "run",
            output.to_str().unwrap(),
            "--executor",
            "ray",
            "--ray-allow-pending",
            "--no-cache",
            "--ray-address",
            &server.uri(),
        ])
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert!(submitted.status.success(), "{}", text(&submitted.stderr));

    let run_dirs = project_root.join(".oxymake/runs");
    let driver = fs::read_dir(&run_dirs)
        .unwrap()
        .map(|entry| entry.unwrap().path().join("oxymake_dag_driver.py"))
        .find(|path| path.is_file())
        .expect("Ray submission should generate the driver");
    let stub = TempDir::new().unwrap();
    fs::write(stub.path().join("ray.py"), SYNC_RAY_STUB).unwrap();
    let execution = Command::new("python3")
        .arg(driver)
        .current_dir(&project_root)
        .env("PYTHONPATH", stub.path())
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert!(execution.status.success(), "{}", text(&execution.stderr));
    assert_eq!(fs::read_to_string(output).unwrap(), "new\n");
}
