//! Call-mode wrapper script generation.
//!
//! Generates a Python script that:
//! 1. Imports the target function (e.g., `pipeline.features:compute`)
//! 2. Deserializes each input file using auto-detected codecs
//! 3. Calls the function with deserialized arguments
//! 4. Serializes each return value to the output path
//!
//! The wrapper is executed as a subprocess via the same process-spawning
//! infrastructure used for shell mode. Everything goes through disk —
//! Arrow IPC transport is deferred to Phase 2.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use ox_codec_core::codec::FormatCodec;
use ox_codec_core::registry;
use ox_core::model::{ConcreteJob, OutputRef, ResolvedInput, ResolvedOutput};

use crate::error::ExecLocalError;

/// Input argument descriptor for the wrapper script.
struct InputArg<'a> {
    /// Variable name in the generated Python script.
    var_name: String,
    /// The file path to read from.
    file_path: &'a Path,
    /// The codec to use for deserialization.
    codec: &'static FormatCodec,
    /// Optional named argument for the function call.
    param_name: Option<&'a str>,
}

/// Output descriptor for the wrapper script.
struct OutputArg<'a> {
    /// The file path to write to.
    file_path: &'a Path,
    /// The codec to use for serialization.
    codec: &'static FormatCodec,
    /// Optional named return from the function.
    return_name: Option<&'a str>,
}

/// Generate a Python wrapper script for a call-mode job.
///
/// The generated script:
/// - Imports the target module and function
/// - Reads each input file using the appropriate codec
/// - Calls the function with positional or keyword arguments
/// - Writes each output to disk using the appropriate codec
///
/// Returns the Python source code as a string.
pub(crate) fn generate_wrapper(job: &ConcreteJob) -> Result<String, ExecLocalError> {
    let (function, _lang) = match &job.execution {
        ox_core::model::ExecutionBlock::Call { function, lang } => {
            (function.as_str(), lang.as_str())
        }
        _ => {
            return Err(ExecLocalError::UnsupportedExecution(
                "generate_wrapper called on non-Call execution block".into(),
            ));
        }
    };

    // Parse module:function syntax (e.g., "pipeline.features:compute").
    let (module_path, func_name) = parse_function_ref(function)?;

    // Resolve input codecs.
    let inputs = resolve_input_codecs(&job.inputs)?;

    // Resolve output codecs.
    let outputs = resolve_output_codecs(&job.outputs)?;

    // Collect all Python imports needed.
    let mut codecs_used: Vec<&FormatCodec> = Vec::new();
    for inp in &inputs {
        codecs_used.push(inp.codec);
    }
    for out in &outputs {
        codecs_used.push(out.codec);
    }
    let imports = registry::collect_imports(&codecs_used);

    // Generate the script.
    let mut script = String::with_capacity(2048);

    // Header.
    writeln!(script, "#!/usr/bin/env python3").unwrap();
    writeln!(
        script,
        "\"\"\"Auto-generated call-mode wrapper by OxyMake.\"\"\""
    )
    .unwrap();
    writeln!(script, "import os").unwrap();
    writeln!(script, "import sys").unwrap();
    writeln!(script, "import traceback").unwrap();
    writeln!(script, "import pathlib").unwrap();
    writeln!(script).unwrap();

    // JAX persistent compilation cache (benefits all call-mode jobs).
    writeln!(
        script,
        "_jax_cache = pathlib.Path('.oxymake') / 'jax_cache'"
    )
    .unwrap();
    writeln!(script, "_jax_cache.mkdir(parents=True, exist_ok=True)").unwrap();
    writeln!(
        script,
        "os.environ.setdefault('JAX_ENABLE_COMPILATION_CACHE', 'true')"
    )
    .unwrap();
    writeln!(
        script,
        "os.environ.setdefault('JAX_COMPILATION_CACHE_DIR', str(_jax_cache))"
    )
    .unwrap();
    writeln!(
        script,
        "os.environ.setdefault('JAX_PERSISTENT_CACHE_MIN_COMPILE_TIME_SECS', '0')"
    )
    .unwrap();
    writeln!(script).unwrap();

    // Codec imports.
    for imp in &imports {
        writeln!(script, "{imp}").unwrap();
    }
    writeln!(script).unwrap();

    // Import the target function.
    writeln!(script, "# Import the target function").unwrap();
    writeln!(script, "from {module_path} import {func_name}").unwrap();
    writeln!(script).unwrap();

    // Wrap everything in try/except for structured error reporting.
    writeln!(script, "try:").unwrap();

    // Deserialize inputs.
    writeln!(script, "    # Deserialize inputs").unwrap();
    for inp in &inputs {
        let read_expr = inp
            .codec
            .python_read
            .replace("{path}", &format!("{:?}", inp.file_path.display()));
        writeln!(script, "    {} = {}", inp.var_name, read_expr).unwrap();
    }
    writeln!(script).unwrap();

    // Call the function.
    writeln!(script, "    # Call the function").unwrap();
    let call_args = build_call_args(&inputs, &job.params);
    if outputs.len() <= 1 {
        writeln!(script, "    _result = {func_name}({call_args})").unwrap();
    } else {
        // Multiple outputs: expect a tuple or dict return.
        let return_names = build_return_names(&outputs);
        writeln!(script, "    _result = {func_name}({call_args})").unwrap();
        writeln!(script).unwrap();

        // Unpack the result.
        writeln!(script, "    # Unpack multiple return values").unwrap();
        let has_named_returns = outputs.iter().any(|o| o.return_name.is_some());
        if has_named_returns {
            // Dict-style unpacking: result["name"]
            for (i, name) in return_names.iter().enumerate() {
                writeln!(script, "    _out_{i} = _result[{name:?}]").unwrap();
            }
        } else {
            // Tuple-style unpacking.
            let vars: Vec<String> = (0..outputs.len()).map(|i| format!("_out_{i}")).collect();
            writeln!(script, "    {} = _result", vars.join(", ")).unwrap();
        }
    }
    writeln!(script).unwrap();

    // Serialize outputs.
    writeln!(script, "    # Serialize outputs").unwrap();
    if outputs.len() == 1 {
        let out = &outputs[0];
        let write_stmt = out
            .codec
            .python_write
            .replace("{obj}", "_result")
            .replace("{path}", &format!("{:?}", out.file_path.display()));
        writeln!(script, "    {write_stmt}").unwrap();
    } else {
        for (i, out) in outputs.iter().enumerate() {
            let write_stmt = out
                .codec
                .python_write
                .replace("{obj}", &format!("_out_{i}"))
                .replace("{path}", &format!("{:?}", out.file_path.display()));
            writeln!(script, "    {write_stmt}").unwrap();
        }
    }

    // Error handler with structured reporting.
    writeln!(script).unwrap();
    writeln!(script, "except Exception as _e:").unwrap();
    writeln!(
        script,
        "    print(f\"OxyMake call-mode error: {{type(_e).__name__}}: {{_e}}\", file=sys.stderr)"
    )
    .unwrap();
    writeln!(script, "    traceback.print_exc(file=sys.stderr)").unwrap();
    writeln!(script, "    sys.exit(1)").unwrap();

    Ok(script)
}

