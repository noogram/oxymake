//! Call-mode wrapper script generation for Ray.
//!
//! Generates a Python script that runs inside a Ray job and:
//! 1. Retrieves `InMemory` inputs from the Ray object store via `ray.get()`
//! 2. Reads `File` inputs from disk using auto-detected codecs
//! 3. Imports and calls the target function
//! 4. Based on `MaterializePolicy`, puts outputs in the object store via `ray.put()`
//!    and/or writes them to the shared filesystem
//! 5. Writes an object manifest JSON file for downstream job coordination

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use ox_codec_core::codec::FormatCodec;
use ox_codec_core::registry;
use ox_core::model::{ConcreteJob, ExecutionBlock, OutputRef};

use crate::error::RayError;
use crate::object_store::{self, ObjectStoreStrategy};

/// Input descriptor for the Ray call-mode wrapper.
struct InputArg<'a> {
    /// Variable name in the generated Python script.
    var_name: String,
    /// Source: either a file path or an object ref env var.
    source: InputSource<'a>,
    /// Optional named argument for the function call.
    param_name: Option<&'a str>,
}

enum InputSource<'a> {
    /// Read from disk via codec.
    File {
        file_path: &'a Path,
        codec: &'static FormatCodec,
    },
    /// Retrieve from Ray object store.
    ObjectStore {
        /// Environment variable containing the hex-encoded object ref.
        env_var: String,
    },
}

/// Output descriptor for the Ray call-mode wrapper.
struct OutputArg<'a> {
    /// Output index (used for manifest keys).
    index: usize,
    /// Output name (used for manifest keys if present).
    name: Option<&'a str>,
    /// Object store strategy from MaterializePolicy.
    strategy: ObjectStoreStrategy,
    /// File path for disk writes (if applicable).
    file_path: Option<&'a Path>,
    /// Codec for file writes (if applicable).
    codec: Option<&'static FormatCodec>,
}

