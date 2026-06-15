# Installation

OxyMake is a single binary called `ox`, written in Rust. There are several
ways to install it.

## Install (from source)

```bash
git clone https://github.com/noogram/oxymake.git
cd oxymake
cargo install --path crates/ox-cli
```

This installs both `ox` and `oxymake` to `~/.cargo/bin/`. Make sure this
directory is in your `$PATH`.

## Development setup

For working on OxyMake itself:

```bash
git clone https://github.com/noogram/oxymake.git
cd oxymake
cargo build                    # debug build → target/debug/ox
cargo test --workspace         # run all tests
cargo run --bin ox -- --help   # run without installing
```

With [just](https://github.com/casey/just) (recommended):

```bash
just build      # debug build
just test       # all tests
just demo       # interactive feature demo
just lint       # clippy checks
just ci         # full CI check (fmt + lint + test + demo)
just --list     # all available recipes
```

## Prerequisites

### Required

- **Rust 1.85+** (for installation from source)

### Optional (depending on your workflow)

- **Python 3.9+** -- for rules using `lang = "python"`
- **uv** -- for `environment = { uv = "pyproject.toml" }` ([install uv](https://docs.astral.sh/uv/getting-started/installation/))
- **conda/mamba** -- for `environment = { conda = "..." }`
- **Docker** -- for `environment = { docker = "..." }`
- **Nix** -- for `environment = { nix = "..." }`

## Verify Installation

```bash
ox --version
# ox 0.1.0

ox init
# Initialized OxyMake project in .
#   Created: Oxymakefile.toml
#   Created: .oxymake/
```

## What Gets Installed

OxyMake is a single binary with no runtime dependencies. All state is
stored in a `.oxymake/` directory within your project:

```
your-project/
  Oxymakefile.toml       # Your workflow definition
  .oxymake/
    state.db             # SQLite execution state
    cache/               # Content-addressable cache
    logs/                # Job execution logs
```

No daemon, no server, no background processes. Each `ox run` is a
self-contained process that reads state, executes, writes state, and exits.

## Next Steps

Now that OxyMake is installed, head to
[Your First Workflow](./first-workflow.md) to build something.
