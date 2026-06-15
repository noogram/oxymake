//! # ox-cli — OxyMake Command-Line Interface
//!
//! This is the library entry point for OxyMake CLI. It defines the CLI surface
//! and delegates to ox-api for all build logic. Both the `ox` and `oxymake`
//! binaries call [`run`] as their entry point.

use clap::{Parser, Subcommand};
use ox_render::{ColorMode, Theme};

mod commands;

#[derive(Parser)]
#[command(name = "ox", about = "OxyMake — workflow orchestration")]
#[command(version = version_string(), propagate_version = true)]
#[command(after_help = "\
Exit codes:
  0  success
  1  runtime error or job failure
  2  command-line usage error

Machine interfaces (for scripts and agents):
  Most subcommands accept --json for machine-readable output. `ox run --json`
  emits NDJSON events (one JSON object per line); `ox run --report-json <path>`
  writes that stream to a file; `ox subscribe --help` lists the event types.
  `ox lock generate` writes ox.lock, the reproducibility lockfile.")]
struct Cli {
    /// Color output mode
    #[arg(long, global = true, default_value = "auto")]
    color: ColorMode,

    #[command(subcommand)]
    command: Commands,
}

fn version_string() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Subcommand)]
enum Commands {
    /// Execute a workflow, ensuring requested outputs exist
    ///
    /// OxyMake writes all outputs atomically (temp-then-rename) so a crash never
    /// leaves partial files. Cache keys include content hashes of inputs.
    ///
    /// Scripts are responsible for domain validation: if a script exits 0,
    /// OxyMake trusts the outputs are valid and caches them.
    /// See docs/design/output-integrity.md for the full contract.
    Run(Box<commands::RunArgs>),
    /// Show the execution plan without running
    Plan(commands::PlanArgs),
    /// Show current execution status
    Status(commands::StatusArgs),
    /// Cancel running jobs
    Cancel(commands::CancelArgs),
    /// Invalidate cached outputs
    Invalidate(commands::InvalidateArgs),
    /// Show the full dependency chain for a target
    Explain(commands::ExplainArgs),
    /// Query the dependency graph (Bazel-style)
    ///
    /// Supports: deps(X), rdeps(X), allpaths(X, Y)
    Query(commands::QueryArgs),
    /// Visualize the DAG
    Dag(commands::DagArgs),
    /// View job logs
    Logs(commands::LogsArgs),
    /// List past runs
    History(commands::HistoryArgs),
    /// Manage snapshots
    Snapshot(commands::SnapshotArgs),
    /// Manage gates
    Gate(commands::GateArgs),
    /// Validate the Oxymakefile
    Lint(commands::LintArgs),
    /// Generate or verify a reproducibility lockfile
    Lock(commands::LockArgs),
    /// Initialize a new workflow
    Init(commands::InitArgs),
    /// Clean outputs and cache
    Clean(commands::CleanArgs),
    /// Start the MCP server for AI agent integration
    Serve(commands::ServeArgs),
    /// Subscribe to the event stream for an active session
    Subscribe(commands::SubscribeArgs),
    /// Live TUI dashboard for monitoring execution
    Top(commands::TopArgs),
    /// Web dashboard for monitoring and DAG visualization
    Dashboard(commands::DashboardArgs),
    /// Test and validate a workflow without executing it
    Test(commands::TestArgs),
    /// Check internal DAG invariants (orphans, shadows, overlaps)
    CheckConsistency(commands::CheckConsistencyArgs),
    /// Translate a workflow file (Snakemake or WDL) into OxyMake TOML
    Translate(commands::TranslateArgs),
    /// Export an Oxymakefile to another format (Snakemake or WDL)
    Export(commands::ExportArgs),
    /// Display the OxyMake ASCII art logo
    Logo,
    /// Print the operator handbook (orientation + pointers to the docs)
    ///
    /// A concise starting point for any operator (human or LLM): the one-line
    /// what, the core loop (`ox init` → `lint` → `plan` → `run` → `status` →
    /// `explain`), and where the canonical docs live. `ox guide` prints the
    /// same text to stdout.
    #[command(long_about = commands::HANDBOOK)]
    Guide,
}

/// Build the full clap `Command` tree for the `ox` CLI.
///
/// Used by the `gen-man` binary to render man pages from the same source of
/// truth as `--help`, and by tests that introspect the CLI surface.
pub fn command() -> clap::Command {
    use clap::CommandFactory;
    Cli::command()
}