/// Parse a function reference like `"pipeline.features:compute"` into
/// `("pipeline.features", "compute")`.
fn parse_function_ref(function: &str) -> Result<(&str, &str), ExecLocalError> {
    let (module, func) = function.split_once(':').ok_or_else(|| {
        ExecLocalError::UnsupportedExecution(format!(
            "call mode function {function:?} must be in 'module:function' format \
             (e.g., 'pipeline.features:compute')"
        ))
    })?;

    if module.is_empty() || func.is_empty() {
        return Err(ExecLocalError::UnsupportedExecution(format!(
            "call mode function {function:?} has empty module or function name"
        )));
    }

    Ok((module, func))
}

/// Resolve codecs for all input files.
fn resolve_input_codecs(inputs: &[ResolvedInput]) -> Result<Vec<InputArg<'_>>, ExecLocalError> {
    let mut args = Vec::with_capacity(inputs.len());

    for (i, input) in inputs.iter().enumerate() {
        let file_path = match &input.reference {
            OutputRef::File(p) => p.as_path(),
            other => {
                return Err(ExecLocalError::UnsupportedExecution(format!(
                    "call mode Phase 1 only supports file inputs, got {other:?}"
                )));
            }
        };

        let codec = registry::lookup(file_path, input.format.as_deref()).map_err(|e| {
            ExecLocalError::UnsupportedExecution(format!(
                "cannot detect format for input {}: {e}",
                file_path.display()
            ))
        })?;

        let var_name = match &input.name {
            Some(name) => format!("_inp_{name}"),
            None => format!("_inp_{i}"),
        };

        args.push(InputArg {
            var_name,
            file_path,
            codec,
            param_name: input.name.as_deref(),
        });
    }

    Ok(args)
}