/// Generate a Python wrapper script for a call-mode Ray job.
///
/// The generated script handles:
/// - `ray.init()` to connect to the running Ray runtime
/// - Reading inputs from object store (`ray.get()`) or disk
/// - Calling the target function
/// - Putting outputs in the object store (`ray.put()`)
/// - Optionally writing outputs to shared FS based on MaterializePolicy
/// - Writing an object manifest for downstream job coordination
pub fn generate_wrapper(job: &ConcreteJob) -> Result<String, RayError> {
    let (function, _lang) = match &job.execution {
        ExecutionBlock::Call { function, lang } => (function.as_str(), lang.as_str()),
        _ => {
            return Err(RayError::CallModeError(
                "generate_wrapper called on non-Call execution block".into(),
            ));
        }
    };

    let (module_path, func_name) = parse_function_ref(function)?;
    let inputs = resolve_inputs(&job.inputs)?;
    let outputs = resolve_outputs(&job.outputs)?;

    // Collect codec imports needed for file I/O.
    let mut codecs_used: Vec<&FormatCodec> = Vec::new();
    for inp in &inputs {
        if let InputSource::File { codec, .. } = &inp.source {
            codecs_used.push(codec);
        }
    }
    for out in &outputs {
        if let Some(codec) = out.codec {
            codecs_used.push(codec);
        }
    }
    let codec_imports = registry::collect_imports(&codecs_used);

    let mut script = String::with_capacity(4096);

    // Header.
    writeln!(script, "#!/usr/bin/env python3").unwrap();
    writeln!(
        script,
        "\"\"\"Auto-generated Ray call-mode wrapper by OxyMake.\"\"\""
    )
    .unwrap();
    writeln!(script, "import json").unwrap();
    writeln!(script, "import os").unwrap();
    writeln!(script, "import sys").unwrap();
    writeln!(script, "import ray").unwrap();
    writeln!(script).unwrap();

    // Codec imports.
    for imp in &codec_imports {
        writeln!(script, "{imp}").unwrap();
    }
    if !codec_imports.is_empty() {
        writeln!(script).unwrap();
    }

    // Initialize Ray (connects to the existing cluster runtime).
    writeln!(script, "# Connect to the Ray runtime").unwrap();
    writeln!(script, "ray.init()").unwrap();
    writeln!(script).unwrap();

    // Import the target function.
    writeln!(script, "# Import the target function").unwrap();
    writeln!(script, "from {module_path} import {func_name}").unwrap();
    writeln!(script).unwrap();

    // Deserialize / retrieve inputs.
    writeln!(script, "# Load inputs").unwrap();
    for inp in &inputs {
        match &inp.source {
            InputSource::File { file_path, codec } => {
                let read_expr = codec
                    .python_read
                    .replace("{path}", &format!("{:?}", file_path.display()));
                writeln!(script, "{} = {}", inp.var_name, read_expr).unwrap();
            }
            InputSource::ObjectStore { env_var } => {
                writeln!(
                    script,
                    "{var} = ray.get(ray.ObjectRef(bytes.fromhex(os.environ[\"{env_var}\"])))",
                    var = inp.var_name,
                )
                .unwrap();
            }
        }
    }
    writeln!(script).unwrap();

    // Call the function.
    writeln!(script, "# Call the function").unwrap();
    let call_args = build_call_args(&inputs, &job.params);
    if outputs.len() <= 1 {
        writeln!(script, "_result = {func_name}({call_args})").unwrap();
    } else {
        writeln!(script, "_result = {func_name}({call_args})").unwrap();
        writeln!(script).unwrap();
        writeln!(script, "# Unpack multiple return values").unwrap();
        let has_named = outputs.iter().any(|o| o.name.is_some());
        if has_named {
            for (i, out) in outputs.iter().enumerate() {
                let key = match out.name {
                    Some(n) => n.to_string(),
                    None => i.to_string(),
                };
                writeln!(script, "_out_{i} = _result[\"{key}\"]").unwrap();
            }
        } else {
            let vars: Vec<String> = (0..outputs.len()).map(|i| format!("_out_{i}")).collect();
            writeln!(script, "{} = _result", vars.join(", ")).unwrap();
        }
    }
    writeln!(script).unwrap();

    // Put outputs in object store and/or write to disk.
    writeln!(script, "# Store outputs").unwrap();
    writeln!(script, "_manifest = {{}}").unwrap();
    writeln!(script).unwrap();

    for (i, out) in outputs.iter().enumerate() {
        let result_var = if outputs.len() == 1 {
            "_result".to_string()
        } else {
            format!("_out_{i}")
        };

        let manifest_key = out
            .name
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("output_{}", out.index));

        if out.strategy.put_in_object_store {
            writeln!(script, "_ref_{i} = ray.put({result_var})",).unwrap();
            writeln!(script, "_manifest[\"{manifest_key}\"] = {{").unwrap();
            writeln!(script, "    \"object_ref_hex\": _ref_{i}.hex(),").unwrap();
            // Add type hint from the result.
            writeln!(script, "    \"type_hint\": type({result_var}).__name__").unwrap();
            writeln!(script, "}}").unwrap();
        }

        if out.strategy.write_to_shared_fs {
            if let (Some(file_path), Some(codec)) = (out.file_path, out.codec) {
                let write_stmt = codec
                    .python_write
                    .replace("{obj}", &result_var)
                    .replace("{path}", &format!("{:?}", file_path.display()));
                writeln!(script, "{write_stmt}").unwrap();
            }
        }
        writeln!(script).unwrap();
    }

    // Write the object manifest.
    writeln!(
        script,
        "# Write object manifest for downstream coordination"
    )
    .unwrap();
    writeln!(
        script,
        "_manifest_path = os.path.join(os.environ.get(\"OXYMAKE_WORKSPACE\", \".\"), \"{}\")",
        object_store::MANIFEST_FILENAME
    )
    .unwrap();
    writeln!(script, "with open(_manifest_path, \"w\") as _f:").unwrap();
    writeln!(script, "    json.dump(_manifest, _f)").unwrap();
    writeln!(script).unwrap();
    writeln!(
        script,
        "print(\"OxyMake: call-mode job completed successfully\")"
    )
    .unwrap();

    Ok(script)
}

/// Parse a function reference like `"pipeline.features:compute"`.
fn parse_function_ref(function: &str) -> Result<(&str, &str), RayError> {
    let (module, func) = function.split_once(':').ok_or_else(|| {
        RayError::CallModeError(format!(
            "call mode function {function:?} must be in 'module:function' format"
        ))
    })?;
    if module.is_empty() || func.is_empty() {
        return Err(RayError::CallModeError(format!(
            "call mode function {function:?} has empty module or function name"
        )));
    }
    Ok((module, func))
}

