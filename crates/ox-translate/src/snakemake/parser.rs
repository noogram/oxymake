use anyhow::Result;
use regex::Regex;

use crate::ir::*;

/// Parse a Snakefile into a WorkflowIR.
pub fn parse_snakefile(content: &str) -> Result<WorkflowIR> {
    let mut parser = SnakefileParser::new(content);
    parser.parse();
    Ok(parser.into_ir())
}

// ---------------------------------------------------------------------------
// Parser state machine
// ---------------------------------------------------------------------------

struct SnakefileParser<'a> {
    lines: Vec<&'a str>,
    rules: Vec<RuleIR>,
    config_file: Option<String>,
    config_values: Vec<ConfigEntryIR>,
    includes: Vec<String>,
    diagnostics: Vec<Diagnostic>,
    escalations: Vec<Escalation>,
    global_container: Option<String>,
    global_report: Option<String>,
    next_esc_id: usize,
    /// Tracks whether the current rule being parsed has expand() calls.
    current_rule_has_expand: bool,
}

#[derive(Debug)]
struct RawDirective {
    name: String,
    body: String,
    line: usize,
}

impl<'a> SnakefileParser<'a> {
    fn new(content: &'a str) -> Self {
        Self {
            lines: content.lines().collect(),
            rules: Vec::new(),
            config_file: None,
            config_values: Vec::new(),
            includes: Vec::new(),
            diagnostics: Vec::new(),
            escalations: Vec::new(),
            global_container: None,
            global_report: None,
            next_esc_id: 1,
            current_rule_has_expand: false,
        }
    }

    fn make_esc_id(&mut self) -> String {
        let id = format!("esc-{:04}", self.next_esc_id);
        self.next_esc_id += 1;
        id
    }

    fn add_escalation(&mut self, mut esc: Escalation) {
        esc.id = self.make_esc_id();
        self.escalations.push(esc);
    }