/// Resolve codecs for all output files.
fn resolve_output_codecs(outputs: &[ResolvedOutput]) -> Result<Vec<OutputArg<'_>>, ExecLocalError> {
    let mut args = Vec::with_capacity(outputs.len());

    for output in outputs {
        let file_path = match &output.reference {
            OutputRef::File(p) => p.as_path(),
            other => {
                return Err(ExecLocalError::UnsupportedExecution(format!(
                    "call mode Phase 1 only supports file outputs, got {other:?}"
                )));
            }
        };

        let codec = registry::lookup(file_path, output.format.as_deref()).map_err(|e| {
            ExecLocalError::UnsupportedExecution(format!(
                "cannot detect format for output {}: {e}",
                file_path.display()
            ))
        })?;

        args.push(OutputArg {
            file_path,
            codec,
            return_name: output.name.as_deref(),
        });
    }

    Ok(args)
}

/// Build the function call argument string.
///
/// If any input has a named parameter, use keyword arguments for all named
/// inputs and positional for unnamed ones. Params from the job configuration
/// are appended as additional keyword arguments.
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
        // Try to parse as number, otherwise pass as string.
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

/// Build the list of return names for multi-output unpacking.
fn build_return_names(outputs: &[OutputArg<'_>]) -> Vec<String> {
    outputs
        .iter()
        .enumerate()
        .map(|(i, out)| match out.return_name {
            Some(name) => name.to_string(),
            None => i.to_string(),
        })
        .collect()
}

/// Build the shell command to execute the wrapper script.
///
/// Uses `python3 <script_path>` as the base command. The caller is
/// responsible for wrapping with the appropriate environment
/// (uv run, conda run, etc.).
pub(crate) fn wrapper_command(script_path: &Path) -> String {
    format!("python3 {}", script_path.display())
}

// ---------------------------------------------------------------------------
// Warm worker support (Stage 5: fork-after-import)
// ---------------------------------------------------------------------------

/// Warm worker execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmWorkerMode {
    /// Fork-after-import: template process forks for each dispatch.
    /// State isolation by construction (child exits after each job).
    /// Best for numpy-only pipelines. JAX JIT cache is lost on fork.
    Fork,
    /// Persistent: same process handles all dispatches sequentially.
    /// JIT cache (JAX/XLA) persists across dispatches.
    /// Risk: state contamination from global variables.
    Persistent,
}

