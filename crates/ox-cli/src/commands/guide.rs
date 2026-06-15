use anyhow::Result;

/// The operator handbook text, shared by `ox guide` (printed to stdout) and
/// `ox help guide` (rendered by clap as the subcommand's long help).
///
/// Keep this concise and operator-oriented (human or LLM): it orients the
/// reader, then points at the canonical docs. It does NOT restate the
/// Definition of Done or other policy — that lives in CONTRIBUTING.md.
pub const HANDBOOK: &str = "\
OxyMake — operator handbook

OxyMake is a content-addressed workflow engine: declare rules in an
Oxymakefile.toml, and `ox` builds only what is stale, in dependency order.

The core loop:
  ox init      scaffold a new workflow (Oxymakefile.toml + .oxymake/)
  ox lint      validate the Oxymakefile before running
  ox plan      show what would execute, without running it
  ox run       execute the workflow, ensuring requested outputs exist
  ox status    inspect the current run (counts, failures, progress)
  ox explain   show the full dependency chain for a target

Where the canonical docs live:
  - CLI reference     `ox help` (per-command `ox help <cmd>`), `man ox`
                      (and `man oxymake`, the alias binary).
  - Concepts & guides the mdBook under docs/book/ (start at introduction.md).
  - Contributing      CONTRIBUTING.md — the single source of truth for the
                      Definition of Done and the contribution workflow.

Machine interfaces: most subcommands accept --json; `ox run --json` emits
NDJSON events. `ox serve --mcp` exposes the engine over the Model Context
Protocol (stdio) so an agent can drive `ox` directly. See `ox help` for the
full list.

Contributing? Read CONTRIBUTING.md for the Definition of Done and workflow.
";

pub fn cmd_guide() -> Result<()> {
    println!("{HANDBOOK}");
    Ok(())
}
