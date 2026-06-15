//! Generate man pages for the `ox` CLI.
//!
//! Renders one roff page per command from the same clap definitions that
//! drive `--help`, so the two surfaces cannot drift apart:
//!
//! - `ox.1` — the root command (subcommand list, global options, exit codes)
//! - `ox-<sub>.1` — one page per subcommand (e.g. `ox-run.1`)
//! - `ox-<sub>-<nested>.1` — nested subcommands (e.g. `ox-snapshot-create.1`)
//! - `oxymake.1` — the alias binary's root page, so `man oxymake` resolves
//!   too (the `oxymake` and `ox` binaries share the same CLI surface)
//!
//! Usage: `gen-man [OUT_DIR]` (default: `docs/man`). Wired into the release
//! flow via `just man` — see RELEASING.md.

use std::io::Write as _;
use std::path::{Path, PathBuf};

fn render(cmd: &clap::Command, name: &str, out_dir: &Path) -> std::io::Result<()> {
    // clap's `string` feature is off workspace-wide; Command::name needs a
    // 'static str. Leaking is fine in this short-lived generator.
    let page = cmd.clone().name(&*name.to_string().leak());
    let mut buf: Vec<u8> = Vec::new();
    clap_mangen::Man::new(page).render(&mut buf)?;
    let path = out_dir.join(format!("{name}.1"));
    std::fs::File::create(&path)?.write_all(&buf)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn render_tree(cmd: &clap::Command, name: &str, out_dir: &Path) -> std::io::Result<()> {
    render(cmd, name, out_dir)?;
    for sub in cmd.get_subcommands() {
        if sub.get_name() == "help" {
            continue;
        }
        render_tree(sub, &format!("{name}-{}", sub.get_name()), out_dir)?;
    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    let out_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs/man"));
    std::fs::create_dir_all(&out_dir)?;
    render_tree(&ox_cli::command(), "ox", &out_dir)?;
    // The `oxymake` alias binary ships alongside `ox` and shares the same CLI
    // surface; render its root page so `man oxymake` resolves (AGENTS.md points
    // there). Only the root page — subcommands are documented under `ox-<sub>`.
    render(&ox_cli::command(), "oxymake", &out_dir)
}