/// Resolve inputs to their wrapper-script descriptors.
fn resolve_inputs(inputs: &[ox_core::model::ResolvedInput]) -> Result<Vec<InputArg<'_>>, RayError> {
    let mut args = Vec::with_capacity(inputs.len());

    for (i, input) in inputs.iter().enumerate() {
        let var_name = match &input.name {
            Some(name) => format!("_inp_{name}"),
            None => format!("_inp_{i}"),
        };

        let source = match &input.reference {
            OutputRef::File(p) => {
                let codec =
                    registry::lookup(p.as_path(), input.format.as_deref()).map_err(|e| {
                        RayError::CallModeError(format!(
                            "cannot detect format for input {}: {e}",
                            p.display()
                        ))
                    })?;
                InputSource::File {
                    file_path: p.as_path(),
                    codec,
                }
            }
            OutputRef::InMemory { .. } => {
                let env_var = format!("OXYMAKE_OBJREF_{}", i);
                InputSource::ObjectStore { env_var }
            }
            OutputRef::Virtual { id, .. } => {
                return Err(RayError::CallModeError(format!(
                    "virtual inputs are not supported in Ray call mode: {id}"
                )));
            }
        };

        args.push(InputArg {
            var_name,
            source,
            param_name: input.name.as_deref(),
        });
    }

    Ok(args)
}

/// Resolve outputs to their wrapper-script descriptors.
fn resolve_outputs(
    outputs: &[ox_core::model::ResolvedOutput],
) -> Result<Vec<OutputArg<'_>>, RayError> {
    let mut args = Vec::with_capacity(outputs.len());

    for (i, output) in outputs.iter().enumerate() {
        // Determine object store strategy from MaterializePolicy.
        // Note: is_dag_leaf is not known at wrapper generation time;
        // the scheduler resolves Final→leaf at dispatch time and sets
        // the policy accordingly. We treat Final as non-leaf here;
        // the scheduler overrides to Always for actual leaves.
        let strategy = object_store::strategy_for_policy(output.materialize, false);

        let (file_path, codec) = match &output.reference {
            OutputRef::File(p) => {
                let codec =
                    registry::lookup(p.as_path(), output.format.as_deref()).map_err(|e| {
                        RayError::CallModeError(format!(
                            "cannot detect format for output {}: {e}",
                            p.display()
                        ))
                    })?;
                (Some(p.as_path()), Some(codec))
            }
            OutputRef::InMemory { .. } => (None, None),
            OutputRef::Virtual { id, .. } => {
                return Err(RayError::CallModeError(format!(
                    "virtual outputs are not supported in Ray call mode: {id}"
                )));
            }
        };

        args.push(OutputArg {
            index: i,
            name: output.name.as_deref(),
            strategy,
            file_path,
            codec,
        });
    }

    Ok(args)
}