/// Generate the Python warm worker script for a set of codecs.
///
/// The script imports all required libraries, registers codec readers/writers,
/// then enters a dispatch loop. The `mode` parameter controls whether each
/// dispatch forks a child (state isolation) or runs in-process (JIT persistence).
pub(crate) fn generate_warmup_script_with_mode(
    job: &ConcreteJob,
    mode: WarmWorkerMode,
) -> Result<String, ExecLocalError> {
    let inputs = resolve_input_codecs(&job.inputs)?;
    let outputs = resolve_output_codecs(&job.outputs)?;

    let mut codecs_used: Vec<&FormatCodec> = Vec::new();
    for inp in &inputs {
        codecs_used.push(inp.codec);
    }
    for out in &outputs {
        codecs_used.push(out.codec);
    }
    let imports = registry::collect_imports(&codecs_used);

    let mut script = String::with_capacity(4096);

    // Header
    writeln!(script, "#!/usr/bin/env python3").unwrap();
    writeln!(
        script,
        "\"\"\"OxyMake warm worker — fork-after-import.\"\"\""
    )
    .unwrap();
    writeln!(script, "import importlib").unwrap();
    writeln!(script, "import json").unwrap();
    writeln!(script, "import os").unwrap();
    writeln!(script, "import signal").unwrap();
    writeln!(script, "import sys").unwrap();
    writeln!(script, "import traceback").unwrap();
    writeln!(script).unwrap();

    // JAX compilation cache: persist XLA compiled kernels between dispatches
    // and across runs. This eliminates JIT compilation overhead (~1-2s per
    // function) for repeated calls with the same input shapes.
    writeln!(script, "# JAX persistent compilation cache").unwrap();
    writeln!(script, "import pathlib").unwrap();
    writeln!(
        script,
        "_jax_cache = pathlib.Path('.oxymake') / 'jax_cache'"
    )
    .unwrap();
    writeln!(script, "_jax_cache.mkdir(parents=True, exist_ok=True)").unwrap();
    writeln!(
        script,
        "os.environ.setdefault('JAX_ENABLE_COMPILATION_CACHE', 'true')"
    )
    .unwrap();
    writeln!(
        script,
        "os.environ.setdefault('JAX_COMPILATION_CACHE_DIR', str(_jax_cache))"
    )
    .unwrap();
    writeln!(
        script,
        "os.environ.setdefault('JAX_PERSISTENT_CACHE_MIN_COMPILE_TIME_SECS', '0')"
    )
    .unwrap();
    writeln!(script).unwrap();

    // Codec imports
    writeln!(script, "# Codec imports (pre-warmed)").unwrap();
    for imp in &imports {
        writeln!(script, "{imp}").unwrap();
    }
    writeln!(script).unwrap();

    // Codec reader/writer dictionaries
    writeln!(
        script,
        "# Codec readers: codec_name -> lambda(path) -> object"
    )
    .unwrap();
    writeln!(script, "_CODEC_READERS = {{").unwrap();
    let mut seen_codecs: Vec<&str> = Vec::new();
    for inp in &inputs {
        if !seen_codecs.contains(&inp.codec.name) {
            let read_lambda = inp.codec.python_read.replace("{path}", "p");
            writeln!(
                script,
                "    {}: lambda p: {},",
                format_args!("{:?}", inp.codec.name),
                read_lambda
            )
            .unwrap();
            seen_codecs.push(inp.codec.name);
        }
    }
    writeln!(script, "}}").unwrap();
    writeln!(script).unwrap();

    writeln!(script, "# Codec writers: codec_name -> lambda(obj, path)").unwrap();
    writeln!(script, "_CODEC_WRITERS = {{").unwrap();
    seen_codecs.clear();
    for out in &outputs {
        if !seen_codecs.contains(&out.codec.name) {
            let write_lambda = out
                .codec
                .python_write
                .replace("{obj}", "o")
                .replace("{path}", "p");
            writeln!(
                script,
                "    {}: lambda o, p: {},",
                format_args!("{:?}", out.codec.name),
                write_lambda
            )
            .unwrap();
            seen_codecs.push(out.codec.name);
        }
    }
    writeln!(script, "}}").unwrap();
    writeln!(script).unwrap();

    // Ready signal
    writeln!(script, "sys.stdout.write('{{\"status\":\"ready\"}}\\n')").unwrap();
    writeln!(script, "sys.stdout.flush()").unwrap();
    writeln!(script).unwrap();

    match mode {
        WarmWorkerMode::Fork => generate_fork_dispatch_loop(&mut script),
        WarmWorkerMode::Persistent => generate_persistent_dispatch_loop(&mut script),
    }

    Ok(script)
}

