//! Parse WDL (Workflow Description Language) files into the translation IR.
//!
//! Supports WDL 1.0 and 1.1 syntax. Handles `task`, `workflow`, `call`,
//! `scatter`, `runtime`, and `meta` blocks. Features that have no OxyMake
//! equivalent (e.g., `if` conditionals, `struct` types) produce escalations.

use regex::Regex;

use crate::ir::*;

/// Parse a WDL file into the translation IR.
pub fn parse_wdl(content: &str) -> anyhow::Result<WorkflowIR> {
    let mut rules = Vec::new();
    let mut diagnostics = Vec::new();
    let mut escalations = Vec::new();
    let mut config_values = Vec::new();

    // Extract version
    let version = extract_version(content);
    if let Some(ref v) = version {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Info,
            message: format!("WDL version: {v}"),
            line: Some(1),
        });
    }

    // Parse tasks
    for task in parse_tasks(content, &mut diagnostics) {
        let rule = convert_task(&task, &mut diagnostics, &mut escalations);
        rules.push(rule);
    }

    // Parse workflow block for call ordering and scatter
    if let Some(wf) = parse_workflow_block(content, &mut diagnostics) {
        // Extract scatter patterns. A single-variable scatter is a 1:1
        // map over the array, which `expand = "zip"` preserves exactly.
        for scatter in &wf.scatters {
            if let Some(rule) = rules.iter_mut().find(|r| r.name == scatter.call_name) {
                rule.expand = Some("zip".to_string());
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Info,
                    message: format!(
                        "Task '{}': WDL scatter over '{}' mapped to expand = \"zip\" \
                         (one job per element of '{}')",
                        scatter.call_name, scatter.variable, scatter.variable
                    ),
                    line: scatter.line,
                });
            }
        }

        // Nested scatters are a cross-product (N x M jobs): no expand mode
        // over a single variable can represent that cardinality — escalate
        // instead of guessing (H33).
        for scatter in &wf.nested_scatters {
            escalations.push(Escalation::new(
                EscalationTier::Human,
                EscalationCategory::MissingFeature,
                Severity::Correctness,
                Some(&scatter.call_name),
                "nested_scatter",
                &format!(
                    "call {} under nested scatter ({})",
                    scatter.call_name, scatter.variable
                ),
                scatter.line,
                EscalationInstructions::new(
                    &format!(
                        "Task '{}' is called under a nested WDL scatter over ({}); \
                         the translation cannot preserve the cross-product cardinality",
                        scatter.call_name, scatter.variable
                    ),
                    vec![
                        "Flatten the nested scatter into a single pre-computed pair list",
                        "Or split the rule so each scatter level maps to its own rule",
                    ],
                    vec!["The translated workflow produces one job per (outer, inner) pair"],
                ),
            ));
        }

        // Extract workflow-level inputs as config values. Typed inputs
        // (Int/Boolean/Float/…) have no value yet — escalate so the
        // placeholder is resolved instead of shipping silently (H32).
        for input in &wf.inputs {
            if !input.wdl_type.starts_with("File") && !input.wdl_type.starts_with("String") {
                escalations.push(unmapped_input_escalation(
                    None,
                    &input.name,
                    &input.wdl_type,
                    None,
                ));
            }
            config_values.push(ConfigEntryIR {
                key: input.name.clone(),
                values: vec![format!("# WDL type: {}", input.wdl_type)],
            });
        }
    }

    // Parse imports
    let includes: Vec<String> = parse_imports(content)
        .into_iter()
        .map(|imp| imp.uri)
        .collect();

    Ok(WorkflowIR {
        rules,
        config_file: None,
        includes,
        diagnostics,
        config_values,
        escalations,
        global_container: None,
        global_report: None,
    })
}

// ---------------------------------------------------------------------------
// Version extraction
// ---------------------------------------------------------------------------

fn extract_version(content: &str) -> Option<String> {
    let re = Regex::new(r"(?m)^version\s+(\S+)").unwrap();
    re.captures(content).map(|c| c[1].to_string())
}

// ---------------------------------------------------------------------------
// Task parsing
// ---------------------------------------------------------------------------

struct WdlTask {
    name: String,
    inputs: Vec<WdlInput>,
    outputs: Vec<WdlOutput>,
    command: Option<String>,
    runtime: WdlRuntime,
    #[allow(dead_code)]
    meta: Vec<(String, String)>,
    line: usize,
}

struct WdlInput {
    name: String,
    wdl_type: String,
    optional: bool,
}

