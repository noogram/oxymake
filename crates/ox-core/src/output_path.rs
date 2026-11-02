//! Lexical output-path policy shared by every executor (ADR-021).
//!
//! Library callers must validate the complete resolved graph before cache
//! restoration or dispatch. This catches declaration mistakes; commands are
//! executable scripts, so this is not a sandbox.

use std::path::{Component, Path, PathBuf};

use crate::job_graph::JobGraph;
use crate::model::{JobId, JobNode, OutputRef};

/// A declared file output lies outside every declared root.
#[derive(Debug, thiserror::Error)]
#[error(
    "output path escapes declared roots for job '{job}': declared '{path}' resolves to '{normalized}' (project root '{project_root}'); declare the directory as an absolute [config] value (or pass it with --set key=/absolute/dir) and write the output as {{config.key}}/…"
)]
pub struct OutputPathError {
    /// Job declaring the output.
    pub job: JobId,
    /// Path as declared by the resolved job.
    pub path: PathBuf,
    /// Absolute, lexically normalized output path.
    pub normalized: PathBuf,
    /// Project root used for relative paths.
    pub project_root: PathBuf,
}

/// Resolve a file output and require it to lie under a declared root.
///
/// `project_root` must be absolute. Additional roots are the absolute scalar
/// values of the resolved configuration, after profiles and overrides. No
/// filesystem access occurs, and neither outputs nor roots need to exist.
///
/// # Errors
/// Returns [`OutputPathError`] when the normalized output is outside all roots.
pub fn validate_output_path(
    job: &JobId,
    path: &Path,
    project_root: &Path,
    trusted_dirs: &[PathBuf],
) -> Result<PathBuf, OutputPathError> {
    let normalized = lexical_normalize(&project_root.join(path));
    if std::iter::once(project_root)
        .chain(
            trusted_dirs
                .iter()
                .map(PathBuf::as_path)
                .filter(|p| p.is_absolute()),
        )
        .any(|root| normalized.starts_with(lexical_normalize(root)))
    {
        Ok(normalized)
    } else {
        Err(OutputPathError {
            job: job.clone(),
            path: path.to_path_buf(),
            normalized,
            project_root: project_root.to_path_buf(),
        })
    }
}

/// Check every file output in the resolved graph, including skipped jobs.
///
/// Call before selection, cache restoration, dry-run reporting or dispatch.
/// Non-file outputs are outside this policy. No filesystem access occurs.
///
/// # Errors
/// Returns the first [`OutputPathError`] in graph insertion order.
pub fn validate_output_paths(
    graph: &JobGraph,
    project_root: &Path,
    trusted_dirs: &[PathBuf],
) -> Result<(), OutputPathError> {
    for node in graph.inner().node_weights() {
        if let JobNode::Job(job) = node {
            for output in &job.outputs {
                if let OutputRef::File(path) = &output.reference {
                    validate_output_path(&job.id, path, project_root, trusted_dirs)?;
                }
            }
        }
    }
    Ok(())
}

/// Normalize without accessing the filesystem. Parents above `/` stay at `/`.
pub(crate) fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some_and(|name| name != "..") {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_same_roots_for_relative_and_absolute_paths_without_io() {
        let root = Path::new("/project/./work/..");
        let trusted = vec![PathBuf::from("/results/sub/..")];
        for (path, expected) in [
            ("nested/../out", "/project/out"),
            ("/project/out", "/project/out"),
            ("données/sub dir/../out put", "/project/données/out put"),
            ("../../results/new/out", "/results/new/out"),
            ("/results/new/out", "/results/new/out"),
        ] {
            assert_eq!(
                validate_output_path(&JobId::from("job"), Path::new(path), root, &trusted).unwrap(),
                PathBuf::from(expected)
            );
        }
    }

    #[test]
    fn rejects_traversal_and_component_prefixes_with_remedy() {
        let root = Path::new("/project");
        for path in ["../outside/out", "/project-other/out", "/results/../out"] {
            let err = validate_output_path(
                &JobId::from("figure"),
                Path::new(path),
                root,
                &[PathBuf::from("/results")],
            )
            .unwrap_err();
            assert_eq!(err.job, JobId::from("figure"));
            assert_eq!(err.path, PathBuf::from(path));
            assert_eq!(err.project_root, root);
            let message = err.to_string();
            for part in [
                "figure",
                path,
                err.normalized.to_str().unwrap(),
                "/project",
                "absolute [config]",
                "--set",
                "{config.key}/",
            ] {
                assert!(message.contains(part), "{message}");
            }
        }
    }

    #[test]
    fn root_directory_and_parents_above_root_follow_lexical_semantics() {
        assert_eq!(
            lexical_normalize(Path::new("/../../project/out")),
            PathBuf::from("/project/out")
        );
        assert_eq!(
            lexical_normalize(Path::new("../../a/../out")),
            PathBuf::from("../../out")
        );
        assert!(
            validate_output_path(
                &JobId::from("job"),
                Path::new("/anywhere/out"),
                Path::new("/project"),
                &[PathBuf::from("/")]
            )
            .is_ok()
        );
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;
    use crate::model::*;
    use std::collections::BTreeMap;

    fn job(outputs: Vec<OutputRef>) -> ConcreteJob {
        ConcreteJob {
            id: JobId::from("job"),
            rule: RuleName("work".into()),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![],
            outputs: outputs
                .into_iter()
                .map(|reference| ResolvedOutput {
                    reference,
                    name: None,
                    format: None,
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                })
                .collect(),
            execution: ExecutionBlock::Shell {
                command: "true".into(),
            },
            resources: BTreeMap::new(),
            env: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::Terminate,
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: vec![],
            log: LogConfig::default(),
            shell_executable: None,
            clean_outputs: Default::default(),
            platform_scope: Default::default(),
            reproducibility: Default::default(),
        }
    }

    #[test]
    fn graph_check_includes_outputs_of_skipped_jobs() {
        let mut graph =
            JobGraph::build(vec![job(vec![OutputRef::File("../outside/out".into())])]).unwrap();
        graph.mark_skipped(&JobId::from("job"));
        assert_eq!(graph.job_count(), 0);
        let error = validate_output_paths(&graph, Path::new("/project"), &[]).unwrap_err();
        assert_eq!(error.normalized, PathBuf::from("/outside/out"));
    }

    #[test]
    fn graph_check_ignores_non_file_outputs() {
        let graph = JobGraph::build(vec![job(vec![
            OutputRef::Virtual {
                id: "token".into(),
                check: "true".into(),
            },
            OutputRef::InMemory { type_hint: None },
        ])])
        .unwrap();
        validate_output_paths(&graph, Path::new("/project"), &[]).unwrap();
    }
}