/// Generate the fork-after-import dispatch loop.
fn generate_fork_dispatch_loop(script: &mut String) {
    // Dispatch loop (fork-after-import)
    writeln!(script, "for raw_line in sys.stdin:").unwrap();
    writeln!(script, "    raw_line = raw_line.strip()").unwrap();
    writeln!(script, "    if not raw_line: continue").unwrap();
    writeln!(script, "    try:").unwrap();
    writeln!(script, "        msg = json.loads(raw_line)").unwrap();
    writeln!(script, "    except json.JSONDecodeError:").unwrap();
    writeln!(
        script,
        "        sys.stdout.write('{{\"status\":\"error\",\"msg\":\"json decode\"}}\\n')"
    )
    .unwrap();
    writeln!(script, "        sys.stdout.flush()").unwrap();
    writeln!(script, "        continue").unwrap();
    writeln!(script, "    if msg.get('cmd') == 'shutdown': break").unwrap();
    writeln!(script, "    if msg.get('cmd') != 'exec':").unwrap();
    writeln!(
        script,
        "        sys.stdout.write('{{\"status\":\"error\",\"msg\":\"unknown cmd\"}}\\n')"
    )
    .unwrap();
    writeln!(script, "        sys.stdout.flush()").unwrap();
    writeln!(script, "        continue").unwrap();
    writeln!(script, "    r_fd, w_fd = os.pipe()").unwrap();
    writeln!(script, "    pid = os.fork()").unwrap();
    writeln!(script, "    if pid == 0:").unwrap();
    writeln!(script, "        os.close(r_fd)").unwrap();
    writeln!(
        script,
        "        sys.stdout = sys.stderr  # protect protocol from user prints"
    )
    .unwrap();
    writeln!(script, "        try:").unwrap();
    writeln!(
        script,
        "            mod = importlib.import_module(msg['module'])"
    )
    .unwrap();
    writeln!(script, "            func = getattr(mod, msg['function'])").unwrap();
    writeln!(script, "            kwargs = {{}}").unwrap();
    writeln!(script, "            for inp in msg.get('inputs', []):").unwrap();
    writeln!(
        script,
        "                reader = _CODEC_READERS.get(inp['codec'])"
    )
    .unwrap();
    writeln!(
        script,
        "                if reader: kwargs[inp['var']] = reader(inp['path'])"
    )
    .unwrap();
    writeln!(script, "                else:").unwrap();
    writeln!(
        script,
        "                    with open(inp['path'], 'rb') as f: kwargs[inp['var']] = f.read()"
    )
    .unwrap();
    writeln!(script, "            kwargs.update(msg.get('params', {{}}))").unwrap();
    writeln!(script, "            result = func(**kwargs)").unwrap();
    writeln!(script, "            outs = msg.get('outputs', [])").unwrap();
    writeln!(script, "            if len(outs) == 1:").unwrap();
    writeln!(
        script,
        "                w = _CODEC_WRITERS.get(outs[0]['codec'])"
    )
    .unwrap();
    writeln!(script, "                if w: w(result, outs[0]['path'])").unwrap();
    writeln!(script, "            else:").unwrap();
    writeln!(script, "                for out in outs:").unwrap();
    writeln!(script, "                    val = result[out.get('name')] if isinstance(result, dict) and out.get('name') else result").unwrap();
    writeln!(
        script,
        "                    w = _CODEC_WRITERS.get(out['codec'])"
    )
    .unwrap();
    writeln!(script, "                    if w: w(val, out['path'])").unwrap();
    writeln!(
        script,
        "            os.write(w_fd, b'{{\"status\":\"ok\"}}\\n')"
    )
    .unwrap();
    writeln!(script, "        except Exception as e:").unwrap();
    writeln!(script, "            err = json.dumps({{'status':'error','msg':f'{{type(e).__name__}}: {{e}}','traceback':traceback.format_exc()}})").unwrap();
    writeln!(script, "            os.write(w_fd, (err + '\\n').encode())").unwrap();
    writeln!(script, "        finally:").unwrap();
    writeln!(script, "            os.close(w_fd)").unwrap();
    writeln!(script, "            os._exit(0)").unwrap();
    writeln!(script, "    else:").unwrap();
    writeln!(script, "        os.close(w_fd)").unwrap();
    writeln!(script, "        try:").unwrap();
    writeln!(script, "            data = os.read(r_fd, 4_000_000)").unwrap();
    writeln!(script, "            os.close(r_fd)").unwrap();
    writeln!(script, "            os.waitpid(pid, 0)").unwrap();
    writeln!(
        script,
        "            if data: sys.stdout.write(data.decode())"
    )
    .unwrap();
    writeln!(
        script,
        "            else: sys.stdout.write('{{\"status\":\"error\",\"msg\":\"no output\"}}\\n')"
    )
    .unwrap();
    writeln!(script, "        except Exception as e:").unwrap();
    writeln!(
        script,
        "            sys.stdout.write(json.dumps({{'status':'error','msg':str(e)}}) + '\\n')"
    )
    .unwrap();
    writeln!(
        script,
        "            try: os.kill(pid, signal.SIGKILL); os.waitpid(pid, 0)"
    )
    .unwrap();
    writeln!(script, "            except ProcessLookupError: pass").unwrap();
    writeln!(script, "        sys.stdout.flush()").unwrap();
}

