# OxyMake development recipes
# Install just: cargo install just
#
# SLURM recipes moved to infra/ — see:
#   just -f infra/setup/slurm-docker/justfile --list

# Default: show available recipes
default:
    @just --list

# --- Build -------------------------------------------------------------------

# Build in debug mode
[group('build')]
build:
    cargo build --workspace

# Build in release mode
[group('build')]
release:
    cargo build --release --workspace

# Install ox and oxymake to ~/.cargo/bin/
[group('build')]
install:
    cargo install --path crates/ox-cli

# Install ox globally from source (with --force to overwrite existing)
[group('build')]
ox-install:
    cargo install --path crates/ox-cli --force

# Install ox globally and verify the installed version
[group('build')]
ox-install-check:
    cargo install --path crates/ox-cli --force
    ox --version

# Regenerate man pages into docs/man/ from the clap definitions
[group('build')]
man:
    cargo run --release -p ox-cli --bin gen-man -- docs/man

# Install man pages so `man ox` / `man ox-run` work (may need sudo)
[group('build')]
man-install: man
    mkdir -p /usr/local/share/man/man1
    cp docs/man/*.1 /usr/local/share/man/man1/

# Update ox to latest (git pull + release build + install)
[group('build')]
ox-update:
    git pull
    cargo build --release --bin ox
    cargo install --path crates/ox-cli --force
    @echo 'Updated to:' && ox --version

# --- Test & Lint -------------------------------------------------------------

# Run all tests
[group('test')]
test:
    cargo test --workspace

# Full CI check (build + fmt + lint + test + doc + demo)
[group('test')]
ci: check fmt-check lint test doc demo-ci

# Type-check the workspace without producing binaries (gate #1 in CONTRIBUTING)
[group('test')]
check:
    cargo check --workspace

# Run clippy lints
[group('test')]
lint:
    cargo clippy --workspace -- -D warnings

# Build the docs with broken-intra-doc-links as errors (mirrors the CI `doc` job)
[group('test')]
doc:
    RUSTDOCFLAGS="-Dwarnings" cargo doc --workspace --no-deps

# Format code
[group('test')]
fmt:
    cargo fmt --all

# Check formatting without modifying
[group('test')]
fmt-check:
    cargo fmt --all -- --check

# --- Coverage ----------------------------------------------------------------

# Generate coverage report (requires cargo-llvm-cov)
[group('test')]
coverage:
    cargo llvm-cov --workspace --lcov --output-path lcov.info
    cargo llvm-cov report --fail-under-lines 80

# Generate HTML coverage report and open in browser
[group('test')]
coverage-html:
    cargo llvm-cov --workspace --html
    cargo llvm-cov report --fail-under-lines 80
    @echo "Report: target/llvm-cov/html/index.html"

# Check new code coverage against main (requires diff-cover)
[group('test')]
coverage-diff:
    cargo llvm-cov --workspace --lcov --output-path lcov.info
    diff-cover lcov.info --compare-branch=origin/main --fail-under=95

# --- Demo --------------------------------------------------------------------

# Run the interactive demo
[group('demo')]
demo:
    cargo build --bin ox
    OX="{{justfile_directory()}}/target/debug/ox" bash examples/demo/run-demo.sh

# Run the demo non-interactively (for CI)
[group('demo')]
demo-ci:
    cargo build --bin ox
    OX="{{justfile_directory()}}/target/debug/ox" bash examples/demo/run-demo.sh < /dev/null

# Run the dashboard demo (pipeline + web dashboard + browser)
[group('demo')]
demo-dashboard:
    cargo build --bin ox
    OX="{{justfile_directory()}}/target/debug/ox" bash examples/demo/run-dashboard-demo.sh

# Guided terminal walkthrough with explanations
[group('demo')]
demo-guided:
    cargo build --bin ox
    OX="{{justfile_directory()}}/target/debug/ox" bash examples/guided-tour.sh

# --- Benchmark ---------------------------------------------------------------

# Run DAG resolution benchmark (builds release binary first)
[group('benchmark')]
benchmark *sizes:
    cargo build --release
    bash benchmark/perf/run.sh {{sizes}}

# Quick benchmark — 1K jobs only (fast feedback loop)
[group('benchmark')]
benchmark-quick:
    cargo build --release
    bash benchmark/perf/run.sh 1000

# Snakemake compatibility suite (requires snakemake)
[group('benchmark')]
benchmark-snakemake:
    cargo build --release
    bash benchmark/run_benchmark.sh

# --- QA ---------------------------------------------------------------------

# Run all QA scenarios (builds debug binary first)
[group('qa')]
qa-all: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh

# QA: fresh clone → build → run -vv
[group('qa')]
qa-fresh-install: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh fresh-install

# QA: external results_dir via config
[group('qa')]
qa-external-results-dir: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh external-results-dir

# QA: second run hits cache, zero re-execution
[group('qa')]
qa-cache-reuse: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh cache-reuse

# QA: remove data file, re-run rebuilds chain
[group('qa')]
qa-missing-data: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh missing-data

# QA: corrupt output file, re-run detects and rebuilds
[group('qa')]
qa-corrupted-output: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh corrupted-output

# QA: nuke all outputs, full rebuild
[group('qa')]
qa-delete-all-results: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh delete-all-results

# QA: parallel execution -j1 vs -j4 vs -j8
[group('qa')]
qa-parallel: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh parallel-j1
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh parallel-j4
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh parallel-j8

# QA: ox run (no args) uses rule.all default target
[group('qa')]
qa-default-target: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh default-target

# QA: verbose modes (no flag, -v, -vv)
[group('qa')]
qa-verbose-modes: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh verbose-modes

# QA: --set results_dir override
[group('qa')]
qa-config-override: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh config-override

# QA: broken script gives clear error
[group('qa')]
qa-error-reporting: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh error-reporting

# QA: --dry-run shows plan, creates nothing
[group('qa')]
qa-dry-run: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh dry-run

# QA: partial state, resume rebuilds only missing outputs
[group('qa')]
qa-interrupted-resume: build
    OX="{{justfile_directory()}}/target/debug/ox" bash tests/qa/run-qa.sh interrupted-resume

# QA: stress — run all scenarios (alias for qa-all)
[group('qa')]
qa-stress-all: qa-all

# List available QA scenarios
[group('qa')]
qa-list:
    @bash tests/qa/run-qa.sh --list

# --- Paper -------------------------------------------------------------------

# Build the paper PDF
[group('paper')]
paper-build:
    cd docs/paper && latexmk -pdf oxymake-paper.tex

# Clean paper build artifacts
[group('paper')]
paper-clean:
    cd docs/paper && latexmk -C oxymake-paper.tex

# Watch and continuously recompile paper on changes
[group('paper')]
paper-watch:
    cd docs/paper && latexmk -pvc -pdf oxymake-paper.tex

# --- Dashboard ---------------------------------------------------------------

# Start the web dashboard (requires a prior ox run in a project)
[group('dashboard')]
dashboard db=".oxymake/state.db":
    cargo run --bin ox -- dashboard --db {{db}}

# Start the TUI with mock data
[group('dashboard')]
top-mock:
    cargo run --bin ox -- top --mock

# --- Docs site ---------------------------------------------------------------
# The documentation site (oxymake.noogram.dev) is mdBook → Cloudflare Pages.
# Runbook + deploy procedure: docs/WEBDOCS.md. Install mdBook: cargo install mdbook

# Build the documentation site into docs/book/book/
[group('docs')]
docs-build:
    mdbook build docs/book

# Serve the docs locally with live reload (http://localhost:3000)
[group('docs')]
docs-serve:
    mdbook serve docs/book --open

# Remove the generated docs site output
[group('docs')]
docs-clean:
    mdbook clean docs/book

# Manually deploy the docs site to Cloudflare Pages (phase 2 — see docs/WEBDOCS.md)
[group('docs')]
docs-deploy: docs-build
    wrangler pages deploy docs/book/book --project-name=oxymake-docs

# --- Release -----------------------------------------------------------------

# The single turnkey entrypoint: preflight gates → man pages → bump → commit →
# tag v<version> → push (CI re-gates, builds the `ox` binary for Linux + macOS,
# cuts the GitHub Release with checksums). crates.io is NOT touched here — the
# `oxymake` reservation crate is a one-shot manual publish (see RELEASING.md).
# macOS sed syntax (operator is on darwin).
#
# Turnkey release: preflight → bump → tag → push → CI builds + GitHub Release
[group('release')]
tag-release version:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== Releasing v{{version}} ==="
    # Guard: clean working tree
    if [ -n "$(git status --porcelain)" ]; then
        echo "ERROR: working tree is dirty. Commit or stash first."; exit 1
    fi
    # Guard: on main
    BRANCH=$(git branch --show-current)
    if [ "$BRANCH" != "main" ]; then
        echo "ERROR: not on main (on $BRANCH)"; exit 1
    fi
    # Guard: tag must not already exist (no accidental re-tag / overwrite)
    if git rev-parse -q --verify "refs/tags/v{{version}}" >/dev/null; then
        echo "ERROR: tag v{{version}} already exists. Bump the version or delete the tag."; exit 1
    fi
    # Guard: CHANGELOG must carry an entry for this version (Keep a Changelog
    # discipline — release notes are written BEFORE the tag, never after).
    if ! grep -qE "^## \[{{version}}\]" CHANGELOG.md; then
        echo "ERROR: CHANGELOG.md has no '## [{{version}}]' section."
        echo "       Move the [Unreleased] items under '## [{{version}}] - <date>' first."; exit 1
    fi
    echo "→ Preflight: test / clippy / fmt / doc"
    cargo test --workspace --quiet
    cargo clippy --workspace -- -D warnings
    cargo fmt --all -- --check
    RUSTDOCFLAGS="-Dwarnings" cargo doc --workspace --no-deps --quiet
    echo "→ Regenerating man pages (docs/man/*.1 from the clap definitions)"
    just man
    echo "→ Setting workspace version to {{version}}"
    sed -i '' 's/^version = ".*"/version = "{{version}}"/' Cargo.toml
    cargo check --workspace --quiet
    git add -A
    git diff --cached --quiet || git commit -m "release: v{{version}}"
    git tag -a "v{{version}}" -m "Release v{{version}}"
    echo "→ Pushing main + tag (triggers release workflow)"
    git push origin main
    git push origin "v{{version}}"
    echo "✓ Released v{{version}} — watch https://github.com/noogram/oxymake/actions"
    echo "  Reminder: the crate name 'oxymake' on crates.io is a one-shot manual"
    echo "  reservation (RELEASING.md). It is NOT published by this recipe or CI."
