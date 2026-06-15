//! Implementation of the `ox translate` command.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};

use ox_translate::ir::{DiagLevel, Diagnostic, WorkflowIR};
use ox_translate::oxymake::generator::{generate_escalation_file, generate_oxymakefile};
use ox_translate::snakemake::parser::parse_snakefile;
use ox_translate::wdl::parser::parse_wdl;

/// Exit code returned when translation completed but escalations were
/// recorded. The two output files are still written; this lets CI gate on a
/// non-zero status without parsing the summary line.
const EXIT_ESCALATIONS_PRESENT: i32 = 2;

#[derive(clap::Args)]
pub struct TranslateArgs {
    /// Path to the workflow file to translate (Snakefile or .wdl)
    pub input: String,

    /// Write the translated TOML to this file. The escalation file is
    /// written alongside as `<OUTPUT>.escalations.toml` when escalations
    /// exist. When omitted, the default sink is
    /// `<INPUT>.translated.toml` (and `<INPUT>.translated.toml.escalations.toml`).
    #[arg(short = 'o', long)]
    pub output: Option<String>,

    /// Source format (auto-detected from file extension if omitted)
    #[arg(long, value_enum)]
    pub from: Option<SourceFormat>,

    /// Continue translating even when unsupported Snakemake constructs are
    /// encountered (checkpoint, module, pepfile, localrules, ruleorder,
    /// onsuccess, onerror, onstart). Without this flag, such constructs make
    /// `ox translate` exit non-zero without writing output.
    #[arg(long)]
    pub lossy: bool,
}

#[derive(clap::ValueEnum, Clone)]
pub enum SourceFormat {
    /// Snakemake format
    Snakemake,
    /// WDL (Workflow Description Language) format
    Wdl,
}

pub fn cmd_translate(args: TranslateArgs) -> Result<()> {
    let input_path = PathBuf::from(&args.input);
    let content = std::fs::read_to_string(&input_path)
        .with_context(|| format!("failed to read {}", input_path.display()))?;

    let format = args
        .from
        .unwrap_or_else(|| detect_format(&input_path, &content));

    let mut ir = match format {
        SourceFormat::Snakemake => parse_snakefile(&content)
            .with_context(|| format!("failed to parse Snakefile {}", input_path.display()))?,
        SourceFormat::Wdl => parse_wdl(&content)
            .with_context(|| format!("failed to parse WDL file {}", input_path.display()))?,
    };

    if matches!(format, SourceFormat::Snakemake) {
        let base_dir = input_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        if let Err(e) = ir.resolve_config(base_dir) {
            eprintln!("warning: could not resolve configfile: {e}");
        }
    }

    let toml = generate_oxymakefile(&ir);

    for diag in &ir.diagnostics {
        eprintln!("{:?}: {}", diag.level, diag.message);
    }

    // Fail loudly on unsupported constructs unless --lossy opts out.
    // This runs before writing any output: rejected Snakefiles produce no
    // translated TOML on disk.
    if !args.lossy {
        if let Some(err) = ir.diagnostics.iter().find(|d| d.level == DiagLevel::Error) {
            return Err(anyhow!("{}", err.message));
        }
    }

    let output_path = match args.output {
        Some(p) => PathBuf::from(p),
        None => default_output_path(&input_path),
    };
    let escalation_path = escalation_path_for(&output_path);

    std::fs::write(&output_path, &toml)
        .with_context(|| format!("failed to write {}", output_path.display()))?;

    if let Some(esc_content) = generate_escalation_file(&ir) {
        std::fs::write(&escalation_path, esc_content)
            .with_context(|| format!("failed to write {}", escalation_path.display()))?;
    }

    let summary = build_summary(&ir);
    eprintln!(
        "wrote {} ({})",
        output_path.display(),
        if ir.escalations.is_empty() {
            "no escalations".to_string()
        } else {
            format!(
                "{} escalation(s) → {}",
                ir.escalations.len(),
                escalation_path.display()
            )
        }
    );
    eprintln!("{summary}");

    if !ir.escalations.is_empty() {
        std::process::exit(EXIT_ESCALATIONS_PRESENT);
    }

    Ok(())
}

/// Default output path when `-o` is not supplied: `<input>.translated.toml`.
fn default_output_path(input: &std::path::Path) -> PathBuf {
    let mut name = input.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(".translated.toml");
    input.with_file_name(name)
}