struct WdlOutput {
    name: String,
    #[allow(dead_code)]
    wdl_type: String,
    expression: String,
}

#[derive(Default)]
struct WdlRuntime {
    docker: Option<String>,
    cpu: Option<String>,
    memory: Option<String>,
    disks: Option<String>,
    gpu: Option<String>,
}

fn parse_tasks(content: &str, diagnostics: &mut Vec<Diagnostic>) -> Vec<WdlTask> {
    let mut tasks = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let task_re = Regex::new(r"^\s*task\s+(\w+)\s*\{").unwrap();

    let mut i = 0;
    while i < lines.len() {
        if let Some(caps) = task_re.captures(lines[i]) {
            let name = caps[1].to_string();
            let start = i;
            // Find matching closing brace
            if let Some(end) = find_matching_brace(&lines, i) {
                let task_block: Vec<&str> = lines[start..=end].to_vec();
                let task = parse_task_block(&name, &task_block, start + 1);
                tasks.push(task);
                i = end + 1;
                continue;
            }
            // Unbalanced braces: say so instead of silently dropping the
            // task — typically a stray `{`/`}` in a brace-form command
            // body (use `command <<< … >>>` for raw shell text) (H31).
            diagnostics.push(Diagnostic {
                level: DiagLevel::Error,
                message: format!(
                    "task '{name}': unbalanced braces — task skipped \
                     (raw shell text in a `command {{…}}` body? \
                     use `command <<< … >>>` instead)"
                ),
                line: Some(start + 1),
            });
        }
        i += 1;
    }

    tasks
}

fn parse_task_block(name: &str, lines: &[&str], start_line: usize) -> WdlTask {
    let inputs = parse_section(lines, "input");
    let outputs = parse_section(lines, "output");
    let command = parse_command_section(lines);
    let runtime = parse_runtime_section(lines);
    let meta = parse_meta_section(lines);

    let parsed_inputs: Vec<WdlInput> = inputs
        .iter()
        .filter_map(|line| parse_input_decl(line.trim()))
        .collect();

    let parsed_outputs: Vec<WdlOutput> = outputs
        .iter()
        .filter_map(|line| parse_output_decl(line.trim()))
        .collect();

    WdlTask {
        name: name.to_string(),
        inputs: parsed_inputs,
        outputs: parsed_outputs,
        command,
        runtime,
        meta,
        line: start_line,
    }
}

fn parse_input_decl(line: &str) -> Option<WdlInput> {
    if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
        return None;
    }
    // Match patterns like: File input_file, String sample_name, Int threads
    // Also: File? optional_file, Array[File] files
    let re = Regex::new(r"^\s*([\w\[\]?+]+)\s+(\w+)").unwrap();
    re.captures(line).map(|caps| {
        let wdl_type = caps[1].to_string();
        let name = caps[2].to_string();
        let optional = wdl_type.contains('?');
        WdlInput {
            name,
            wdl_type,
            optional,
        }
    })
}

fn parse_output_decl(line: &str) -> Option<WdlOutput> {
    if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
        return None;
    }
    // Match: Type name = expression
    let re = Regex::new(r"^\s*([\w\[\]?+]+)\s+(\w+)\s*=\s*(.+)$").unwrap();
    re.captures(line).map(|caps| WdlOutput {
        wdl_type: caps[1].to_string(),
        name: caps[2].to_string(),
        expression: caps[3].trim().to_string(),
    })
}

fn parse_section(lines: &[&str], section_name: &str) -> Vec<String> {
    let mut result = Vec::new();
    let pattern = format!(r"^\s*{}\s*\{{", section_name);
    let re = Regex::new(&pattern).unwrap();

    let mut i = 0;
    while i < lines.len() {
        if re.is_match(lines[i]) {
            if let Some(end) = find_matching_brace(lines, i) {
                for line in &lines[i + 1..end] {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        result.push(trimmed.to_string());
                    }
                }
                break;
            }
        }
        i += 1;
    }

    result
}

fn parse_command_section(lines: &[&str]) -> Option<String> {
    // WDL commands use either command { ... } or command <<< ... >>>
    let brace_re = Regex::new(r"^\s*command\s*\{").unwrap();
    let heredoc_re = Regex::new(r"^\s*command\s*<<<").unwrap();

    let mut i = 0;
    while i < lines.len() {
        if brace_re.is_match(lines[i]) {
            if let Some(end) = find_matching_brace(lines, i) {
                let cmd_lines: Vec<&str> = lines[i + 1..end]
                    .iter()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .collect();
                return Some(cmd_lines.join("\n"));
            }
        } else if heredoc_re.is_match(lines[i]) {
            // Find closing >>>
            let mut cmd_lines = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                if lines[j].trim().starts_with(">>>") {
                    break;
                }
                let trimmed = lines[j].trim();
                if !trimmed.is_empty() {
                    cmd_lines.push(trimmed);
                }
                j += 1;
            }
            return Some(cmd_lines.join("\n"));
        }
        i += 1;
    }

    None
}