    fn parse(&mut self) {
        let n = self.lines.len();
        let mut i = 0;

        let rule_re = Regex::new(r"^rule\s+(\w+)\s*:").unwrap();
        let configfile_re = Regex::new(r#"^configfile:\s*"([^"]+)""#).unwrap();
        let include_re = Regex::new(r#"^include:\s*"([^"]+)""#).unwrap();
        let container_re = Regex::new(r#"^container:\s*"([^"]+)""#).unwrap();
        let report_re = Regex::new(r#"^report:\s*"([^"]+)""#).unwrap();
        let pylist_re = Regex::new(r#"^(\w+)\s*=\s*\[([^\]]+)\]"#).unwrap();
        let string_re = Regex::new(r#""([^"]+)"|'([^']+)'"#).unwrap();

        while i < n {
            let line = self.lines[i];
            let trimmed = line.trim();

            if trimmed.is_empty() || trimmed.starts_with('#') {
                i += 1;
                continue;
            }

            // configfile
            if let Some(caps) = configfile_re.captures(trimmed) {
                self.config_file = Some(caps[1].to_string());
                i += 1;
                continue;
            }

            // include
            if let Some(caps) = include_re.captures(trimmed) {
                self.includes.push(caps[1].to_string());
                i += 1;
                continue;
            }

            // Global container (no longer silently dropped!)
            if let Some(caps) = container_re.captures(trimmed) {
                self.global_container = Some(caps[1].to_string());
                i += 1;
                continue;
            }

            // Global report (no longer silently dropped!)
            if let Some(caps) = report_re.captures(trimmed) {
                self.global_report = Some(caps[1].to_string());
                self.add_escalation(Escalation::new(
                    EscalationTier::Human,
                    EscalationCategory::MissingFeature,
                    Severity::Informational,
                    None,
                    "global report directive",
                    trimmed,
                    Some(i + 1),
                    EscalationInstructions::new(
                        "Global report directive has no OxyMake equivalent",
                        vec![
                            "Decide on a reporting strategy for the OxyMake workflow",
                            "Consider post-hoc report generation from execution history",
                        ],
                        vec!["Reporting approach documented"],
                    ),
                ));
                i += 1;
                continue;
            }

            // Inline Python list: SAMPLES = ["A", "B", "C"] → Tier 1 mechanical
            if let Some(caps) = pylist_re.captures(trimmed) {
                let varname = caps[1].to_string();
                let items_str = &caps[2];
                let values: Vec<String> = string_re
                    .captures_iter(items_str)
                    .map(|c| c.get(1).or(c.get(2)).unwrap().as_str().to_string())
                    .collect();
                if !values.is_empty() {
                    self.config_values.push(ConfigEntryIR {
                        key: varname.to_lowercase(),
                        values,
                    });
                    i += 1;
                    continue;
                }
            }

            // Global wildcard_constraints
            if trimmed.starts_with("wildcard_constraints:") {
                let start = i;
                i += 1;
                while i < n && is_indented(self.lines[i]) {
                    i += 1;
                }
                self.diagnostics.push(Diagnostic {
                    level: DiagLevel::Info,
                    message: "Global wildcard_constraints found (not yet translated)".into(),
                    line: Some(start + 1),
                });
                continue;
            }

            // rule
            if let Some(caps) = rule_re.captures(trimmed) {
                let rule_name = caps[1].to_string();
                let rule_start = i;
                i += 1;

                while i < n {
                    let rline = self.lines[i];
                    if rline.trim().is_empty() {
                        let mut j = i + 1;
                        while j < n && self.lines[j].trim().is_empty() {
                            j += 1;
                        }
                        if j < n && is_indented(self.lines[j]) {
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    if !is_indented(rline) {
                        break;
                    }
                    i += 1;
                }

                let rule_lines = &self.lines[rule_start + 1..i];
                let directives = self.extract_directives(rule_lines, rule_start + 1);
                let rule = self.build_rule(&rule_name, &directives, rule_start);
                self.rules.push(rule);
                continue;
            }

            // Python imports / top-level code
            if trimmed.starts_with("from ") || trimmed.starts_with("import ") {
                self.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warning,
                    message: format!("Python import not translatable: {}", truncate(trimmed, 60)),
                    line: Some(i + 1),
                });
            } else if trimmed.starts_with("min_version(") {
                // Snakemake version constraint — informational only
                self.diagnostics.push(Diagnostic {
                    level: DiagLevel::Info,
                    message: format!("Version constraint dropped: {}", truncate(trimmed, 60)),
                    line: Some(i + 1),
                });
            } else if let Some(construct) = unsupported_top_level_construct(trimmed) {
                self.diagnostics.push(Diagnostic {
                    level: DiagLevel::Error,
                    message: format!(
                        "Snakefile:{}: unsupported construct '{}' (use --lossy to opt out)",
                        i + 1,
                        construct
                    ),
                    line: Some(i + 1),
                });
            } else {
                self.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warning,
                    message: format!(
                        "Unrecognized top-level construct: {}",
                        truncate(trimmed, 60)
                    ),
                    line: Some(i + 1),
                });
            }
            i += 1;
        }
    }

    fn extract_directives(&self, lines: &[&str], base_line: usize) -> Vec<RawDirective> {
        let directive_re = Regex::new(r#"^\s{4}(\w+)\s*:\s*(.*)"#).unwrap();
        let mut directives = Vec::new();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];
            if let Some(caps) = directive_re.captures(line) {
                let name = caps[1].to_string();
                let first_line_val = caps[2].trim().to_string();
                let dir_line = base_line + i;
                i += 1;

                if name == "run" {
                    // Collect the raw body lines, then strip only the *common*
                    // leading indentation. Trimming each line individually
                    // would flatten the block and break nested Python
                    // (`with`, `for`, `if`, ...).
                    let mut raw_lines: Vec<&str> = Vec::new();
                    while i < lines.len() {
                        let rline = lines[i];
                        if rline.trim().is_empty() || has_indent(rline, 8) {
                            raw_lines.push(rline);
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    // Leading whitespace is ASCII (spaces/tabs), so byte
                    // offsets are safe for slicing.
                    let common_indent = raw_lines
                        .iter()
                        .filter(|l| !l.trim().is_empty())
                        .map(|l| l.len() - l.trim_start().len())
                        .min()
                        .unwrap_or(0);
                    let mut body = String::new();
                    for rline in &raw_lines {
                        if rline.trim().is_empty() {
                            body.push('\n');
                        } else {
                            body.push_str(rline[common_indent..].trim_end());
                            body.push('\n');
                        }
                    }
                    directives.push(RawDirective {
                        name,
                        body: body.trim_end().to_string(),
                        line: dir_line,
                    });
                    continue;
                }

                let mut body = first_line_val;
                while i < lines.len() {
                    let rline = lines[i];
                    if rline.trim().is_empty() {
                        i += 1;
                        continue;
                    }
                    if has_indent(rline, 8) {
                        if !body.is_empty() {
                            body.push('\n');
                        }
                        body.push_str(rline.trim());
                        i += 1;
                    } else {
                        break;
                    }
                }

                directives.push(RawDirective {
                    name,
                    body,
                    line: dir_line,
                });
            } else {
                i += 1;
            }
        }

        directives
    }

    fn build_rule(
        &mut self,
        name: &str,
        directives: &[RawDirective],
        source_line: usize,
    ) -> RuleIR {
        let mut rule = RuleIR {
            name: name.to_string(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            execution: ExecutionIR::None,
            threads: None,
            resources: Vec::new(),
            environment: None,
            wildcard_constraints: Vec::new(),
            params: Vec::new(),
            log: Vec::new(),
            source_line: Some(source_line + 1),
            expand: None,
        };

        // Reset expand tracking before parsing this rule's directives
        self.current_rule_has_expand = false;

        for dir in directives {
            match dir.name.as_str() {
                "input" => {
                    rule.inputs = self.parse_port_list_with_escalations(&dir.body, name, dir.line);
                }
                "output" => {
                    rule.outputs =
                        self.parse_output_list_with_escalations(&dir.body, name, dir.line);
                }
                "shell" => {
                    let cmd = extract_string_literal(&dir.body);
                    rule.execution = ExecutionIR::Shell(translate_shell_command(&cmd));
                }
                "run" => {
                    rule.execution = ExecutionIR::Run(dir.body.clone());
                }
                "script" => {
                    let path = extract_string_literal(&dir.body);
                    rule.execution = ExecutionIR::Script(path);
                }
                "notebook" => {
                    let path = extract_string_literal(&dir.body);
                    rule.execution = ExecutionIR::Notebook(path);
                }
                "threads" => {
                    let trimmed = dir.body.trim();
                    if let Ok(n) = trimmed.parse::<u32>() {
                        rule.threads = Some(ThreadsIR::Literal(n));
                    } else {
                        // No longer silently dropped!
                        rule.threads = Some(ThreadsIR::Dynamic(trimmed.to_string()));
                        self.add_escalation(
                            Escalation::new(
                                EscalationTier::Assisted,
                                EscalationCategory::DynamicThreads,
                                Severity::Correctness,
                                Some(name),
                                &format!("threads: {}", trimmed),
                                trimmed,
                                Some(dir.line + 1),
                                EscalationInstructions::new(
                                    "Dynamic thread count requires manual resolution",
                                    vec![
                                        "Inspect the function to determine the thread count",
                                        "Replace with a fixed value or config reference",
                                        "Add resources = { cpu = N } to the rule",
                                    ],
                                    vec![
                                        "Rule has a valid cpu resource value",
                                        "oxymake lint passes",
                                    ],
                                ),
                            )
                            .with_context(EscalationContext {
                                function_name: Some(trimmed.trim_end_matches("()").to_string()),
                                ..Default::default()
                            }),
                        );
                    }
                }
                "resources" => {
                    rule.resources = parse_resources(&dir.body);
                }
                "conda" => {
                    let path = extract_string_literal(&dir.body);
                    rule.environment = Some(EnvironmentIR::Conda(path));
                }
                "container" => {
                    let img = extract_string_literal(&dir.body);
                    rule.environment = Some(EnvironmentIR::Container(img));
                }
                "singularity" => {
                    let img = extract_string_literal(&dir.body);
                    rule.environment = Some(EnvironmentIR::Singularity(img));
                }
                "wildcard_constraints" => {
                    rule.wildcard_constraints = parse_wildcard_constraints(&dir.body);
                }
                "params" => {
                    rule.params = self.parse_params_with_escalations(&dir.body, name, dir.line);
                }
                "log" => {
                    rule.log = parse_log_paths(&dir.body);
                }
                "wrapper" => {
                    let wrapper_id = extract_string_literal(&dir.body);
                    self.add_escalation(
                        Escalation::new(
                            EscalationTier::Assisted,
                            EscalationCategory::MissingFeature,
                            Severity::Correctness,
                            Some(name),
                            &format!("wrapper: \"{}\"", wrapper_id),
                            &dir.body,
                            Some(dir.line + 1),
                            EscalationInstructions::new(
                                "Snakemake wrapper must be inlined as a shell/script block",
                                vec![
                                    &format!("Fetch wrapper from snakemake-wrappers registry: {}", wrapper_id),
                                    "Extract the wrapper's shell command or script",
                                    "Inline it as the rule's shell or script block",
                                    "Adjust variable references from snakemake.* to OxyMake {input}/{output}",
                                ],
                                vec![
                                    "Rule has a valid shell or script block",
                                    "No snakemake.* references remain",
                                    "oxymake lint passes",
                                ],
                            ),
                        )
                        .with_context(EscalationContext {
                            directive_name: Some("wrapper".into()),
                            directive_value: Some(wrapper_id),
                            ..Default::default()
                        }),
                    );
                }
                "benchmark" | "retries" | "priority" => {
                    self.add_escalation(
                        Escalation::new(
                            EscalationTier::MechanicalDeferred,
                            EscalationCategory::MissingFeature,
                            if dir.name == "retries" {
                                Severity::Correctness
                            } else {
                                Severity::Performance
                            },
                            Some(name),
                            &format!("{}: {}", dir.name, dir.body.trim()),
                            &dir.body,
                            Some(dir.line + 1),
                            EscalationInstructions::new(
                                &format!("'{}' directive not yet supported in OxyMake", dir.name),
                                vec![
                                    &format!(
                                        "Track OxyMake feature request for '{}' support",
                                        dir.name
                                    ),
                                    "Once supported, re-run translator for mechanical conversion",
                                ],
                                vec![&format!("OxyMake supports '{}' directive", dir.name)],
                            ),
                        )
                        .with_context(EscalationContext {
                            directive_name: Some(dir.name.clone()),
                            directive_value: Some(dir.body.trim().to_string()),
                            ..Default::default()
                        }),
                    );
                }
                "message" => {
                    // Tier 1 mechanical: map to description
                    self.diagnostics.push(Diagnostic {
                        level: DiagLevel::Info,
                        message: format!("Rule '{}': message mapped to description", name),
                        line: Some(dir.line + 1),
                    });
                }
                "shadow" | "group" | "envmodules" => {
                    self.add_escalation(
                        Escalation::new(
                            EscalationTier::Assisted,
                            EscalationCategory::MissingFeature,
                            if dir.name == "group" {
                                Severity::Performance
                            } else {
                                Severity::Correctness
                            },
                            Some(name),
                            &format!("{}: {}", dir.name, dir.body.trim()),
                            &dir.body,
                            Some(dir.line + 1),
                            EscalationInstructions::new(
                                &format!("'{}' directive needs manual conversion", dir.name),
                                vec![
                                    &format!("Evaluate whether '{}' behavior is needed", dir.name),
                                    "Implement equivalent behavior in the shell command if needed",
                                ],
                                vec!["Workflow behavior is preserved"],
                            ),
                        )
                        .with_context(EscalationContext {
                            directive_name: Some(dir.name.clone()),
                            directive_value: Some(dir.body.trim().to_string()),
                            ..Default::default()
                        }),
                    );
                }
                _ => {
                    if is_unsupported_construct_name(&dir.name) {
                        self.diagnostics.push(Diagnostic {
                            level: DiagLevel::Error,
                            message: format!(
                                "Snakefile:{}: unsupported construct '{}' (use --lossy to opt out)",
                                dir.line + 1,
                                dir.name
                            ),
                            line: Some(dir.line + 1),
                        });
                    } else {
                        self.diagnostics.push(Diagnostic {
                            level: DiagLevel::Warning,
                            message: format!("Rule '{}': unknown directive '{}'", name, dir.name),
                            line: Some(dir.line + 1),
                        });
                    }
                }
            }
        }

        // If any input used expand(), set expand = "product" on the rule
        if self.current_rule_has_expand {
            rule.expand = Some("product".to_string());
        }

        rule
    }

    /// Parse input port list, emitting escalations for complex expressions.
    fn parse_port_list_with_escalations(
        &mut self,
        body: &str,
        rule_name: &str,
        line: usize,
    ) -> Vec<PortIR> {
        let mut ports = Vec::new();
        let named_re = Regex::new(r#"(\w+)\s*=\s*"([^"]+)""#).unwrap();
        let expand_re = Regex::new(r#"expand\(\s*"([^"]+)"\s*,\s*(.*)\)"#).unwrap();
        let string_re = Regex::new(r#""([^"]+)""#).unwrap();
        let config_expand_re =
            Regex::new(r#"expand\(\s*"([^"]+)"\s*,\s*(\w+)\s*=\s*config\["(\w+)"\]\s*\)"#).unwrap();
        let literal_expand_re =
            Regex::new(r#"expand\(\s*"([^"]+)"\s*,\s*(\w+)\s*=\s*\[([^\]]+)\]\s*\)"#).unwrap();
        let str_re = Regex::new(r#""([^"]+)"|'([^']+)'"#).unwrap();

        let joined = body.replace('\n', " ");
        let items = split_respecting_parens(&joined);

        for item in items {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }

            // Tier 1: expand() with config["key"] reference
            if let Some(caps) = config_expand_re.captures(item) {
                let pattern = caps[1].to_string();
                let _wc_name = caps[2].to_string();
                let config_key = caps[3].to_string();
                self.current_rule_has_expand = true;
                ports.push(PortIR {
                    name: None,
                    pattern,
                    lifecycle: None,
                });
                // Record that this config key is referenced
                if !self.config_values.iter().any(|c| c.key == config_key) {
                    // Will be populated from YAML or needs manual config
                    self.diagnostics.push(Diagnostic {
                        level: DiagLevel::Info,
                        message: format!(
                            "expand() references config[\"{}\"] — ensure [config].{} is defined",
                            config_key, config_key
                        ),
                        line: Some(line + 1),
                    });
                }
                continue;
            }

            // Tier 1: expand() with literal list
            if let Some(caps) = literal_expand_re.captures(item) {
                let pattern = caps[1].to_string();
                let wc_name = caps[2].to_string();
                let list_str = &caps[3];
                self.current_rule_has_expand = true;
                let values: Vec<String> = str_re
                    .captures_iter(list_str)
                    .map(|c| c.get(1).or(c.get(2)).unwrap().as_str().to_string())
                    .collect();
                if !values.is_empty() {
                    // Add to config values
                    if !self.config_values.iter().any(|c| c.key == wc_name) {
                        self.config_values.push(ConfigEntryIR {
                            key: wc_name,
                            values,
                        });
                    }
                }
                ports.push(PortIR {
                    name: None,
                    pattern,
                    lifecycle: None,
                });
                continue;
            }

            // Tier 2: expand() with complex iterator
            if let Some(caps) = expand_re.captures(item) {
                let pattern = caps[1].to_string();
                let iterator = caps[2].trim().to_string();
                self.current_rule_has_expand = true;
                ports.push(PortIR {
                    name: None,
                    pattern: pattern.clone(),
                    lifecycle: None,
                });
                self.add_escalation(
                    Escalation::new(
                        EscalationTier::Assisted,
                        EscalationCategory::DynamicInput,
                        Severity::Correctness,
                        Some(rule_name),
                        &format!("expand(\"{}\", {})", pattern, iterator),
                        item,
                        Some(line + 1),
                        EscalationInstructions::new(
                            "expand() with complex iterator needs manual resolution",
                            vec![
                                "Identify the data source for the iterator",
                                "Extract unique values and add to [config] section",
                                "Ensure wildcard expansion covers all values",
                            ],
                            vec!["All expand values are in [config]", "oxymake lint passes"],
                        ),
                    )
                    .with_context(EscalationContext {
                        function_name: Some(iterator),
                        ..Default::default()
                    }),
                );
                continue;
            }

            // Named input
            if let Some(caps) = named_re.captures(item) {
                let name = caps[1].to_string();
                if name == "sample" || name == "zip" {
                    continue;
                }
                let path = caps[2].to_string();
                ports.push(PortIR {
                    name: Some(name),
                    pattern: path,
                    lifecycle: None,
                });
                continue;
            }

            // Lifecycle modifiers
            if let Some(port) = try_parse_lifecycle(item) {
                ports.push(port);
                continue;
            }

            // Simple string
            if let Some(caps) = string_re.captures(item) {
                ports.push(PortIR {
                    name: None,
                    pattern: caps[1].to_string(),
                    lifecycle: None,
                });
                continue;
            }

            // Complex expression — Tier 2 escalation (NOT # MANUAL:)
            if !item.is_empty() {
                ports.push(PortIR {
                    name: None,
                    pattern: format!("# ESCALATION: {}", item),
                    lifecycle: None,
                });
                self.add_escalation(
                    Escalation::new(
                        EscalationTier::Assisted,
                        EscalationCategory::DynamicInput,
                        Severity::Correctness,
                        Some(rule_name),
                        item,
                        item,
                        Some(line + 1),
                        EscalationInstructions::new(
                            "Complex Python expression in input needs manual conversion",
                            vec![
                                "Inspect the original Python expression",
                                "Determine what file paths it generates",
                                "Replace with static paths or a preprocessing step",
                            ],
                            vec!["Input resolves to valid file paths", "oxymake lint passes"],
                        ),
                    )
                    .with_context(EscalationContext {
                        function_name: if item.contains('(') {
                            Some(item.split('(').next().unwrap_or(item).trim().to_string())
                        } else {
                            None
                        },
                        ..Default::default()
                    }),
                );
            }
        }

        ports
    }

    /// Parse output port list with report() wrapper detection (no longer silently dropped).
    fn parse_output_list_with_escalations(
        &mut self,
        body: &str,
        rule_name: &str,
        line: usize,
    ) -> Vec<PortIR> {
        let mut ports = Vec::new();
        let named_re = Regex::new(r#"(\w+)\s*=\s*"([^"]+)""#).unwrap();
        let string_re = Regex::new(r#""([^"]+)""#).unwrap();
        // report() wrapper detection
        let report_re = Regex::new(r#"report\(\s*"([^"]+)"\s*(?:,\s*"([^"]+)")?\s*\)"#).unwrap();
        let named_report_re =
            Regex::new(r#"(\w+)\s*=\s*report\(\s*"([^"]+)"\s*(?:,\s*"([^"]+)")?\s*\)"#).unwrap();

        let joined = body.replace('\n', " ");
        let items = split_respecting_parens(&joined);

        for item in items {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }

            // Named report() wrapper: name=report("path", "template")
            if let Some(caps) = named_report_re.captures(item) {
                let name = caps[1].to_string();
                let path = caps[2].to_string();
                let template = caps.get(3).map(|m| m.as_str().to_string());
                ports.push(PortIR {
                    name: Some(name),
                    pattern: path.clone(),
                    lifecycle: None,
                });
                if let Some(tpl) = &template {
                    self.add_escalation(Escalation::new(
                        EscalationTier::Human,
                        EscalationCategory::MissingFeature,
                        Severity::Informational,
                        Some(rule_name),
                        &format!("report(\"{}\", \"{}\")", path, tpl),
                        item,
                        Some(line + 1),
                        EscalationInstructions::new(
                            "Report template annotation has no OxyMake equivalent",
                            vec![
                                "Decide on a reporting strategy",
                                "Consider post-hoc report generation from execution history",
                                &format!("Original template: {}", tpl),
                            ],
                            vec!["Reporting approach documented"],
                        ),
                    ));
                }
                continue;
            }

            // Unnamed report() wrapper: report("path", "template")
            if let Some(caps) = report_re.captures(item) {
                let path = caps[1].to_string();
                let template = caps.get(2).map(|m| m.as_str().to_string());
                ports.push(PortIR {
                    name: None,
                    pattern: path.clone(),
                    lifecycle: None,
                });
                if let Some(tpl) = &template {
                    self.add_escalation(Escalation::new(
                        EscalationTier::Human,
                        EscalationCategory::MissingFeature,
                        Severity::Informational,
                        Some(rule_name),
                        &format!("report(\"{}\", \"{}\")", path, tpl),
                        item,
                        Some(line + 1),
                        EscalationInstructions::new(
                            "Report template annotation has no OxyMake equivalent",
                            vec![
                                "Decide on a reporting strategy",
                                &format!("Original template: {}", tpl),
                            ],
                            vec!["Reporting approach documented"],
                        ),
                    ));
                }
                continue;
            }

            // Named output: name="path"
            if let Some(caps) = named_re.captures(item) {
                let name = caps[1].to_string();
                if name == "sample" || name == "zip" {
                    continue;
                }
                ports.push(PortIR {
                    name: Some(name),
                    pattern: caps[2].to_string(),
                    lifecycle: None,
                });
                continue;
            }

            // Lifecycle modifiers
            if let Some(port) = try_parse_lifecycle(item) {
                ports.push(port);
                continue;
            }

            // Simple string
            if let Some(caps) = string_re.captures(item) {
                ports.push(PortIR {
                    name: None,
                    pattern: caps[1].to_string(),
                    lifecycle: None,
                });
                continue;
            }

            // Complex output expression
            if !item.is_empty() {
                ports.push(PortIR {
                    name: None,
                    pattern: format!("# ESCALATION: {}", item),
                    lifecycle: None,
                });
                self.add_escalation(Escalation::new(
                    EscalationTier::Assisted,
                    EscalationCategory::DynamicInput,
                    Severity::Correctness,
                    Some(rule_name),
                    item,
                    item,
                    Some(line + 1),
                    EscalationInstructions::new(
                        "Complex output expression needs manual conversion",
                        vec![
                            "Inspect the original expression",
                            "Replace with static paths",
                        ],
                        vec!["oxymake lint passes"],
                    ),
                ));
            }
        }

        ports
    }

    /// Parse params with escalations for complex expressions.
    fn parse_params_with_escalations(
        &mut self,
        body: &str,
        rule_name: &str,
        line: usize,
    ) -> Vec<ParamIR> {
        let mut params = Vec::new();
        let joined = body.replace('\n', " ");
        let items = split_respecting_parens(&joined);
        let named_re = Regex::new(r#"^(\w+)\s*=\s*(.*)"#).unwrap();
        // Tier 1: config.get("key", default)
        let config_get_re = Regex::new(r#"^config\.get\(\s*"(\w+)"\s*,\s*(.+)\s*\)$"#).unwrap();
        // Tier 1: config["key"]
        let config_bracket_re = Regex::new(r#"^config\["(\w+)"\]$"#).unwrap();

        for item in items {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            if let Some(caps) = named_re.captures(item) {
                let name = caps[1].to_string();
                let raw_value = caps[2].trim().trim_end_matches(',').to_string();

                // Try Tier 1: config.get("key", default)
                if let Some(cfg_caps) = config_get_re.captures(&raw_value) {
                    let config_key = cfg_caps[1].to_string();
                    let default = cfg_caps[2].trim().to_string();
                    let clean_default = extract_string_literal(&default);
                    // Add config entry if not already present
                    if !self.config_values.iter().any(|c| c.key == config_key) {
                        self.config_values.push(ConfigEntryIR {
                            key: config_key.clone(),
                            values: vec![clean_default.clone()],
                        });
                    }
                    params.push(ParamIR {
                        name,
                        value: format!("{{config.{}}}", config_key),
                    });
                    continue;
                }

                // Try Tier 1: config["key"]
                if let Some(cfg_caps) = config_bracket_re.captures(&raw_value) {
                    let config_key = cfg_caps[1].to_string();
                    params.push(ParamIR {
                        name,
                        value: format!("{{config.{}}}", config_key),
                    });
                    continue;
                }

                // Detect complex expressions
                let is_complex = raw_value.starts_with("lambda ")
                    || (raw_value.contains('(')
                        && !raw_value.starts_with('"')
                        && !raw_value.starts_with('\''));

                if is_complex {
                    params.push(ParamIR {
                        name: name.clone(),
                        value: format!("# ESCALATION: {}", raw_value),
                    });
                    self.add_escalation(
                        Escalation::new(
                            EscalationTier::Assisted,
                            EscalationCategory::DynamicParam,
                            Severity::Correctness,
                            Some(rule_name),
                            &format!("params.{} = {}", name, raw_value),
                            &raw_value,
                            Some(line + 1),
                            EscalationInstructions::new(
                                &format!(
                                    "Complex param '{}' requires manual conversion",
                                    name
                                ),
                                vec![
                                    "Inspect the Python expression to understand what it computes",
                                    "If it only interpolates wildcards, convert to OxyMake {sample} syntax",
                                    "If it has conditionals, move logic to a script block",
                                    "If it references external data, add a preprocessing step",
                                ],
                                vec![
                                    &format!("params.{} has a valid value", name),
                                    "No Python expressions remain in params",
                                ],
                            ),
                        )
                        .with_context(EscalationContext {
                            function_name: if raw_value.starts_with("lambda ") {
                                Some("lambda".into())
                            } else {
                                raw_value
                                    .split('(')
                                    .next()
                                    .map(|s| s.trim().to_string())
                            },
                            ..Default::default()
                        }),
                    );
                } else {
                    let v = extract_string_literal(&raw_value);
                    params.push(ParamIR { name, value: v });
                }
            }
        }
        params
    }

    fn into_ir(self) -> WorkflowIR {
        WorkflowIR {
            rules: self.rules,
            config_file: self.config_file,
            includes: self.includes,
            diagnostics: self.diagnostics,
            config_values: self.config_values,
            escalations: self.escalations,
            global_container: self.global_container,
            global_report: self.global_report,
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Snakemake constructs that cannot be losslessly translated to OxyMake and
/// must fail loudly unless the operator explicitly opts in via `--lossy`.
pub(crate) const UNSUPPORTED_CONSTRUCTS: &[&str] = &[
    "checkpoint",
    "module",
    "pepfile",
    "localrules",
    "ruleorder",
    "onsuccess",
    "onerror",
    "onstart",
];

/// If `trimmed` (a top-level Snakefile line, leading whitespace removed) opens
/// with one of the unsupported constructs, return that construct's name.
/// Matches both `keyword name:` (e.g. `checkpoint foo:`) and `keyword: ...`
/// (e.g. `pepfile: "config.yaml"`).
pub(crate) fn unsupported_top_level_construct(trimmed: &str) -> Option<&'static str> {
    let head = trimmed
        .split(|c: char| c.is_whitespace() || c == ':')
        .next()
        .unwrap_or("");
    UNSUPPORTED_CONSTRUCTS
        .iter()
        .copied()
        .find(|&kw| kw == head)
}

/// True when `name` is the bare keyword of an unsupported construct.
pub(crate) fn is_unsupported_construct_name(name: &str) -> bool {
    UNSUPPORTED_CONSTRUCTS.contains(&name)
}

pub(crate) fn is_indented(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

pub(crate) fn has_indent(line: &str, min_spaces: usize) -> bool {
    let spaces = line.len() - line.trim_start().len();
    spaces >= min_spaces
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Cut at the last char boundary at or before `max` bytes —
        // byte-slicing mid-codepoint panics on non-ASCII input (H30).
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

/// Translate Snakemake shell command conventions to OxyMake:
/// - `{{` / `}}` (Snakemake literal brace escape) → `{` / `}`
/// - `{wildcards.X}` → `{X}` (OxyMake uses bare wildcard names)
pub(crate) fn translate_shell_command(cmd: &str) -> String {
    // First, replace {wildcards.X} → {X}
    let wc_re = Regex::new(r"\{wildcards\.(\w+)\}").unwrap();
    let cmd = wc_re.replace_all(cmd, "{$1}");
    // Then, unescape double braces: {{ → { and }} → }
    cmd.replace("{{", "{").replace("}}", "}")
}

pub(crate) fn extract_string_literal(s: &str) -> String {
    let s = s.trim();
    for q in &[r#"""""#, "'''"] {
        if let Some(stripped) = s.strip_prefix(q) {
            if let Some(end) = stripped.find(q) {
                return stripped[..end].to_string();
            }
        }
    }
    for q in &['"', '\''] {
        if s.starts_with(*q) {
            if let Some(end) = s[1..].find(*q) {
                return s[1..1 + end].to_string();
            }
        }
    }
    s.to_string()
}

fn try_parse_lifecycle(item: &str) -> Option<PortIR> {
    let temp_re = Regex::new(r#"temp\(\s*"([^"]+)"\s*\)"#).unwrap();
    let prot_re = Regex::new(r#"protected\(\s*"([^"]+)"\s*\)"#).unwrap();
    let dir_re = Regex::new(r#"directory\(\s*"([^"]+)"\s*\)"#).unwrap();

    if let Some(caps) = temp_re.captures(item) {
        return Some(PortIR {
            name: None,
            pattern: caps[1].to_string(),
            lifecycle: Some("temporary".into()),
        });
    }
    if let Some(caps) = prot_re.captures(item) {
        return Some(PortIR {
            name: None,
            pattern: caps[1].to_string(),
            lifecycle: Some("protected".into()),
        });
    }
    if let Some(caps) = dir_re.captures(item) {
        return Some(PortIR {
            name: None,
            pattern: caps[1].to_string(),
            lifecycle: Some("directory".into()),
        });
    }
    None
}

pub(crate) fn split_respecting_parens(s: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;

    for ch in s.chars() {
        match ch {
            '(' | '[' => {
                depth += 1;
                current.push(ch);
            }
            ')' | ']' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                items.push(current.clone());
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.trim().is_empty() {
        items.push(current);
    }
    items
}

pub(crate) fn parse_resources(body: &str) -> Vec<ResourceIR> {
    let re = Regex::new(r"(\w+)\s*=\s*(\S+)").unwrap();
    let mut resources = Vec::new();
    for caps in re.captures_iter(body) {
        resources.push(ResourceIR {
            key: caps[1].to_string(),
            value: caps[2].to_string(),
        });
    }
    resources
}

pub(crate) fn parse_log_paths(body: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let joined = body.replace('\n', " ");
    let items = split_respecting_parens(&joined);
    let string_re = Regex::new(r#""([^"]+)""#).unwrap();

    for item in items {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if let Some(caps) = string_re.captures(item) {
            paths.push(caps[1].to_string());
        }
    }
    paths
}

pub(crate) fn parse_wildcard_constraints(body: &str) -> Vec<(String, String)> {
    let re = Regex::new(r#"(\w+)\s*=\s*"([^"]+)""#).unwrap();
    let mut constraints = Vec::new();
    for caps in re.captures_iter(body) {
        constraints.push((caps[1].to_string(), caps[2].to_string()));
    }
    constraints
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_rule() {
        let input = r#"
rule process:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.txt"
    shell:
        "sort {input} > {output}"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.rules.len(), 1);
        let r = &ir.rules[0];
        assert_eq!(r.name, "process");
        assert_eq!(r.inputs[0].pattern, "data/{sample}.csv");
        assert_eq!(r.outputs[0].pattern, "results/{sample}.txt");
        assert!(
            matches!(&r.execution, ExecutionIR::Shell(cmd) if cmd == "sort {input} > {output}")
        );
        assert!(ir.escalations.is_empty());
    }

    #[test]
    fn test_named_inputs() {
        let input = r#"
rule align:
    input:
        fastq="data/{sample}.fastq",
        ref="refs/genome.fa"
    output:
        "results/{sample}.bam"
    threads: 8
    shell:
        "bwa mem -t {threads} {input.ref} {input.fastq} > {output}"
"#;
        let ir = parse_snakefile(input).unwrap();
        let r = &ir.rules[0];
        assert_eq!(r.inputs[0].name.as_deref(), Some("fastq"));
        assert_eq!(r.inputs[1].name.as_deref(), Some("ref"));
        assert_eq!(r.threads, Some(ThreadsIR::Literal(8)));
    }

    #[test]
    fn test_dynamic_threads_escalation() {
        let input = r#"
rule step:
    output:
        "out.txt"
    threads: get_threads()
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(
            ir.rules[0].threads,
            Some(ThreadsIR::Dynamic("get_threads()".into()))
        );
        assert_eq!(ir.escalations.len(), 1);
        assert_eq!(
            ir.escalations[0].category,
            EscalationCategory::DynamicThreads
        );
        assert_eq!(ir.escalations[0].tier, EscalationTier::Assisted);
        assert_eq!(ir.escalations[0].severity, Severity::Correctness);
    }

    #[test]
    fn test_params_config_get_tier1() {
        let input = r#"
rule gen:
    output:
        "out.txt"
    params:
        seed=config.get("seed", 42)
    shell:
        "echo {params.seed}"
"#;
        let ir = parse_snakefile(input).unwrap();
        let r = &ir.rules[0];
        assert_eq!(r.params[0].name, "seed");
        assert_eq!(r.params[0].value, "{config.seed}");
        // Config entry with default should have been created
        assert!(
            ir.config_values
                .iter()
                .any(|c| c.key == "seed" && c.values == vec!["42"])
        );
        // No escalations — this is Tier 1!
        assert!(ir.escalations.is_empty());
    }

    #[test]
    fn test_params_config_bracket_tier1() {
        let input = r#"
rule gen:
    output:
        "out.txt"
    params:
        species=config["species"]
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.rules[0].params[0].value, "{config.species}");
        assert!(ir.escalations.is_empty());
    }

    #[test]
    fn test_params_lambda_escalation() {
        let input = r#"
rule step:
    output:
        "out.txt"
    params:
        extra=lambda wc: compute(wc)
    shell:
        "run"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(ir.rules[0].params[0].value.starts_with("# ESCALATION:"));
        assert_eq!(ir.escalations.len(), 1);
        assert_eq!(ir.escalations[0].category, EscalationCategory::DynamicParam);
        assert!(!ir.escalations[0].instructions.steps.is_empty());
    }

    #[test]
    fn test_inline_python_list_tier1() {
        let input = r#"
SAMPLES = ["alpha", "beta", "gamma"]

rule all:
    input:
        "results/done.txt"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            ir.config_values
                .iter()
                .any(|c| c.key == "samples" && c.values == vec!["alpha", "beta", "gamma"])
        );
        // No warnings for this line — it was mechanically translated
        assert!(
            ir.diagnostics
                .iter()
                .all(|d| !d.message.contains("SAMPLES"))
        );
    }

    #[test]
    fn test_expand_with_literal_list_tier1() {
        let input = r#"
rule all:
    input:
        expand("results/{sample}.txt", sample=["A", "B", "C"])
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.rules[0].inputs[0].pattern, "results/{sample}.txt");
        assert!(
            ir.config_values
                .iter()
                .any(|c| c.key == "sample" && c.values == vec!["A", "B", "C"])
        );
        assert!(ir.escalations.is_empty());
    }

    #[test]
    fn test_expand_with_config_ref_tier1() {
        let input = r#"
configfile: "config.yaml"

rule all:
    input:
        expand("results/{sample}.txt", sample=config["samples"])
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.rules[0].inputs[0].pattern, "results/{sample}.txt");
        assert!(ir.escalations.is_empty());
    }

    #[test]
    fn test_expand_with_complex_iterator_escalation() {
        let input = r#"
rule count:
    input:
        expand("results/{unit.name}.txt", unit=units.itertuples())
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.escalations.len(), 1);
        assert_eq!(ir.escalations[0].category, EscalationCategory::DynamicInput);
        assert!(ir.escalations[0].construct.contains("itertuples"));
    }

    #[test]
    fn test_wrapper_escalation() {
        let input = r#"
rule align:
    input:
        "data/{sample}.fastq"
    output:
        "results/{sample}.bam"
    wrapper:
        "v7.2.0/bio/star/align"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.escalations.len(), 1);
        assert_eq!(
            ir.escalations[0].category,
            EscalationCategory::MissingFeature
        );
        assert_eq!(ir.escalations[0].tier, EscalationTier::Assisted);
        assert!(
            ir.escalations[0].context.directive_value.as_deref() == Some("v7.2.0/bio/star/align")
        );
    }

    #[test]
    fn test_benchmark_deferred_escalation() {
        let input = r#"
rule step:
    output:
        "out.txt"
    benchmark:
        "bench.txt"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.escalations.len(), 1);
        assert_eq!(ir.escalations[0].tier, EscalationTier::MechanicalDeferred);
        assert_eq!(ir.escalations[0].severity, Severity::Performance);
    }

    #[test]
    fn test_global_container_captured() {
        let input = r#"
container: "docker://ubuntu:22.04"

rule step:
    output:
        "out.txt"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(
            ir.global_container.as_deref(),
            Some("docker://ubuntu:22.04")
        );
    }

    #[test]
    fn test_global_report_escalation() {
        let input = r#"
report: "report/workflow.rst"

rule step:
    output:
        "out.txt"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.global_report.as_deref(), Some("report/workflow.rst"));
        assert_eq!(ir.escalations.len(), 1);
        assert_eq!(ir.escalations[0].tier, EscalationTier::Human);
    }

    #[test]
    fn test_report_output_wrapper_escalation() {
        let input = r#"
rule pca:
    input:
        "data/all.rds"
    output:
        report("results/pca.svg", "../report/pca.rst")
    shell:
        "Rscript plot.R"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.rules[0].outputs[0].pattern, "results/pca.svg");
        assert_eq!(ir.escalations.len(), 1);
        assert_eq!(ir.escalations[0].tier, EscalationTier::Human);
        assert!(ir.escalations[0].construct.contains("pca.rst"));
    }

    #[test]
    fn test_named_report_output() {
        let input = r#"
rule deseq2:
    output:
        table=report("results/diffexp.tsv", "../report/diffexp.rst")
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.rules[0].outputs[0].name.as_deref(), Some("table"));
        assert_eq!(ir.rules[0].outputs[0].pattern, "results/diffexp.tsv");
        assert_eq!(ir.escalations.len(), 1);
    }

    #[test]
    fn test_complex_input_escalation() {
        let input = r#"
rule all:
    input:
        get_final_output()
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.escalations.len(), 1);
        assert_eq!(ir.escalations[0].category, EscalationCategory::DynamicInput);
        assert!(ir.escalations[0].context.function_name.as_deref() == Some("get_final_output"));
    }

    #[test]
    fn test_unpack_input_escalation() {
        let input = r#"
rule align:
    input:
        unpack(get_fq)
    output:
        "results/{sample}.bam"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.escalations.len(), 1);
        assert!(ir.escalations[0].construct.contains("unpack"));
    }

    #[test]
    fn test_configfile() {
        let input = r#"
configfile: "config.yaml"

rule all:
    input:
        "results/done.txt"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.config_file.as_deref(), Some("config.yaml"));
    }

    #[test]
    fn test_log_single() {
        let input = r#"
rule count:
    output:
        "out.txt"
    log:
        "logs/count.log"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.rules[0].log, vec!["logs/count.log"]);
    }

    #[test]
    fn test_params_simple_literal() {
        let input = r#"
rule gen:
    output:
        "out.txt"
    params:
        n_rows=100,
        label="test"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(
            ir.rules[0].params[0],
            ParamIR {
                name: "n_rows".into(),
                value: "100".into()
            }
        );
        assert_eq!(
            ir.rules[0].params[1],
            ParamIR {
                name: "label".into(),
                value: "test".into()
            }
        );
        assert!(ir.escalations.is_empty());
    }

    #[test]
    fn test_lifecycle_modifiers() {
        let input = r#"
rule step:
    output:
        temp("tmp/intermediate.txt"),
        protected("results/final.txt")
    shell:
        "process"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(
            ir.rules[0].outputs[0].lifecycle.as_deref(),
            Some("temporary")
        );
        assert_eq!(
            ir.rules[0].outputs[1].lifecycle.as_deref(),
            Some("protected")
        );
    }

    #[test]
    fn test_run_block() {
        let input = r#"
rule analyze:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.json"
    run:
        import json
        print("hello")
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            matches!(&ir.rules[0].execution, ExecutionIR::Run(code) if code.contains("import json"))
        );
    }

    #[test]
    fn test_run_block_preserves_nested_indentation() {
        // Regression: the run body was dedented line-by-line with trim(),
        // flattening nested blocks and producing IndentationError at runtime.
        let input = r#"
rule analyze:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.json"
    run:
        import csv
        with open(input[0]) as f:
            data = list(csv.reader(f))
        for row in data:
            if row:
                print(row)
"#;
        let ir = parse_snakefile(input).unwrap();
        let ExecutionIR::Run(code) = &ir.rules[0].execution else {
            panic!("expected Run execution");
        };
        // Common 8-space indent stripped, relative indentation preserved.
        assert!(code.contains("import csv\n"), "code: {code}");
        assert!(code.contains("with open(input[0]) as f:\n"), "code: {code}");
        assert!(
            code.contains("    data = list(csv.reader(f))"),
            "nested block must keep its relative indent; code: {code}"
        );
        assert!(
            code.contains("        print(row)"),
            "doubly nested block must keep its relative indent; code: {code}"
        );
    }

    #[test]
    fn test_resources() {
        let input = r#"
rule heavy:
    output:
        "out.vcf"
    threads: 16
    resources:
        mem_mb=32000
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.rules[0].threads, Some(ThreadsIR::Literal(16)));
        assert_eq!(ir.rules[0].resources[0].key, "mem_mb");
    }

    #[test]
    fn test_conda_env() {
        let input = r#"
rule align:
    output:
        "out.bam"
    conda:
        "envs/alignment.yaml"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            matches!(&ir.rules[0].environment, Some(EnvironmentIR::Conda(p)) if p == "envs/alignment.yaml")
        );
    }

    #[test]
    fn test_container_env() {
        let input = r#"
rule step:
    output:
        "out.txt"
    container:
        "docker://ubuntu:22.04"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            matches!(&ir.rules[0].environment, Some(EnvironmentIR::Container(img)) if img == "docker://ubuntu:22.04")
        );
    }

    #[test]
    fn test_singularity_env() {
        let input = r#"
rule step:
    output:
        "out.txt"
    singularity:
        "image.sif"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            matches!(&ir.rules[0].environment, Some(EnvironmentIR::Singularity(img)) if img == "image.sif")
        );
    }

    #[test]
    fn test_script_execution() {
        let input = r#"
rule analyze:
    output:
        "out.json"
    script:
        "scripts/analyze.py"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            matches!(&ir.rules[0].execution, ExecutionIR::Script(p) if p == "scripts/analyze.py")
        );
    }

    #[test]
    fn test_notebook_execution() {
        let input = r#"
rule explore:
    output:
        "out.html"
    notebook:
        "notebooks/explore.ipynb"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            matches!(&ir.rules[0].execution, ExecutionIR::Notebook(p) if p == "notebooks/explore.ipynb")
        );
    }

    #[test]
    fn test_unknown_directive() {
        let input = r#"
rule step:
    output:
        "out.txt"
    foobar:
        "something"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            ir.diagnostics
                .iter()
                .any(|d| d.message.contains("unknown directive 'foobar'"))
        );
    }

    #[test]
    fn test_include_directive() {
        let input = r#"
include: "rules/align.smk"

rule all:
    input:
        "done.txt"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.includes, vec!["rules/align.smk"]);
    }

    #[test]
    fn test_wildcard_constraints_in_rule() {
        let input = r#"
rule gen:
    output:
        "data/{sample}.csv"
    wildcard_constraints:
        sample="[^.]+"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(
            ir.rules[0].wildcard_constraints[0],
            ("sample".into(), "[^.]+".into())
        );
    }

    #[test]
    fn test_helper_is_indented() {
        assert!(is_indented("    hello"));
        assert!(is_indented("\thello"));
        assert!(!is_indented("hello"));
    }

    #[test]
    fn test_helper_has_indent() {
        assert!(has_indent("        deep", 8));
        assert!(!has_indent("    shallow", 8));
    }

    #[test]
    fn test_helper_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("this is a long string", 10), "this is a ...");
    }

    #[test]
    fn test_helper_truncate_non_ascii_boundary() {
        // Byte 10 falls mid-codepoint ('é' is 2 bytes starting at odd
        // offsets after the leading 'a') — must cut at a char boundary
        // instead of panicking (H30).
        let s = format!("a{}", "é".repeat(35));
        let out = truncate(&s, 10);
        assert!(out.ends_with("..."));
        assert!(out.len() <= 13);

        // 3-byte chars, limit mid-codepoint.
        let jp = "日本語の長いコメント行";
        let out = truncate(jp, 10);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn test_helper_extract_string_literal() {
        assert_eq!(extract_string_literal(r#""hello""#), "hello");
        assert_eq!(extract_string_literal(r#"'world'"#), "world");
        assert_eq!(extract_string_literal(r#""""multi""""#), "multi");
        assert_eq!(extract_string_literal("'''triple'''"), "triple");
        assert_eq!(extract_string_literal("bare"), "bare");
    }

    #[test]
    fn test_helper_split_respecting_parens() {
        let parts = split_respecting_parens("a, b(c, d), e");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].trim(), "b(c, d)");
    }

    #[test]
    fn test_helper_parse_resources() {
        let res = parse_resources("mem_mb=32000, gpu=1");
        assert_eq!(res.len(), 2);
    }

    #[test]
    fn test_helper_parse_log_paths() {
        let paths = parse_log_paths(r#""a.log", "b.log""#);
        assert_eq!(paths, vec!["a.log", "b.log"]);
    }

    #[test]
    fn test_helper_parse_wildcard_constraints() {
        let wcs = parse_wildcard_constraints(r#"sample="[a-z]+""#);
        assert_eq!(wcs, vec![("sample".into(), "[a-z]+".into())]);
    }

    #[test]
    fn test_retries_deferred_escalation() {
        let input = r#"
rule step:
    output:
        "out.txt"
    retries: 3
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.escalations.len(), 1);
        assert_eq!(ir.escalations[0].tier, EscalationTier::MechanicalDeferred);
        assert_eq!(ir.escalations[0].severity, Severity::Correctness);
    }

    #[test]
    fn test_shadow_assisted_escalation() {
        let input = r#"
rule step:
    output:
        "out.txt"
    shadow:
        "minimal"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.escalations.len(), 1);
        assert_eq!(ir.escalations[0].tier, EscalationTier::Assisted);
    }

    #[test]
    fn test_min_version_info() {
        let input = r#"
min_version("8.8.0")

rule step:
    output:
        "out.txt"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            ir.diagnostics
                .iter()
                .any(|d| d.message.contains("Version constraint"))
        );
    }

    #[test]
    fn test_escalation_ids_are_unique() {
        let input = r#"
rule a:
    output:
        "out.txt"
    wrapper:
        "v1/tool"
    benchmark:
        "bench.txt"

rule b:
    output:
        "out2.txt"
    threads: get_threads()
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.escalations.len(), 3);
        let ids: Vec<&str> = ir.escalations.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids[0], "esc-0001");
        assert_eq!(ids[1], "esc-0002");
        assert_eq!(ids[2], "esc-0003");
    }

    #[test]
    fn test_named_outputs() {
        let input = r#"
rule align:
    output:
        bam="results/{sample}.bam",
        bai="results/{sample}.bam.bai"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(ir.rules[0].outputs[0].name.as_deref(), Some("bam"));
        assert_eq!(ir.rules[0].outputs[1].name.as_deref(), Some("bai"));
    }

    #[test]
    fn test_directory_modifier() {
        let input = r#"
rule index:
    output:
        directory("genome_index")
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert_eq!(
            ir.rules[0].outputs[0].lifecycle.as_deref(),
            Some("directory")
        );
    }

    #[test]
    fn test_message_mapped_to_description() {
        let input = r#"
rule step:
    output:
        "out.txt"
    message:
        "Processing step"
    shell:
        "echo"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            ir.diagnostics
                .iter()
                .any(|d| d.message.contains("message mapped to description"))
        );
        assert!(ir.escalations.is_empty()); // message is Tier 1
    }

    #[test]
    fn test_double_brace_escaping() {
        let input = r#"
rule format_output:
    output:
        "results/{sample}.txt"
    shell:
        "awk '{{print $1}}' {input} > {output}"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            matches!(&ir.rules[0].execution, ExecutionIR::Shell(cmd) if cmd == "awk '{print $1}' {input} > {output}")
        );
    }

    #[test]
    fn test_wildcards_dot_syntax() {
        let input = r#"
rule process:
    input:
        "data/{sample}.csv"
    output:
        "results/{sample}.txt"
    shell:
        "echo {wildcards.sample} && sort {input} > {output}"
"#;
        let ir = parse_snakefile(input).unwrap();
        assert!(
            matches!(&ir.rules[0].execution, ExecutionIR::Shell(cmd) if cmd == "echo {sample} && sort {input} > {output}")
        );
    }

    #[test]
    fn test_translate_shell_command_combined() {
        // Both transforms together
        let result =
            translate_shell_command("awk '{{print {wildcards.sample}}}' {input} > {output}");
        assert_eq!(result, "awk '{print {sample}}' {input} > {output}");
    }

    #[test]
    fn test_translate_shell_command_no_transforms() {
        // No double braces or wildcards.X — pass through unchanged
        let result = translate_shell_command("sort {input} > {output}");
        assert_eq!(result, "sort {input} > {output}");
    }

    #[test]
    fn test_unsupported_checkpoint_emits_error() {
        let input =
            "\ncheckpoint split:\n    output:\n        \"x.txt\"\n    shell:\n        \"echo\"\n";
        let ir = parse_snakefile(input).unwrap();
        let errs: Vec<_> = ir
            .diagnostics
            .iter()
            .filter(|d| d.level == DiagLevel::Error)
            .collect();
        assert_eq!(errs.len(), 1, "expected exactly one Error diagnostic");
        assert_eq!(
            errs[0].message,
            "Snakefile:2: unsupported construct 'checkpoint' (use --lossy to opt out)"
        );
    }

    #[test]
    fn test_unsupported_top_level_constructs_all_named() {
        for kw in &[
            "checkpoint",
            "module",
            "pepfile",
            "localrules",
            "ruleorder",
            "onsuccess",
            "onerror",
            "onstart",
        ] {
            let input = format!("\n{} foo:\n", kw);
            let ir = parse_snakefile(&input).unwrap();
            let err = ir
                .diagnostics
                .iter()
                .find(|d| d.level == DiagLevel::Error)
                .unwrap_or_else(|| panic!("no Error diagnostic for top-level '{}'", kw));
            assert!(
                err.message
                    .contains(&format!("unsupported construct '{}'", kw)),
                "message for '{}' was: {}",
                kw,
                err.message
            );
            assert!(err.message.contains("use --lossy to opt out"));
        }
    }

    #[test]
    fn test_unsupported_rule_level_directive_emits_error() {
        // `onsuccess` is one of the named unsupported keywords; placed as a
        // rule-level directive it must still trip the hard error path.
        let input = "\nrule r:\n    output:\n        \"out.txt\"\n    onsuccess:\n        \"echo\"\n    shell:\n        \"echo\"\n";
        let ir = parse_snakefile(input).unwrap();
        let err = ir
            .diagnostics
            .iter()
            .find(|d| d.level == DiagLevel::Error)
            .expect("expected Error diagnostic for rule-level onsuccess");
        assert!(
            err.message.contains("unsupported construct 'onsuccess'"),
            "got: {}",
            err.message
        );
        assert!(err.message.contains("use --lossy to opt out"));
    }

    #[test]
    fn test_unknown_top_level_still_warning_not_error() {
        let input = "\nfoo_bar_baz xyz\n";
        let ir = parse_snakefile(input).unwrap();
        assert!(
            ir.diagnostics.iter().all(|d| d.level != DiagLevel::Error),
            "unrecognized non-listed construct must stay Warning, not Error"
        );
        assert!(ir.diagnostics.iter().any(|d| d.level == DiagLevel::Warning));
    }
}