/// Generate the persistent dispatch loop (no fork — same process reuses JIT cache).
fn generate_persistent_dispatch_loop(script: &mut String) {
    use std::fmt::Write;

    writeln!(
        script,
        "# Persistent dispatch loop — no fork, JIT cache persists"
    )
    .unwrap();
    writeln!(script, "for raw_line in sys.stdin:").unwrap();
    writeln!(script, "    raw_line = raw_line.strip()").unwrap();
    writeln!(script, "    if not raw_line: continue").unwrap();
    writeln!(script, "    try:").unwrap();
    writeln!(script, "        msg = json.loads(raw_line)").unwrap();
    writeln!(script, "    except json.JSONDecodeError:").unwrap();
    writeln!(
        script,
        "        sys.stdout.write('{{\"status\":\"error\",\"msg\":\"json decode\"}}\\n')"
    )
    .unwrap();
    writeln!(script, "        sys.stdout.flush()").unwrap();
    writeln!(script, "        continue").unwrap();
    writeln!(script, "    if msg.get('cmd') == 'shutdown': break").unwrap();
    writeln!(script, "    if msg.get('cmd') != 'exec':").unwrap();
    writeln!(
        script,
        "        sys.stdout.write('{{\"status\":\"error\",\"msg\":\"unknown cmd\"}}\\n')"
    )
    .unwrap();
    writeln!(script, "        sys.stdout.flush()").unwrap();
    writeln!(script, "        continue").unwrap();
    writeln!(
        script,
        "    # Execute in-process (no fork) — JIT cache persists."
    )
    .unwrap();
    writeln!(script, "    _saved_stdout = sys.stdout").unwrap();
    writeln!(
        script,
        "    sys.stdout = sys.stderr  # protect protocol from user prints"
    )
    .unwrap();
    writeln!(script, "    try:").unwrap();
    writeln!(
        script,
        "        mod = importlib.import_module(msg['module'])"
    )
    .unwrap();
    writeln!(script, "        func = getattr(mod, msg['function'])").unwrap();
    writeln!(script, "        kwargs = {{}}").unwrap();
    writeln!(script, "        for inp in msg.get('inputs', []):").unwrap();
    writeln!(
        script,
        "            reader = _CODEC_READERS.get(inp['codec'])"
    )
    .unwrap();
    writeln!(
        script,
        "            if reader: kwargs[inp['var']] = reader(inp['path'])"
    )
    .unwrap();
    writeln!(script, "            else:").unwrap();
    writeln!(
        script,
        "                with open(inp['path'], 'rb') as f: kwargs[inp['var']] = f.read()"
    )
    .unwrap();
    writeln!(script, "        kwargs.update(msg.get('params', {{}}))").unwrap();
    writeln!(script, "        result = func(**kwargs)").unwrap();
    writeln!(script, "        outs = msg.get('outputs', [])").unwrap();
    writeln!(script, "        if len(outs) == 1:").unwrap();
    writeln!(
        script,
        "            w = _CODEC_WRITERS.get(outs[0]['codec'])"
    )
    .unwrap();
    writeln!(script, "            if w: w(result, outs[0]['path'])").unwrap();
    writeln!(script, "        else:").unwrap();
    writeln!(script, "            for out in outs:").unwrap();
    writeln!(script, "                val = result[out.get('name')] if isinstance(result, dict) and out.get('name') else result").unwrap();
    writeln!(
        script,
        "                w = _CODEC_WRITERS.get(out['codec'])"
    )
    .unwrap();
    writeln!(script, "                if w: w(val, out['path'])").unwrap();
    writeln!(script, "        sys.stdout = _saved_stdout").unwrap();
    writeln!(
        script,
        "        sys.stdout.write('{{\"status\":\"ok\"}}\\n')"
    )
    .unwrap();
    writeln!(script, "    except Exception as e:").unwrap();
    writeln!(script, "        sys.stdout = _saved_stdout").unwrap();
    writeln!(script, "        err = json.dumps({{'status':'error','msg':f'{{type(e).__name__}}: {{e}}','traceback':traceback.format_exc()}})").unwrap();
    writeln!(script, "        sys.stdout.write(err + '\\n')").unwrap();
    writeln!(script, "    sys.stdout.flush()").unwrap();
}