fn parse_runtime_section(lines: &[&str]) -> WdlRuntime {
    let mut runtime = WdlRuntime::default();
    let section_lines = parse_section(lines, "runtime");

    for line in &section_lines {
        if let Some((key, value)) = parse_kv_line(line) {
            match key.as_str() {
                "docker" | "container" => runtime.docker = Some(unquote(&value)),
                "cpu" => runtime.cpu = Some(unquote(&value)),
                "memory" => runtime.memory = Some(unquote(&value)),
                "disks" => runtime.disks = Some(unquote(&value)),
                "gpu" | "gpuCount" | "nvidia_gpu" => runtime.gpu = Some(unquote(&value)),
                _ => {}
            }
        }
    }

    runtime
}

fn parse_meta_section(lines: &[&str]) -> Vec<(String, String)> {
    let mut meta = Vec::new();
    let section_lines = parse_section(lines, "meta");

    for line in &section_lines {
        if let Some((key, value)) = parse_kv_line(line) {
            meta.push((key, unquote(&value)));
        }
    }

    meta
}

fn parse_kv_line(line: &str) -> Option<(String, String)> {
    // Match: key: value or key = value
    let re = Regex::new(r#"^\s*(\w+)\s*[:=]\s*(.+)$"#).unwrap();
    re.captures(line).map(|caps| {
        (
            caps[1].to_string(),
            caps[2].trim().trim_end_matches(',').to_string(),
        )
    })
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Workflow block parsing
// ---------------------------------------------------------------------------

struct WdlWorkflow {
    inputs: Vec<WdlInput>,
    scatters: Vec<WdlScatter>,
    /// Calls under a *nested* scatter: cross-product cardinality that
    /// `expand = "zip"` cannot represent — escalated, never guessed (H33).
    nested_scatters: Vec<WdlScatter>,
}

struct WdlScatter {
    variable: String,
    call_name: String,
    line: Option<usize>,
}

fn parse_workflow_block(content: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<WdlWorkflow> {
    let lines: Vec<&str> = content.lines().collect();
    let wf_re = Regex::new(r"^\s*workflow\s+(\w+)\s*\{").unwrap();

    let mut i = 0;
    while i < lines.len() {
        if let Some(caps) = wf_re.captures(lines[i]) {
            if let Some(end) = find_matching_brace(&lines, i) {
                let wf_lines = &lines[i..=end];
                let inputs = parse_section(wf_lines, "input");
                let parsed_inputs: Vec<WdlInput> = inputs
                    .iter()
                    .filter_map(|line| parse_input_decl(line.trim()))
                    .collect();

                let (scatters, nested_scatters) = parse_scatters(wf_lines, i);

                return Some(WdlWorkflow {
                    inputs: parsed_inputs,
                    scatters,
                    nested_scatters,
                });
            }
            // Same unbalanced-brace hazard as tasks (H31).
            diagnostics.push(Diagnostic {
                level: DiagLevel::Error,
                message: format!(
                    "workflow '{}': unbalanced braces — workflow block skipped",
                    &caps[1]
                ),
                line: Some(i + 1),
            });
        }
        i += 1;
    }

    None
}

fn parse_scatters(lines: &[&str], base_line: usize) -> (Vec<WdlScatter>, Vec<WdlScatter>) {
    let mut scatters = Vec::new();
    let mut nested = Vec::new();
    let scatter_re = Regex::new(r"^\s*scatter\s*\(\s*(\w+)\s+in\s+").unwrap();
    let call_re = Regex::new(r"^\s*call\s+(\w+)").unwrap();

    let mut i = 0;
    while i < lines.len() {
        if let Some(caps) = scatter_re.captures(lines[i]) {
            let variable = caps[1].to_string();
            // Find call inside scatter
            if let Some(end) = find_matching_brace(lines, i) {
                let mut j = i + 1;
                while j < end {
                    if let Some(inner_caps) = scatter_re.captures(lines[j]) {
                        // Nested scatter: every call inside is a
                        // cross-product over (outer, inner) — collect for
                        // escalation instead of guessing a mode (H33).
                        let inner_end = find_matching_brace(lines, j).unwrap_or(end).min(end);
                        for line in lines.iter().take(inner_end).skip(j + 1) {
                            if let Some(call_caps) = call_re.captures(line) {
                                nested.push(WdlScatter {
                                    variable: format!("{variable} x {}", &inner_caps[1]),
                                    call_name: call_caps[1].to_string(),
                                    line: Some(base_line + j + 1),
                                });
                            }
                        }
                        j = inner_end + 1;
                        continue;
                    }
                    if let Some(call_caps) = call_re.captures(lines[j]) {
                        scatters.push(WdlScatter {
                            variable: variable.clone(),
                            call_name: call_caps[1].to_string(),
                            line: Some(base_line + i + 1),
                        });
                    }
                    j += 1;
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }

    (scatters, nested)
}

// ---------------------------------------------------------------------------
// Import parsing
// ---------------------------------------------------------------------------

struct WdlImport {
    uri: String,
}

fn parse_imports(content: &str) -> Vec<WdlImport> {
    let re = Regex::new(r#"(?m)^\s*import\s+"([^"]+)""#).unwrap();
    re.captures_iter(content)
        .map(|caps| WdlImport {
            uri: caps[1].to_string(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Task → IR conversion
// ---------------------------------------------------------------------------

fn convert_task(
    task: &WdlTask,
    diagnostics: &mut Vec<Diagnostic>,
    escalations: &mut Vec<Escalation>,
) -> RuleIR {
    // Convert inputs
    let inputs: Vec<PortIR> = task
        .inputs
        .iter()
        .filter_map(|inp| {
            // File and String types map to file patterns
            if inp.wdl_type.starts_with("File") || inp.wdl_type.starts_with("String") {
                Some(PortIR {
                    name: Some(inp.name.clone()),
                    pattern: format!("{{{}}}", inp.name),
                    lifecycle: None,
                })
            } else if inp.wdl_type.starts_with("Array[File]") {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warning,
                    message: format!(
                        "Task '{}': Array[File] input '{}' approximated as single pattern",
                        task.name, inp.name
                    ),
                    line: Some(task.line),
                });
                Some(PortIR {
                    name: Some(inp.name.clone()),
                    pattern: format!("{{{}}}", inp.name),
                    lifecycle: None,
                })
            } else {
                // Non-file types become params, not inputs
                None
            }
        })
        .collect();

    // Convert non-file inputs to params. There is no faithful OxyMake
    // representation for typed values (Int/Boolean/Float/…): the param is
    // emitted with a placeholder value and a structured escalation so the
    // gap is visible instead of silently shipping a comment string (H32).
    let params: Vec<ParamIR> = task
        .inputs
        .iter()
        .filter(|inp| {
            !inp.wdl_type.starts_with("File")
                && !inp.wdl_type.starts_with("String")
                && !inp.wdl_type.starts_with("Array[File]")
        })
        .map(|inp| {
            escalations.push(unmapped_input_escalation(
                Some(&task.name),
                &inp.name,
                &inp.wdl_type,
                Some(task.line),
            ));
            ParamIR {
                name: inp.name.clone(),
                value: format!("# WDL type: {}", inp.wdl_type),
            }
        })
        .collect();

    // Convert outputs
    let outputs: Vec<PortIR> = task
        .outputs
        .iter()
        .map(|out| {
            let pattern = wdl_expr_to_pattern(&out.expression);
            PortIR {
                name: Some(out.name.clone()),
                pattern,
                lifecycle: None,
            }
        })
        .collect();

    // Convert command
    let execution = if let Some(ref cmd) = task.command {
        let converted = convert_wdl_placeholders(cmd);
        ExecutionIR::Shell(converted)
    } else {
        ExecutionIR::None
    };

    // Convert runtime
    let threads = task.runtime.cpu.as_ref().and_then(|cpu| {
        cpu.parse::<u32>()
            .ok()
            .map(ThreadsIR::Literal)
            .or(Some(ThreadsIR::Dynamic(cpu.clone())))
    });

    let mut resources = Vec::new();
    if let Some(ref mem) = task.runtime.memory {
        let mem_value = convert_wdl_memory(mem);
        resources.push(ResourceIR {
            key: "mem".to_string(),
            value: mem_value,
        });
    }
    if let Some(ref disks) = task.runtime.disks {
        let disk_value = convert_wdl_disks(disks);
        resources.push(ResourceIR {
            key: "disk".to_string(),
            value: disk_value,
        });
    }
    if let Some(ref gpu) = task.runtime.gpu {
        resources.push(ResourceIR {
            key: "gpu".to_string(),
            value: gpu.clone(),
        });
    }

    let environment = task
        .runtime
        .docker
        .as_ref()
        .map(|img| EnvironmentIR::Container(format!("docker://{}", img)));

    // Handle optional inputs as escalations
    for inp in &task.inputs {
        if inp.optional {
            escalations.push(Escalation::new(
                EscalationTier::Assisted,
                EscalationCategory::DynamicInput,
                Severity::Correctness,
                Some(&task.name),
                "optional_input",
                &format!("{}? {}", inp.wdl_type, inp.name),
                Some(task.line),
                EscalationInstructions::new(
                    &format!(
                        "Optional WDL input '{}' needs a default or conditional guard",
                        inp.name
                    ),
                    vec![
                        "Add a default value in the [config] section",
                        "Or add a 'when' guard to the rule",
                    ],
                    vec!["Rule handles missing input gracefully"],
                ),
            ));
        }
    }

    RuleIR {
        name: task.name.clone(),
        inputs,
        outputs,
        execution,
        threads,
        resources,
        environment,
        wildcard_constraints: vec![],
        params,
        log: vec![],
        source_line: Some(task.line),
        expand: None,
    }
}

/// Escalation for a WDL input whose type has no OxyMake mapping
/// (Int, Boolean, Float, Map, …). The translation emits a placeholder
/// value; a human or agent must supply the real one (H32).
fn unmapped_input_escalation(
    rule_name: Option<&str>,
    input_name: &str,
    wdl_type: &str,
    line: Option<usize>,
) -> Escalation {
    Escalation::new(
        EscalationTier::Assisted,
        EscalationCategory::DynamicParam,
        Severity::Correctness,
        rule_name,
        "unmapped_input_type",
        &format!("{wdl_type} {input_name}"),
        line,
        EscalationInstructions::new(
            &format!(
                "WDL input '{input_name}' has type '{wdl_type}', which has no \
                 OxyMake equivalent; its translated value is a placeholder"
            ),
            vec![
                "Replace the '# WDL type: …' placeholder with a concrete value",
                "Or wire the value from the [config] section",
            ],
            vec!["No '# WDL type:' placeholder remains in the translated file"],
        ),
    )
}

/// Convert WDL `~{var}` and `${var}` placeholders to OxyMake `{var}` format.
fn convert_wdl_placeholders(cmd: &str) -> String {
    let re_tilde = Regex::new(r"~\{([^}]+)\}").unwrap();
    let re_dollar = Regex::new(r"\$\{([^}]+)\}").unwrap();

    let result = re_tilde.replace_all(cmd, "{$1}");
    let result = re_dollar.replace_all(&result, "{$1}");
    result.to_string()
}

/// Convert WDL output expressions to OxyMake file patterns.
fn wdl_expr_to_pattern(expr: &str) -> String {
    let expr = expr.trim().trim_matches('"');

    // Handle glob() expressions
    if let Some(inner) = expr.strip_prefix("glob(").and_then(|s| s.strip_suffix(')')) {
        return format!("# glob: {}", inner.trim().trim_matches('"'));
    }

    // Handle string interpolation: "prefix~{var}suffix" or "prefix${var}suffix"
    let re_tilde = Regex::new(r"~\{([^}]+)\}").unwrap();
    let re_dollar = Regex::new(r"\$\{([^}]+)\}").unwrap();

    let result = re_tilde.replace_all(expr, "{$1}");
    let result = re_dollar.replace_all(&result, "{$1}");

    result.to_string()
}

/// Convert WDL memory strings (e.g., "16 GB", "500 MiB") to OxyMake format.
fn convert_wdl_memory(mem: &str) -> String {
    let mem = mem.trim();
    let re = Regex::new(r"(\d+)\s*(GB|GiB|G|MB|MiB|M)").unwrap();
    if let Some(caps) = re.captures(mem) {
        let amount: u64 = caps[1].parse().unwrap_or(0);
        let unit = &caps[2];
        match unit {
            "GB" | "GiB" | "G" => format!("{}G", amount),
            "MB" | "MiB" | "M" => format!("{}M", amount),
            _ => mem.to_string(),
        }
    } else {
        mem.to_string()
    }
}

/// Convert WDL disks strings (e.g., "local-disk 100 HDD") to OxyMake format.
fn convert_wdl_disks(disks: &str) -> String {
    let disks = disks.trim();
    // Common WDL pattern: "local-disk SIZE TYPE"
    let re = Regex::new(r"local-disk\s+(\d+)\s+\w+").unwrap();
    if let Some(caps) = re.captures(disks) {
        let gb: u64 = caps[1].parse().unwrap_or(0);
        format!("{}G", gb)
    } else {
        disks.to_string()
    }
}

// ---------------------------------------------------------------------------
// Utility: brace matching
// ---------------------------------------------------------------------------

fn find_matching_brace(lines: &[&str], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut found_open = false;
    let mut in_heredoc = false;

    for (i, line) in lines.iter().enumerate().skip(start) {
        // Heredoc command bodies (`command <<< … >>>`) are opaque spans:
        // they hold raw shell text whose braces (`awk '{…}'`, `echo "}"`,
        // unclosed `${VAR`) must not affect block matching (H31).
        let mut rest: &str = line;
        loop {
            if in_heredoc {
                match rest.find(">>>") {
                    Some(pos) => {
                        in_heredoc = false;
                        rest = &rest[pos + 3..];
                    }
                    None => break,
                }
            } else {
                let (scan, after_opener) = match rest.find("<<<") {
                    Some(pos) => (&rest[..pos], Some(&rest[pos + 3..])),
                    None => (rest, None),
                };
                for ch in scan.chars() {
                    if ch == '{' {
                        depth += 1;
                        found_open = true;
                    } else if ch == '}' {
                        depth -= 1;
                        if found_open && depth == 0 {
                            return Some(i);
                        }
                    }
                }
                match after_opener {
                    Some(r) => {
                        in_heredoc = true;
                        rest = r;
                    }
                    None => break,
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_task() {
        let wdl = r#"
version 1.0

task hello {
    input {
        File input_file
        String name
    }
    command {
        echo "Hello ~{name}" > output.txt
        cat ~{input_file} >> output.txt
    }
    output {
        File result = "output.txt"
    }
    runtime {
        docker: "ubuntu:22.04"
        cpu: 2
        memory: "4 GB"
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        assert_eq!(ir.rules.len(), 1);

        let rule = &ir.rules[0];
        assert_eq!(rule.name, "hello");
        assert_eq!(rule.inputs.len(), 2); // File and String both map to inputs
        assert_eq!(rule.outputs.len(), 1);
        assert_eq!(rule.outputs[0].pattern, "output.txt");

        // Check command conversion
        if let ExecutionIR::Shell(ref cmd) = rule.execution {
            assert!(cmd.contains("{name}"));
            assert!(cmd.contains("{input_file}"));
            assert!(!cmd.contains("~{"));
        } else {
            panic!("expected Shell execution");
        }

        // Check runtime
        assert_eq!(rule.threads, Some(ThreadsIR::Literal(2)));
        assert!(
            rule.environment
                .as_ref()
                .is_some_and(|e| matches!(e, EnvironmentIR::Container(_)))
        );
    }

    #[test]
    fn test_parse_task_with_dollar_placeholders() {
        let wdl = r#"
version 1.0

task sort_file {
    input {
        File unsorted
    }
    command {
        sort ${unsorted} > sorted.txt
    }
    output {
        File sorted = "sorted.txt"
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        let rule = &ir.rules[0];
        if let ExecutionIR::Shell(ref cmd) = rule.execution {
            assert!(cmd.contains("{unsorted}"));
            assert!(!cmd.contains("${"));
        } else {
            panic!("expected Shell execution");
        }
    }

    #[test]
    fn test_parse_multiple_tasks() {
        let wdl = r#"
version 1.0

task step_a {
    command {
        echo "a"
    }
    output {
        File out_a = "a.txt"
    }
}

task step_b {
    input {
        File in_b
    }
    command {
        cat ~{in_b} > b.txt
    }
    output {
        File out_b = "b.txt"
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        assert_eq!(ir.rules.len(), 2);
        assert_eq!(ir.rules[0].name, "step_a");
        assert_eq!(ir.rules[1].name, "step_b");
    }

    #[test]
    fn test_parse_workflow_with_scatter() {
        let wdl = r#"
version 1.0

task process {
    input {
        File input_file
    }
    command {
        process ~{input_file}
    }
    output {
        File result = "result.txt"
    }
}

workflow my_pipeline {
    input {
        Array[File] input_files
    }
    scatter (f in input_files) {
        call process { input: input_file = f }
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        assert_eq!(ir.rules.len(), 1);

        let rule = &ir.rules[0];
        assert_eq!(rule.name, "process");
        assert_eq!(rule.expand, Some("zip".to_string()));
    }

    #[test]
    fn test_nested_scatter_escalates_instead_of_silent_zip() {
        // A nested WDL scatter is a cross-product (cardinality N×M).
        // expand = "zip" over the outer variable alone would silently
        // produce N jobs instead of N×M (H33). It must escalate, and the
        // call under the nested scatter must NOT get a guessed expand.
        let wdl = r#"
version 1.0

task pair {
    input {
        File sample
    }
    command {
        process ~{sample}
    }
    output {
        File result = "result.txt"
    }
}

workflow nested {
    input {
        Array[File] samples
        Array[String] lanes
    }
    scatter (s in samples) {
        scatter (l in lanes) {
            call pair { input: sample = s }
        }
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        let rule = &ir.rules[0];
        assert_eq!(
            rule.expand, None,
            "nested-scatter call must not get a guessed expand mode"
        );
        assert!(
            ir.escalations
                .iter()
                .any(|e| e.construct == "nested_scatter" && e.original_code.contains("pair")),
            "expected nested_scatter escalation, got: {:?}",
            ir.escalations
                .iter()
                .map(|e| &e.construct)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_single_scatter_diagnostic_names_cardinality() {
        let wdl = r#"
version 1.0

task process {
    input {
        File input_file
    }
    command {
        process ~{input_file}
    }
    output {
        File result = "result.txt"
    }
}

workflow single {
    input {
        Array[File] input_files
    }
    scatter (f in input_files) {
        call process { input: input_file = f }
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        assert_eq!(ir.rules[0].expand, Some("zip".to_string()));
        // The diagnostic must state the 1:1 (zip) cardinality mapping so
        // a reader can verify the semantics, not just the label (H33).
        assert!(
            ir.diagnostics
                .iter()
                .any(|d| d.message.contains("one job per element")),
            "diagnostic must explain scatter cardinality, got: {:?}",
            ir.diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_imports() {
        let wdl = r#"
version 1.0

import "tasks/align.wdl" as align
import "tasks/qc.wdl"

workflow pipeline {
    call align.bwa_mem
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        assert_eq!(ir.includes.len(), 2);
        assert_eq!(ir.includes[0], "tasks/align.wdl");
        assert_eq!(ir.includes[1], "tasks/qc.wdl");
    }

    #[test]
    fn test_parse_runtime_resources() {
        let wdl = r#"
version 1.0

task heavy {
    command {
        run_heavy_job
    }
    output {
        File result = "result.dat"
    }
    runtime {
        cpu: 16
        memory: "32 GB"
        disks: "local-disk 200 SSD"
        docker: "biocontainers/samtools:1.15"
        gpuCount: 2
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        let rule = &ir.rules[0];

        assert_eq!(rule.threads, Some(ThreadsIR::Literal(16)));

        let mem_res = rule.resources.iter().find(|r| r.key == "mem").unwrap();
        assert_eq!(mem_res.value, "32G");

        let disk_res = rule.resources.iter().find(|r| r.key == "disk").unwrap();
        assert_eq!(disk_res.value, "200G");

        let gpu_res = rule.resources.iter().find(|r| r.key == "gpu").unwrap();
        assert_eq!(gpu_res.value, "2");
    }

    #[test]
    fn test_convert_wdl_placeholders() {
        assert_eq!(convert_wdl_placeholders("~{var}"), "{var}");
        assert_eq!(convert_wdl_placeholders("${var}"), "{var}");
        assert_eq!(
            convert_wdl_placeholders("echo ~{name} > ~{output}"),
            "echo {name} > {output}"
        );
    }

    #[test]
    fn test_convert_wdl_memory() {
        assert_eq!(convert_wdl_memory("16 GB"), "16G");
        assert_eq!(convert_wdl_memory("500 MiB"), "500M");
        assert_eq!(convert_wdl_memory("4 GiB"), "4G");
    }

    #[test]
    fn test_convert_wdl_disks() {
        assert_eq!(convert_wdl_disks("local-disk 100 SSD"), "100G");
        assert_eq!(convert_wdl_disks("local-disk 50 HDD"), "50G");
    }

    #[test]
    fn test_wdl_expr_to_pattern() {
        assert_eq!(wdl_expr_to_pattern("\"output.txt\""), "output.txt");
        assert_eq!(
            wdl_expr_to_pattern("\"results/~{sample}.bam\""),
            "results/{sample}.bam"
        );
        assert_eq!(wdl_expr_to_pattern("glob(\"*.vcf\")"), "# glob: *.vcf");
    }

    #[test]
    fn test_optional_input_escalation() {
        let wdl = r#"
version 1.0

task maybe {
    input {
        File? optional_input
        File required_input
    }
    command {
        echo "test"
    }
    output {
        File result = "result.txt"
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        assert!(!ir.escalations.is_empty());
        assert!(ir.escalations[0].construct.contains("optional_input"));
    }

    #[test]
    fn test_heredoc_body_braces_are_opaque() {
        // Braces inside a `command <<< … >>>` body must not affect task
        // brace matching: `echo "}"` used to close the task early and
        // silently swallow the output section (H31).
        let wdl = r#"
version 1.1

task awk_task {
    command <<<
        awk '{n++} END {print n}' in.txt
        echo "}"
    >>>
    output {
        File counted = "counted.txt"
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        assert_eq!(ir.rules.len(), 1);
        let rule = &ir.rules[0];
        assert_eq!(
            rule.outputs.len(),
            1,
            "output section after heredoc command must be parsed"
        );
        assert_eq!(rule.outputs[0].pattern, "counted.txt");
    }

    #[test]
    fn test_unclosed_heredoc_brace_does_not_swallow_task() {
        // `${VAR` with no closing brace inside a heredoc used to leave the
        // task brace depth permanently open → entire task skipped without
        // any diagnostic (H31).
        let wdl = r#"
version 1.1

task shell_var {
    command <<<
        echo ${VAR
    >>>
    output {
        File r = "r.txt"
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        assert_eq!(ir.rules.len(), 1, "task with raw shell text must survive");
    }

    #[test]
    fn test_unbalanced_braces_emit_diagnostic() {
        // A truly unbalanced task block (brace-form command with a stray
        // `{`) cannot be parsed — but it must say so, not vanish (H31).
        let wdl = r#"
version 1.0

task bad {
    command {
        echo "{"
    }
    output {
        File r = "r.txt"
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        assert!(
            ir.diagnostics
                .iter()
                .any(|d| { d.level == DiagLevel::Error && d.message.contains("bad") }),
            "unbalanced task must produce an error diagnostic, got: {:?}",
            ir.diagnostics
        );
    }

    #[test]
    fn test_unmapped_input_types_escalate() {
        // Int/Boolean/Float inputs cannot be represented as file inputs;
        // they become params whose value is a placeholder comment. That
        // MUST surface as a structured escalation, not pass silently (H32).
        let wdl = r#"
version 1.0

task typed {
    input {
        File data
        Int threads_count
        Boolean verbose
        Float cutoff
    }
    command {
        process ~{data} ~{threads_count} ~{verbose} ~{cutoff}
    }
    output {
        File result = "result.txt"
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        let rule = &ir.rules[0];
        // Params still exist so the rule structure is visible.
        assert_eq!(rule.params.len(), 3);

        // Each unmapped type must produce an escalation naming the input.
        for name in ["threads_count", "verbose", "cutoff"] {
            assert!(
                ir.escalations.iter().any(|e| {
                    e.construct == "unmapped_input_type" && e.original_code.contains(name)
                }),
                "expected escalation for unmapped WDL input '{name}'"
            );
        }
    }

    #[test]
    fn test_workflow_level_unmapped_input_escalates() {
        let wdl = r#"
version 1.0

task t {
    command {
        echo hi
    }
    output {
        File r = "r.txt"
    }
}

workflow w {
    input {
        Int batch_size
    }
    call t
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        assert!(
            ir.escalations.iter().any(|e| {
                e.construct == "unmapped_input_type" && e.original_code.contains("batch_size")
            }),
            "expected escalation for unmapped workflow-level input"
        );
    }

    #[test]
    fn test_heredoc_command() {
        let wdl = r#"
version 1.1

task heredoc_task {
    input {
        File input_file
    }
    command <<<
        set -euo pipefail
        cat ~{input_file} | sort > sorted.txt
    >>>
    output {
        File sorted = "sorted.txt"
    }
}
"#;

        let ir = parse_wdl(wdl).unwrap();
        let rule = &ir.rules[0];
        if let ExecutionIR::Shell(ref cmd) = rule.execution {
            assert!(cmd.contains("set -euo pipefail"));
            assert!(cmd.contains("{input_file}"));
        } else {
            panic!("expected Shell execution");
        }
    }
}