/// Build the function call argument string.
///
/// Params from the job configuration are appended as additional keyword arguments.
fn build_call_args(inputs: &[InputArg<'_>], params: &BTreeMap<String, String>) -> String {
    // Python requires positional arguments before keyword arguments:
    // emitting inputs in declaration order would generate `f(name=a, b)`
    // (a SyntaxError) whenever a named input precedes an anonymous one.
    let mut parts: Vec<String> = inputs
        .iter()
        .filter(|inp| inp.param_name.is_none())
        .map(|inp| inp.var_name.clone())
        .collect();
    parts.extend(inputs.iter().filter_map(|inp| {
        inp.param_name
            .map(|name| format!("{name}={}", inp.var_name))
    }));

    // Append params as keyword arguments.
    for (key, value) in params {
        if value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok() {
            parts.push(format!("{key}={value}"));
        } else if value == "true" || value == "false" {
            let py_bool = if value == "true" { "True" } else { "False" };
            parts.push(format!("{key}={py_bool}"));
        } else {
            parts.push(format!("{key}={value:?}"));
        }
    }

    parts.join(", ")
}

/// Build the shell command to execute the call-mode wrapper script.
pub fn wrapper_command(script_path: &Path) -> String {
    format!("python3 {}", script_path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::model::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// H26: Python requires positional arguments before keyword arguments.
    /// A named input listed before an anonymous one must not generate
    /// `f(name=a, b)` — that is a SyntaxError.
    #[test]
    fn call_args_positionals_before_keywords() {
        let codec = registry::lookup(Path::new("x.parquet"), None).unwrap();
        let inputs = vec![
            InputArg {
                var_name: "_inp_cfg".into(),
                source: InputSource::File {
                    file_path: Path::new("cfg.parquet"),
                    codec,
                },
                param_name: Some("cfg"),
            },
            InputArg {
                var_name: "_inp_1".into(),
                source: InputSource::ObjectStore {
                    env_var: "OX_OBJ_1".into(),
                },
                param_name: None,
            },
        ];
        let args = build_call_args(&inputs, &BTreeMap::new());
        assert_eq!(args, "_inp_1, cfg=_inp_cfg");
    }

    fn call_job(
        function: &str,
        inputs: Vec<ResolvedInput>,
        outputs: Vec<ResolvedOutput>,
    ) -> ConcreteJob {
        ConcreteJob {
            id: JobId::from("test-call-1"),
            rule: RuleName::from("test-rule"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs,
            outputs,
            execution: ExecutionBlock::Call {
                function: function.to_string(),
                lang: "python".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::default(),
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        }
    }

    #[test]
    fn test_generate_wrapper_file_io() {
        let job = call_job(
            "pipeline.features:compute",
            vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("data/prices.parquet")),
                name: Some("prices".into()),
                format: None,
            }],
            vec![ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("features/signal.parquet")),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
                format: None,
            }],
        );

        let script = generate_wrapper(&job).unwrap();
        assert!(script.contains("import ray"));
        assert!(script.contains("ray.init()"));
        assert!(script.contains("from pipeline.features import compute"));
        assert!(script.contains("_inp_prices"));
        assert!(script.contains("ray.put(_result)"));
        assert!(script.contains("object_manifest.json"));
        // Always policy: should also write to shared FS.
        assert!(script.contains(".to_parquet("));
    }

    #[test]
    fn test_generate_wrapper_in_memory_input() {
        let job = call_job(
            "pipeline.transform:run",
            vec![ResolvedInput {
                reference: OutputRef::InMemory {
                    type_hint: Some("DataFrame".into()),
                },
                name: Some("data".into()),
                format: None,
            }],
            vec![ResolvedOutput {
                reference: OutputRef::InMemory {
                    type_hint: Some("DataFrame".into()),
                },
                name: None,
                lifecycle: OutputLifecycle::Temporary,
                materialize: MaterializePolicy::Never,
                format: None,
            }],
        );

        let script = generate_wrapper(&job).unwrap();
        assert!(script.contains("ray.get(ray.ObjectRef(bytes.fromhex("));
        assert!(script.contains("OXYMAKE_OBJREF_0"));
        assert!(script.contains("ray.put(_result)"));
        // Never policy: should NOT write to shared FS.
        assert!(!script.contains(".to_parquet("));
    }

    #[test]
    fn test_generate_wrapper_auto_policy() {
        let job = call_job(
            "pipeline.features:compute",
            vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("data/input.parquet")),
                name: None,
                format: None,
            }],
            vec![ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("output/result.parquet")),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Auto,
                format: None,
            }],
        );

        let script = generate_wrapper(&job).unwrap();
        // Auto policy: object store only, no disk write.
        assert!(script.contains("ray.put(_result)"));
        assert!(!script.contains(".to_parquet("));
    }

    #[test]
    fn test_generate_wrapper_multiple_outputs() {
        let job = call_job(
            "pipeline.split:run",
            vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("data/combined.csv")),
                name: None,
                format: None,
            }],
            vec![
                ResolvedOutput {
                    reference: OutputRef::File(PathBuf::from("output/train.csv")),
                    name: Some("train".into()),
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                    format: None,
                },
                ResolvedOutput {
                    reference: OutputRef::File(PathBuf::from("output/test.csv")),
                    name: Some("test".into()),
                    lifecycle: OutputLifecycle::Permanent,
                    materialize: MaterializePolicy::Always,
                    format: None,
                },
            ],
        );

        let script = generate_wrapper(&job).unwrap();
        assert!(script.contains("_result[\"train\"]"));
        assert!(script.contains("_result[\"test\"]"));
        assert!(script.contains("ray.put(_out_0)"));
        assert!(script.contains("ray.put(_out_1)"));
    }

    #[test]
    fn test_parse_function_ref() {
        let (m, f) = parse_function_ref("pipeline.features:compute").unwrap();
        assert_eq!(m, "pipeline.features");
        assert_eq!(f, "compute");
    }

    #[test]
    fn test_parse_function_ref_invalid() {
        assert!(parse_function_ref("no_colon").is_err());
        assert!(parse_function_ref(":func").is_err());
        assert!(parse_function_ref("module:").is_err());
    }

    #[test]
    fn test_non_call_block_rejected() {
        let mut job = call_job("mod:func", vec![], vec![]);
        job.execution = ExecutionBlock::Shell {
            command: "echo hi".into(),
        };
        assert!(generate_wrapper(&job).is_err());
    }
}
