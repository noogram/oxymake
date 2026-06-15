use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Workflow IR
// ---------------------------------------------------------------------------

/// A complete workflow definition in intermediate form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowIR {
    pub rules: Vec<RuleIR>,
    pub config_file: Option<String>,
    pub includes: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
    /// Parsed config values (from inline Python or YAML conversion).
    #[serde(default)]
    pub config_values: Vec<ConfigEntryIR>,
    /// Structured escalations replacing # MANUAL: comments.
    #[serde(default)]
    pub escalations: Vec<Escalation>,
    /// Global directives that apply to all rules.
    #[serde(default)]
    pub global_container: Option<String>,
    #[serde(default)]
    pub global_report: Option<String>,
}

/// A config entry extracted from the Snakefile or config file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigEntryIR {
    pub key: String,
    pub values: Vec<String>,
}

// ---------------------------------------------------------------------------
// Rule IR
// ---------------------------------------------------------------------------

/// A single rule in intermediate form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleIR {
    pub name: String,
    pub inputs: Vec<PortIR>,
    pub outputs: Vec<PortIR>,
    pub execution: ExecutionIR,
    pub threads: Option<ThreadsIR>,
    pub resources: Vec<ResourceIR>,
    pub environment: Option<EnvironmentIR>,
    pub wildcard_constraints: Vec<(String, String)>,
    #[serde(default)]
    pub params: Vec<ParamIR>,
    #[serde(default)]
    pub log: Vec<String>,
    pub source_line: Option<usize>,
    /// Expand mode for wildcard expansion (e.g., "product" or "zip").
    #[serde(default)]
    pub expand: Option<String>,
}

/// An input or output port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PortIR {
    pub name: Option<String>,
    pub pattern: String,
    pub lifecycle: Option<String>,
}

/// A rule parameter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParamIR {
    pub name: String,
    pub value: String,
}

/// Thread count — either literal or dynamic expression.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ThreadsIR {
    Literal(u32),
    /// Dynamic expression (e.g. function call) — cannot be resolved statically.
    Dynamic(String),
}

/// How a rule executes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExecutionIR {
    Shell(String),
    Run(String),
    Script(String),
    Notebook(String),
    None,
}

/// A resource declaration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceIR {
    pub key: String,
    pub value: String,
}

/// Environment specification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EnvironmentIR {
    Conda(String),
    Container(String),
    Singularity(String),
}

// ---------------------------------------------------------------------------
// Escalation model
// ---------------------------------------------------------------------------

/// A structured escalation entry — replaces # MANUAL: comments.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Escalation {
    pub id: String,
    pub tier: EscalationTier,
    pub category: EscalationCategory,
    pub severity: Severity,
    pub rule_name: Option<String>,
    pub construct: String,
    pub original_code: String,
    pub source_line: Option<usize>,
    pub context: EscalationContext,
    pub instructions: EscalationInstructions,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EscalationTier {
    /// Mechanical but blocked on an OxyMake feature.
    MechanicalDeferred,
    /// Agent can resolve with structured instructions.
    Assisted,
    /// Requires human judgment.
    Human,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EscalationCategory {
    ConfigResolution,
    DynamicParam,
    DynamicInput,
    DynamicThreads,
    MissingFeature,
    SilentDrop,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Severity {
    Correctness,
    Performance,
    Cosmetic,
    Informational,
}

/// Structured context for agent resolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct EscalationContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_file: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub wildcards_used: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directive_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directive_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
}

/// Instructions for resolving an escalation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EscalationInstructions {
    pub summary: String,
    pub steps: Vec<String>,
    pub acceptance_criteria: Vec<String>,
}

// ---------------------------------------------------------------------------
// Diagnostics (kept for non-escalation messages)
// ---------------------------------------------------------------------------

/// A diagnostic message from parsing or translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: DiagLevel,
    pub message: String,
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DiagLevel {
    Info,
    Warning,
    Error,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

impl WorkflowIR {
    /// Resolve the `configfile:` directive by reading the referenced YAML file
    /// and populating `config_values`. If `config_values` is already populated
    /// (e.g. from inline Python assignments), this is a no-op.
    ///
    /// `base_dir` is the directory containing the Snakefile.
    pub fn resolve_config(&mut self, base_dir: &std::path::Path) -> anyhow::Result<()> {
        if !self.config_values.is_empty() {
            return Ok(());
        }
        if let Some(ref config_file) = self.config_file {
            self.config_values =
                crate::snakemake::configfile::resolve_configfile(config_file, base_dir)?;
        }
        Ok(())
    }
}

impl Escalation {
    /// Create a new escalation with a unique ID.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tier: EscalationTier,
        category: EscalationCategory,
        severity: Severity,
        rule_name: Option<&str>,
        construct: &str,
        original_code: &str,
        source_line: Option<usize>,
        instructions: EscalationInstructions,
    ) -> Self {
        // Simple incrementing ID based on hash of construct + rule
        let id_seed = format!(
            "{}:{}:{}",
            rule_name.unwrap_or("global"),
            construct,
            source_line.unwrap_or(0)
        );
        let hash = id_seed.len() * 31 + source_line.unwrap_or(0);
        Self {
            id: format!("esc-{:04x}", hash),
            tier,
            category,
            severity,
            rule_name: rule_name.map(String::from),
            construct: construct.into(),
            original_code: original_code.into(),
            source_line,
            context: EscalationContext::default(),
            instructions,
        }
    }

    /// Set context fields.
    pub fn with_context(mut self, ctx: EscalationContext) -> Self {
        self.context = ctx;
        self
    }
}

impl EscalationInstructions {
    pub fn new(summary: &str, steps: Vec<&str>, criteria: Vec<&str>) -> Self {
        Self {
            summary: summary.into(),
            steps: steps.into_iter().map(String::from).collect(),
            acceptance_criteria: criteria.into_iter().map(String::from).collect(),
        }
    }
}

impl ThreadsIR {
    /// Get the literal value if available.
    pub fn as_literal(&self) -> Option<u32> {
        match self {
            ThreadsIR::Literal(n) => Some(*n),
            ThreadsIR::Dynamic(_) => None,
        }
    }
}