/// Run the OxyMake CLI, parsing arguments from the environment.
///
/// Returns a process exit code: 0 on success, 1 on error
/// (clap itself exits 2 on a command-line usage error).
pub fn run() -> i32 {
    let cli = Cli::parse();
    let theme = Theme::from_env(Some(cli.color), &std::io::stderr());

    let result = match cli.command {
        Commands::Run(args) => commands::cmd_run(*args, &theme),
        Commands::Plan(args) => commands::cmd_plan(args, &theme),
        Commands::Status(args) => commands::cmd_status(args, &theme),
        Commands::Cancel(args) => commands::cmd_cancel(args),
        Commands::Invalidate(args) => commands::cmd_invalidate(args),
        Commands::Explain(args) => commands::cmd_explain(args),
        Commands::Query(args) => commands::cmd_query(args),
        Commands::Dag(args) => commands::cmd_dag(args, &theme),
        Commands::Logs(args) => commands::cmd_logs(args, &theme),
        Commands::History(args) => commands::cmd_history(args, &theme),
        Commands::Snapshot(args) => commands::cmd_snapshot(args),
        Commands::Gate(args) => commands::cmd_gate(args),
        Commands::Lint(args) => commands::cmd_lint(args),
        Commands::Lock(args) => commands::cmd_lock(args),
        Commands::Init(args) => commands::cmd_init(args),
        Commands::Clean(args) => commands::cmd_clean(args),
        Commands::Serve(args) => commands::cmd_serve(args),
        Commands::Subscribe(args) => commands::cmd_subscribe(args),
        Commands::Top(args) => commands::cmd_top(args),
        Commands::Dashboard(args) => commands::cmd_dashboard(args),
        Commands::Test(args) => commands::cmd_test(args),
        Commands::CheckConsistency(args) => commands::cmd_check_consistency(args),
        Commands::Translate(args) => commands::cmd_translate(args),
        Commands::Export(args) => commands::cmd_export(args),
        Commands::Logo => commands::cmd_logo(),
        Commands::Guide => commands::cmd_guide(),
    };

    if let Err(e) = result {
        eprintln!("error: {e:#}");
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify_cli() {
        Cli::command().debug_assert();
    }

    #[test]
    fn run_rejects_dead_flags() {
        // `--where` and `--materialize` were documented on `ox run` but never
        // wired to any behavior (silent no-ops). They must be rejected loudly
        // (exit 2) rather than accepted and ignored.
        assert!(Cli::try_parse_from(["ox", "run", "--where", "a=b"]).is_err());
        assert!(Cli::try_parse_from(["ox", "run", "--materialize", "final"]).is_err());
    }

    #[test]
    fn root_help_documents_exit_codes_and_machine_interfaces() {
        let mut cmd = Cli::command();
        let help = cmd.render_long_help().to_string();
        assert!(help.contains("Exit codes"), "missing exit codes section");
        assert!(
            help.contains("usage error"),
            "missing usage-error exit code"
        );
        assert!(help.contains("--json"), "missing machine-interface pointer");
        assert!(help.contains("ox.lock"), "missing lockfile pointer");
    }

    #[test]
    fn run_cache_validation_help_documents_resolution_chain() {
        let cmd = Cli::command();
        let run = cmd.find_subcommand("run").expect("run subcommand");
        let arg = run
            .get_arguments()
            .find(|a| a.get_id() == "cache_validation")
            .expect("--cache-validation arg");
        let help = arg.get_long_help().expect("long help").to_string();
        assert!(help.contains("mtime+hash"), "default strategy not named");
        assert!(help.contains("OX_CACHE_VALIDATION"), "env var not named");
        assert!(
            help.contains("oxymake/config.toml"),
            "global config file not named"
        );
    }

    #[test]
    fn man_page_renders_with_cache_validation_visible() {
        // The man pages are generated from the same clap definitions by the
        // `gen-man` binary; this guards that `ox-run.1` keeps the cache
        // validation default visible.
        let cmd = command();
        let run = cmd
            .find_subcommand("run")
            .expect("run subcommand")
            .clone()
            .name("ox-run");
        let mut buf = Vec::new();
        clap_mangen::Man::new(run).render(&mut buf).unwrap();
        // roff output escapes hyphens (`cache\-validation`); normalize first.
        let page = String::from_utf8(buf).unwrap().replace("\\-", "-");
        assert!(page.contains("cache-validation"));
        assert!(page.contains("mtime+hash"));
    }
}