/// Escalation file lives alongside the main output: `<output>.escalations.toml`.
fn escalation_path_for(output: &std::path::Path) -> PathBuf {
    let mut name = output.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(".escalations.toml");
    output.with_file_name(name)
}

/// Build the summary line emitted on every run.
///
/// Format: `translated: N rules (X mechanical, Y with escalations); dropped:
/// Z unsupported top-level constructs; includes: K files NOT followed`
fn build_summary(ir: &WorkflowIR) -> String {
    let n = ir.rules.len();
    let y = count_rules_with_escalations(ir);
    let x = n.saturating_sub(y);
    let z = count_dropped_top_level(&ir.diagnostics) + count_top_level_escalations(ir);
    let k = ir.includes.len();
    format!(
        "translated: {n} rules ({x} mechanical, {y} with escalations); \
         dropped: {z} unsupported top-level constructs; \
         includes: {k} files NOT followed"
    )
}

fn count_rules_with_escalations(ir: &WorkflowIR) -> usize {
    let names: HashSet<&str> = ir
        .escalations
        .iter()
        .filter_map(|e| e.rule_name.as_deref())
        .collect();
    names.len()
}

/// Top-level escalations: those without a rule name, e.g. the `report:`
/// directive that lives outside any `rule` block.
fn count_top_level_escalations(ir: &WorkflowIR) -> usize {
    ir.escalations
        .iter()
        .filter(|e| e.rule_name.is_none())
        .count()
}

/// Diagnostics generated by the top-level parser when a construct couldn't
/// be translated. Filters by message shape rather than diagnostic level so
/// rule-internal info notes don't inflate the count.
fn count_dropped_top_level(diags: &[Diagnostic]) -> usize {
    diags
        .iter()
        .filter(|d| {
            let m = &d.message;
            m.contains("not translatable")
                || m.contains("Unrecognized top-level")
                || m.contains("Version constraint dropped")
                || m.contains("not yet translated")
        })
        .count()
}

/// Auto-detect the source format from the file extension and content.
fn detect_format(path: &std::path::Path, content: &str) -> SourceFormat {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match ext {
        "wdl" => SourceFormat::Wdl,
        "smk" => SourceFormat::Snakemake,
        _ => {
            if content.trim_start().starts_with("version ") {
                SourceFormat::Wdl
            } else {
                SourceFormat::Snakemake
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(msg: &str) -> Diagnostic {
        Diagnostic {
            level: ox_translate::ir::DiagLevel::Warning,
            message: msg.into(),
            line: None,
        }
    }

    #[test]
    fn default_output_path_appends_translated_toml() {
        let p = default_output_path(std::path::Path::new("/tmp/Snakefile"));
        assert_eq!(p, PathBuf::from("/tmp/Snakefile.translated.toml"));
    }

    #[test]
    fn default_output_path_handles_dotted_input() {
        let p = default_output_path(std::path::Path::new("foo/bar.smk"));
        assert_eq!(p, PathBuf::from("foo/bar.smk.translated.toml"));
    }

    #[test]
    fn escalation_path_sits_next_to_output() {
        let p = escalation_path_for(std::path::Path::new("out/Oxymakefile.toml"));
        assert_eq!(p, PathBuf::from("out/Oxymakefile.toml.escalations.toml"));
    }

    #[test]
    fn dropped_counter_filters_to_top_level_messages() {
        let diags = vec![
            diag("Python import not translatable: from os import ..."),
            diag("Unrecognized top-level construct: foo"),
            diag("Rule 'process': unknown directive 'shadow'"),
            diag("expand() references config[\"samples\"] — ensure ..."),
            diag("Version constraint dropped: min_version(7.0)"),
            diag("Global wildcard_constraints found (not yet translated)"),
        ];
        assert_eq!(count_dropped_top_level(&diags), 4);
    }

    #[test]
    fn summary_shape_matches_spec() {
        let ir = WorkflowIR {
            rules: Vec::new(),
            config_file: None,
            includes: vec!["common.smk".into(), "utils.smk".into()],
            diagnostics: vec![diag("Unrecognized top-level construct: x")],
            config_values: Vec::new(),
            escalations: Vec::new(),
            global_container: None,
            global_report: None,
        };
        let s = build_summary(&ir);
        assert!(s.starts_with("translated: 0 rules (0 mechanical, 0 with escalations); "));
        assert!(s.contains("dropped: 1 unsupported top-level constructs"));
        assert!(s.contains("includes: 2 files NOT followed"));
    }
}