/// Build the JSON dispatch payload for a warm worker.
///
/// The payload contains module, function, input/output paths with codecs,
/// and params — everything the worker needs to execute the function.
pub(crate) fn build_dispatch_payload(
    job: &ConcreteJob,
) -> Result<serde_json::Value, ExecLocalError> {
    let (function, _lang) = match &job.execution {
        ox_core::model::ExecutionBlock::Call { function, lang } => {
            (function.as_str(), lang.as_str())
        }
        _ => {
            return Err(ExecLocalError::UnsupportedExecution(
                "build_dispatch_payload called on non-Call block".into(),
            ));
        }
    };

    let (module_path, func_name) = parse_function_ref(function)?;
    let inputs = resolve_input_codecs(&job.inputs)?;
    let outputs = resolve_output_codecs(&job.outputs)?;

    let input_specs: Vec<serde_json::Value> = inputs
        .iter()
        .map(|inp| {
            serde_json::json!({
                "var": inp.param_name.unwrap_or(inp.var_name.trim_start_matches("_inp_")),
                "path": inp.file_path.display().to_string(),
                "codec": inp.codec.name,
            })
        })
        .collect();

    let output_specs: Vec<serde_json::Value> = outputs
        .iter()
        .map(|out| {
            let mut o = serde_json::json!({
                "path": out.file_path.display().to_string(),
                "codec": out.codec.name,
            });
            if let Some(name) = out.return_name {
                o["name"] = serde_json::Value::String(name.to_string());
            }
            o
        })
        .collect();

    // Convert params to JSON values with type inference.
    let params: serde_json::Map<String, serde_json::Value> = job
        .params
        .iter()
        .map(|(k, v)| {
            let val = if let Ok(i) = v.parse::<i64>() {
                serde_json::Value::Number(i.into())
            } else if let Ok(f) = v.parse::<f64>() {
                serde_json::json!(f)
            } else if v == "true" {
                serde_json::Value::Bool(true)
            } else if v == "false" {
                serde_json::Value::Bool(false)
            } else {
                serde_json::Value::String(v.clone())
            };
            (k.clone(), val)
        })
        .collect();

    Ok(serde_json::json!({
        "cmd": "exec",
        "module": module_path,
        "function": func_name,
        "inputs": input_specs,
        "outputs": output_specs,
        "params": params,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// H26: Python requires positional arguments before keyword arguments.
    /// A named input listed before an anonymous one must not generate
    /// `f(name=a, b)` — that is a SyntaxError.
    #[test]
    fn call_args_positionals_before_keywords() {
        let codec = registry::lookup(Path::new("x.parquet"), None).unwrap();
        let inputs = vec![
            InputArg {
                var_name: "_inp_cfg".into(),
                file_path: Path::new("cfg.parquet"),
                codec,
                param_name: Some("cfg"),
            },
            InputArg {
                var_name: "_inp_1".into(),
                file_path: Path::new("b.parquet"),
                codec,
                param_name: None,
            },
        ];
        let args = build_call_args(&inputs, &BTreeMap::new());
        assert_eq!(args, "_inp_1, cfg=_inp_cfg");
    }

    /// H26: params are keywords and must also come after positionals.
    #[test]
    fn call_args_params_after_positionals() {
        let codec = registry::lookup(Path::new("x.parquet"), None).unwrap();
        let inputs = vec![InputArg {
            var_name: "_inp_0".into(),
            file_path: Path::new("a.parquet"),
            codec,
            param_name: None,
        }];
        let mut params = BTreeMap::new();
        params.insert("alpha".to_string(), "0.5".to_string());
        let args = build_call_args(&inputs, &params);
        assert_eq!(args, "_inp_0, alpha=0.5");
    }

    #[test]
    fn parse_function_ref_valid() {
        let (module, func) = parse_function_ref("pipeline.features:compute").unwrap();
        assert_eq!(module, "pipeline.features");
        assert_eq!(func, "compute");
    }

    #[test]
    fn parse_function_ref_simple() {
        let (module, func) = parse_function_ref("mymodule:run").unwrap();
        assert_eq!(module, "mymodule");
        assert_eq!(func, "run");
    }

    #[test]
    fn parse_function_ref_no_colon() {
        assert!(parse_function_ref("no_colon").is_err());
    }

    #[test]
    fn parse_function_ref_empty_parts() {
        assert!(parse_function_ref(":func").is_err());
        assert!(parse_function_ref("module:").is_err());
    }

    #[test]
    fn generate_wrapper_single_input_output() {
        use ox_core::model::*;
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        let job = ConcreteJob {
            id: JobId::from("test-job-1"),
            rule: RuleName::from("features"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("data/prices.parquet")),
                name: Some("prices".into()),
                format: None,
            }],
            outputs: vec![ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("features/signal.parquet")),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
                format: None,
            }],
            execution: ExecutionBlock::Call {
                function: "pipeline.features:compute".into(),
                lang: "python".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::Terminate,
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        };

        let script = generate_wrapper(&job).unwrap();

        // Check key elements.
        assert!(script.contains("from pipeline.features import compute"));
        assert!(script.contains("_inp_prices = pandas.read_parquet("));
        assert!(script.contains("_result = compute(prices=_inp_prices)"));
        assert!(script.contains(".to_parquet("));
    }

    #[test]
    fn generate_wrapper_multiple_outputs() {
        use ox_core::model::*;
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        let job = ConcreteJob {
            id: JobId::from("test-job-2"),
            rule: RuleName::from("split"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("data/combined.csv")),
                name: None,
                format: None,
            }],
            outputs: vec![
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
            execution: ExecutionBlock::Call {
                function: "pipeline.split:run".into(),
                lang: "python".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::Terminate,
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        };

        let script = generate_wrapper(&job).unwrap();

        // Should have dict-style unpacking since outputs are named.
        assert!(script.contains("from pipeline.split import run"));
        assert!(script.contains("_result[\"train\"]"));
        assert!(script.contains("_result[\"test\"]"));
    }

    #[test]
    fn generate_wrapper_json_format() {
        use ox_core::model::*;
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        let job = ConcreteJob {
            id: JobId::from("test-job-3"),
            rule: RuleName::from("config"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("config/params.json")),
                name: None,
                format: None,
            }],
            outputs: vec![ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("output/result.json")),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
                format: None,
            }],
            execution: ExecutionBlock::Call {
                function: "pipeline.config:process".into(),
                lang: "python".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::Terminate,
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        };

        let script = generate_wrapper(&job).unwrap();

        assert!(script.contains("import json"));
        assert!(script.contains("import pathlib"));
        assert!(script.contains("json.loads(pathlib.Path("));
        assert!(script.contains("pathlib.Path("));
    }

    #[test]
    fn generate_wrapper_passes_params_as_kwargs() {
        use ox_core::model::*;
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        let mut params = BTreeMap::new();
        params.insert("lookback".to_string(), "60".to_string());
        params.insert("normalize".to_string(), "true".to_string());
        params.insert("model_name".to_string(), "ridge".to_string());

        let job = ConcreteJob {
            id: JobId::from("test-job-params"),
            rule: RuleName::from("features"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("data/prices.parquet")),
                name: Some("prices".into()),
                format: None,
            }],
            outputs: vec![ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("features/signal.parquet")),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
                format: None,
            }],
            execution: ExecutionBlock::Call {
                function: "pipeline.features:compute".into(),
                lang: "python".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::Terminate,
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params,
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        };

        let script = generate_wrapper(&job).unwrap();

        // Params should be passed as keyword arguments.
        assert!(script.contains("lookback=60"));
        assert!(script.contains("normalize=True"));
        assert!(script.contains("model_name=\"ridge\""));
    }

    #[test]
    fn generate_wrapper_has_error_handling() {
        use ox_core::model::*;
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        let job = ConcreteJob {
            id: JobId::from("test-job-err"),
            rule: RuleName::from("features"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![ResolvedInput {
                reference: OutputRef::File(PathBuf::from("data/prices.parquet")),
                name: None,
                format: None,
            }],
            outputs: vec![ResolvedOutput {
                reference: OutputRef::File(PathBuf::from("output/result.parquet")),
                name: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
                format: None,
            }],
            execution: ExecutionBlock::Call {
                function: "pipeline:run".into(),
                lang: "python".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::Terminate,
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        };

        let script = generate_wrapper(&job).unwrap();

        // Should have try/except with structured error handling.
        assert!(script.contains("try:"));
        assert!(script.contains("except Exception as _e:"));
        assert!(script.contains("traceback.print_exc("));
        assert!(script.contains("sys.exit(1)"));
    }
}
