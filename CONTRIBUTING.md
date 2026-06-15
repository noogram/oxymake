# Contributing to OxyMake

OxyMake is pre-1.0. Most surfaces are still consolidating. Before opening
a PR that touches a public surface, read `STATUS.md` — it tells you what
is *stable* (handle with care) and what is *unstable* (move freely, but
log it).

## Definition of Done

Every PR must pass these five gates locally before being merged:

| Gate | Command |
|------|---------|
| Build | `cargo check --workspace` |
| Test | `cargo test --workspace` |
| Lint | `cargo clippy --workspace -- -D warnings` |
| Docs | `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` |
| Format | `cargo fmt --all -- --check` |

CI enforces the same five. Do not bypass with `--no-verify`.

The `Docs` gate is easy to miss: the CI `doc` job fails on broken intra-doc
links, which `cargo check` does not catch. Run `just doc` (or `just ci`, which
includes it) before submitting so a doc-link break is caught locally, not in CI.

## Testing policy (TDD required)

All code changes must include tests, and `cargo test --workspace` is a hard
gate — do not submit work with failing tests.

### Bug fixes

Use test-driven development: write a failing test **before** implementing the
fix.

1. Write a test that reproduces the bug (must fail without the fix).
2. Run `cargo test --workspace` — confirm the new test fails.
3. Implement the fix.
4. Run `cargo test --workspace` — confirm all tests pass.

### New features

New functionality must include unit or integration tests covering the primary
behaviour paths.

- Unit tests go in `#[cfg(test)]` modules alongside the source.
- Integration tests go in `crates/<crate>/tests/`.
- Use existing test patterns in the crate as a guide.

### Refactors

Existing tests must continue to pass. If the refactor changes behaviour, update
the tests in the same commit. If no tests exist for the affected code, add them.

## Public surfaces (read this if you are touching `pub`)

OxyMake's seven public surfaces are catalogued in `STATUS.md`. Per
surface, the rule is:

1. **Stable surfaces** (e.g. `ox run --jobs`, the `job_completed` event
   name, `format_version`): breaking changes require a minor-version
   bump *and* an entry in `CHANGELOG.md` under **Breaking changes**.
2. **Unstable surfaces** (everything else): move freely, but still
   log every change in `CHANGELOG.md` under the appropriate section
   (`Added`, `Changed`, `Removed`).
3. **Adding** a new public surface: default to **unstable**. Add it to
   `STATUS.md` under the relevant section. Add a `CHANGELOG.md` entry.
   Document it in the relevant `docs/book/src/reference/` page
   (machine-facing reference) and `docs/book/src/` guide page
   (human-facing).

If you are unsure whether something is "public", ask yourself: *would a
downstream user notice if I removed this between two patch releases?*
If yes, treat it as public.

## CHANGELOG discipline

The repo follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
under [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

- Every PR that touches a public surface adds at least one bullet under
  `[Unreleased]` in `CHANGELOG.md`.
- Sections used: `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`,
  `Security`, and `Breaking changes` (which we surface separately to
  make it impossible to miss).
- Bullets are written for *users*, not for *committers*. Say what
  changed and what to do about it. Link the PR with `(#123)`.

## Coding conventions

- **Rust edition / version**: `1.85+` (see `Cargo.toml`).
- **No `unsafe`** in workspace code except in `#[cfg(test)]` blocks or
  with a `// SAFETY:` comment that explains the invariant.
- **No `println!`** in library crates — emit through a `Reporter` or
  through `tracing`.
- **No new dependencies** in `ox-core` without a PR comment justifying
  it. Other crates: prefer to depend on something already in the
  workspace.

## Shell automation (agents and scripts)

Automated tooling must use **non-interactive flags** with file operations.
`cp`, `mv`, and `rm` are aliased to `-i` (interactive) mode on some systems,
which hangs an unattended agent waiting for `y/n` input.

```bash
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file
rm -rf directory            # NOT: rm -r directory
```

Other commands that may prompt: `scp`/`ssh` (use `-o BatchMode=yes`), `apt-get`
(use `-y`), `brew` (use `HOMEBREW_NO_AUTO_UPDATE=1`).

## Commit and PR style

- Conventional commits encouraged but not enforced: `feat:`, `fix:`,
  `refactor:`, `docs:`, `test:`, `chore:`.
- The PR description must explain **why**, not just what — the diff
  already shows the what.
- Reference the issue or tracking ID if any, in parentheses after the summary.

## Local development

```bash
just build          # debug build
just test           # workspace tests
just clippy         # lint
just fmt            # format
just demo           # end-to-end smoke run
```

See `justfile` for the full list. If `just` is not installed, the same
commands are visible in that file and runnable directly.

## Architecture and design docs

- **ADRs** — long-lived architectural decisions live in
  `docs/adr/`. Read `docs/adr/README.md` for the index.
- **Design notes** — exploratory and time-boxed analyses live in
  `docs/design/`. They are *not* normative — an ADR supersedes a
  design note.
- **Book** — user-facing documentation in `docs/book/src/`.
- **Reference** — machine-facing reference (format, configuration,
  expressions, command pages) in `docs/book/src/reference/`.

## Filing issues

- Reproducible bug: include the `Oxymakefile.toml`, the `ox` command
  you ran, the version (`ox --version`), and the relevant log /
  `--report-json` output.
- Feature request: explain the use case before the proposed solution.
  A 30-line case study often shortcuts a 300-line design discussion.

## Where to ask

- Open an issue on GitHub for anything that should leave a trail.
- For exploratory questions, the design notes in `docs/design/` are
  the best context to read first.

---

OxyMake is built in the open, pre-1.0, by a small team. Be kind, be
specific, and be willing to defend the change you propose.
