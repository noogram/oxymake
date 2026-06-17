# Releasing OxyMake

The standing release procedure for OxyMake — a 24-crate Rust workspace that
ships as a single `ox` binary. Modeled on the biblion template
(`dev/ESERIE/biblion`: `release.yml` + `just release`) and adapted to OxyMake's
binary-first distribution. Follow it top to bottom; you should not have to think.

> **One command does the release:**
>
> ```bash
> just tag-release X.Y.Z
> ```
>
> It runs every preflight gate, regenerates man pages, bumps the version,
> commits, tags `vX.Y.Z`, and pushes. Pushing the tag triggers CI
> (`.github/workflows/release.yml`), which re-runs the gates and publishes a
> GitHub Release with prebuilt binaries + checksums. **You write the CHANGELOG
> entry first; the recipe does the rest.**

---

## The recurring release procedure

Three actors: **you** (write the notes, pick the version), **`just tag-release`**
(the turnkey local step), **CI** (build + publish). Nothing is done twice.

### Step 0 — you: prepare the CHANGELOG and pick the version

1. Decide the version bump from the nature of the changes (see **SemVer** below).
2. In `CHANGELOG.md`, rename the `## [Unreleased]` section to
   `## [X.Y.Z] - YYYY-MM-DD` and start a fresh empty `## [Unreleased]` above it.
   The recipe **refuses to tag** if `CHANGELOG.md` has no `## [X.Y.Z]` heading —
   the release notes are written *before* the tag, never after.
3. Commit the CHANGELOG on `main` (or include it in the same PR you are about to
   merge). The working tree must be clean before `tag-release`.

### Step 1 — you: run the one command

```bash
just tag-release X.Y.Z
```

In order, the recipe (see `justfile`, `[release]` group):

| # | What it does | Why it's a gate |
|---|--------------|-----------------|
| 1 | working tree clean | a release must be reproducible from a known commit |
| 2 | on `main` | releases are cut from `main`, never a feature branch |
| 3 | tag `vX.Y.Z` does not already exist | no accidental re-tag / silent overwrite |
| 4 | `CHANGELOG.md` has a `## [X.Y.Z]` section | Keep a Changelog discipline (Step 0) |
| 5 | `cargo test --workspace` | no untested release |
| 6 | `cargo clippy --workspace -- -D warnings` | lint floor |
| 7 | `cargo fmt --all -- --check` | format floor |
| 8 | `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` | doc floor — broken intra-doc links fail CI but slip past `cargo check` |
| 9 | `just man` regenerates `docs/man/*.1` | tracked man pages stay in sync with the CLI |
| 10 | bump `[workspace.package] version` → `X.Y.Z`, `cargo check` | one version source of truth |
| 11 | commit `release: vX.Y.Z`, annotated tag `vX.Y.Z` | the tag points at the release commit |
| 12 | `git push origin main` then `git push origin vX.Y.Z` | the tag push triggers CI |

If any gate fails the recipe stops with a non-zero exit and **no tag is
created** — fix and re-run. Re-running after a partial failure is safe: every
guard is idempotent and the tag-exists guard prevents double-tagging.

### Step 2 — CI: build and publish (automatic)

The pushed `v*` tag triggers `.github/workflows/release.yml`. Its jobs run in
sequence; a failure in one blocks the next:

1. **`gate`** — re-runs `cargo test`, `cargo clippy -D warnings`, and
   `cargo deny check advisories` on a frozen toolchain. *A `v*` tag can never
   ship an untested binary, even if the local recipe were bypassed.*
2. **`build`** (matrix) — builds `cargo build --release --bin ox` for each
   target, tars the binary, and emits a per-tarball `.sha256`.
3. **`release`** — downloads all artifacts, generates a combined `SHA256SUMS`,
   and creates the GitHub Release with auto-generated notes + every tarball,
   per-file checksum, and `SHA256SUMS` attached.

**Build matrix (current):**

| Target | Runner |
|--------|--------|
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` |
| `aarch64-apple-darwin` | `macos-latest` |
| `x86_64-apple-darwin` | `macos-latest` |

> `aarch64-unknown-linux-gnu` is **not** built today. OxyMake uses `rustls-tls`
> (no OpenSSL cross-compile blocker, unlike biblion), so it *can* be added — but
> it needs a cross linker (`cross` or `cargo-zigbuild`) in CI. Adding it is a
> deliberate, documented future change to the matrix, not a silent gap. Until
> then, ARM Linux users build from source (`cargo install --path crates/ox-cli`).

### Step 3 — you: verify

1. Watch the run: <https://github.com/noogram/oxymake/actions>.
2. Confirm the Release at <https://github.com/noogram/oxymake/releases> has the
   three tarballs + `SHA256SUMS`.
3. Spot-check one binary: download, `tar xzf`, `./ox --version` → `X.Y.Z`.

That is the whole release. The sections below are reference: the version
discipline, the changelog discipline, the crates.io decision, and the one-time
setup that does not repeat.

---

## SemVer discipline

OxyMake is `0.y.z` (pre-1.0): the **minor** acts as the breaking-change axis.

| Bump | When | Pre-1.0 mapping |
|------|------|-----------------|
| **major** (`1.0.0`) | first stable public API commitment | not yet |
| **minor** (`0.Y.0`) | breaking change to any **stable** public surface, OR a notable new feature | the breaking axis while `0.y` |
| **patch** (`0.y.Z`) | backward-compatible fix, internal-only change, docs | safe upgrade |

The seven public surfaces and their stability tiers are declared in
[`STATUS.md`](STATUS.md) — that document, not intuition, decides whether a
change is "breaking". A change to an **unstable** surface does not force a minor
bump, but must be noted in the CHANGELOG. The single version lives in
`[workspace.package].version`; all 23 library crates inherit it. The `oxymake`
reservation crate is the one exception (see crates.io below).

---

## Keep a Changelog discipline

`CHANGELOG.md` follows [Keep a Changelog 1.1.0](https://keepachangelog.com).

- New entries accumulate under `## [Unreleased]` as work merges, grouped into
  `Added` / `Changed` / `Deprecated` / `Removed` / `Fixed` / `Security`.
- At release time, `Unreleased` is renamed to `## [X.Y.Z] - YYYY-MM-DD` (Step 0).
- `just tag-release` enforces the heading exists; it does not write the notes —
  that is a human judgement about what matters to users.

---

## crates.io — Option A, reservation published manually (one-shot)

**Decision: Option A.** OxyMake ships as the `ox` binary via GitHub Releases.
The `oxymake` *name* is reserved on crates.io by a thin placeholder crate
(`crates/oxymake`, `publish = true`, pinned at `0.0.0`). The 23 library crates
stay `publish = false` (inherited from `[workspace.package]`); `ox-metrics` is
workspace-excluded and also `publish = false`.

Why not the other two options:

| Option | What it would mean | Verdict |
|--------|--------------------|---------|
| **A. Reserve name + ship binary** | placeholder crate holds the name; binary via Releases; internals unpublished | **chosen** — low cost, no semver lock on internals |
| B. Publish only `ox-cli` | impossible: `ox-cli` path-deps on 23 unpublished crates → collapses into C | N/A |
| C. Publish all 24 crates | topological publish; every internal becomes public API under semver; every `ox-core` refactor is a breaking release | **deferred** — only if/when the public API stabilises |

### Why the reservation is a manual one-shot, not a CI job

biblion publishes its crates (`biblion`, `paper-resolver`) from CI on every tag
because **those crates are the product** — each tag is a new published version.
OxyMake's product is the binary; the `oxymake` crate is a static placeholder
published **once**.

- Automating it on every tag means a `cargo publish` job that **fails on every
  release after the first** (version `0.0.0` already exists), forcing a
  `continue-on-error: true` — exactly the yellow-CI smell biblion lives with.
- It would put `CARGO_TOKEN` into the tag-triggered path, widening the
  supply-chain surface the `release.yml` premortem deliberately hardened (no
  secrets in the build/release path; SHA-pinned actions; frozen toolchain).

So the reservation is a documented one-shot, run by the operator once:

```bash
# One time only, from a clean checkout on main:
cargo publish -p oxymake          # publishes crates/oxymake (the name holder)
# Verify: https://crates.io/crates/oxymake
```

If Option C is ever adopted, *that* is when a CI publish job (with the index-sync
`sleep` and topological order, biblion-style) gets added — a separate, deliberate
decision, not the default.

---

## Man pages

`docs/man/*.1` are tracked and regenerated from the clap definitions by
`just man` (which `tag-release` runs, so the tagged tree is always current).
They are **not** bundled in the release tarballs (binary-only tarballs keep the
`ox` path predictable for `cargo-binstall` and `curl | sh` installers). Users
who clone get `man ox` / `man ox-run` via `just man-install`.

---

## Rollback / troubleshooting

- **Recipe failed before the tag** — nothing was pushed. Fix the cause and
  re-run `just tag-release X.Y.Z`.
- **Tag pushed but CI failed** — fix forward with a patch release (`X.Y.Z+1`).
  Do not delete and re-push a tag that a Release already consumed; deleting a
  public tag breaks anyone who pinned it.
- **Wrong notes in the GitHub Release** — edit the Release body on GitHub
  (generated notes are a starting point); the immutable record is `CHANGELOG.md`.
- **`tag-release` says CHANGELOG has no entry** — you skipped Step 0; add the
  `## [X.Y.Z]` heading and commit.

---

## One-time setup (reference — does not repeat each release)

These were done for the initial public launch and are recorded here so a future
fork or re-launch can reproduce them.

### Repo flip private → public (sequencing law)

> The first release tag must be created **after** the history purge
> (`git filter-repo`). filter-repo rewrites commit SHAs; a tag created before
> the purge points at an orphaned commit. Tag on the clean public history.

The flip is gated by the command-backed checklist, not by opinion:

```bash
scripts/release-checklist.sh             # pre-flip GATEs must exit 0
# … flip the repo public, then:
scripts/apply-branch-protection.sh       # wire required checks (post-flip only)
scripts/release-checklist.sh --post-flip # post-flip GATEs must exit 0
```

See [`docs/RELEASE-CHECKLIST.md`](docs/RELEASE-CHECKLIST.md) for the full
exogenous-gate discipline (janis: a self-ticked checklist is decoration).

### GitHub repo metadata — the "About" description

The repo's one-line **About** description (the sidebar blurb GitHub shows next
to the repo name and surfaces in search) is set with `gh repo edit`. After a
fresh repo creation / recreation, set it to the canonical wording — exactly:

```bash
gh repo edit noogram/oxymake \
  --description "Next-generation workflow orchestration in Rust"
```

This is the official short description; keep it byte-for-byte identical to the
tagline in `CLAUDE.md` ("next-generation workflow orchestration") so the About
blurb, the repo, and the docs tell one story. Optionally set the homepage and
topics in the same gesture:

```bash
gh repo edit noogram/oxymake \
  --homepage "https://oxymake.noogram.dev" \
  --add-topic workflow-engine --add-topic rust --add-topic dag \
  --add-topic reproducibility --add-topic content-addressable
```

### Name reservation — broader than crates.io

The audience are Snakemake / Nextflow migrants who live in pip and conda. Once
the paper is public the name `oxymake` is squattable everywhere — reserve early:

| Registry | Why | Status |
|----------|-----|--------|
| **crates.io** | Rust-native; `cargo install oxymake` / `cargo-binstall` | placeholder crate `crates/oxymake` (manual `cargo publish -p oxymake`) |
| **PyPI** | the audience's home; a future `pip install oxymake` wrapper is the highest-leverage adoption move | scaffolding in `packaging/pypi/` (thin launcher; reserve the name, then publish) |
| npm | cheap insurance; low audience overlap | optional |

### Citability — Zenodo DOI

The paper cites `oxymake.noogram.dev`. Enable the GitHub↔Zenodo integration
before tagging so the `v*` tag auto-archives → a DOI the paper can cite as a
permanent, versioned artifact. The DOI can land in arXiv v2 if it misses the
first deposit.

### Pre-publish hygiene (biblion's healed scars)

- **Keywords:** ≤ 5 per crate, ≤ 20 chars each (biblion hit "keyword too long").
- **Categories:** exact slugs from the crates.io fixed list.
- **readme path** must be includable under the crate root (`cargo publish` only
  packages files under the crate dir).
- **No OpenSSL:** OxyMake uses `rustls-tls`, so cross-target builds stay clean.
- Existing CI gates (`secret-scan`, `deny`, `forbid-strings`, `topology-guard`)
  are the pre-publish review floor — green on the tag.

---

## Post-launch follow-ups (not blocking a release)

- **`pip install oxymake`** — thin launcher in `packaging/pypi/` that fetches the
  release binary (no Rust build). Highest adoption leverage; do first.
- **Homebrew tap** (`brew install noogram/tap/oxymake`) — formula template in
  `packaging/homebrew/oxymake.rb`; fill the per-release SHA256 from the
  `*.sha256` release assets, commit to the tap repo.
- **`cargo-binstall` metadata**, **`curl | sh` installer**.
- **Full crates.io workspace publish (Option C)** — only if/when the public API
  stabilises.
- **`aarch64-unknown-linux-gnu`** in the release matrix (cross linker required).

> **Launch message sequencing** (prime the Snakemake tribe privately → public
> repo + binary → problem-framed Show HN → decoupled arXiv) is its own runbook:
> [`docs/LAUNCH-SEQUENCE.md`](docs/LAUNCH-SEQUENCE.md).

---

## Difference from the biblion template

OxyMake started from biblion's `release.yml` + `just release` and diverged where
the binary-first, 24-crate shape demanded it:

| Aspect | biblion | OxyMake |
|--------|---------|---------|
| Product | published crates (`biblion`, `paper-resolver`) | single `ox` binary via Releases |
| crates.io | publish real crates from CI every tag | reserve the name once, manually; internals `publish = false` |
| Recipe name | `just release <v>` | `just tag-release <v>` |
| Preflight gates | clean tree, on main, test/clippy/fmt | + tag-not-exists, + CHANGELOG entry, + `just man` regen |
| CI gate before build | none (build runs first) | dedicated `gate` job (test + clippy + `cargo deny`) blocks build |
| Action pinning | floating tags (`@v4`, `@stable`) | every action SHA-pinned; toolchain frozen to an exact version |
| Checksums | none | per-tarball `.sha256` + combined `SHA256SUMS` on the Release |
| crates.io CI step | `cargo publish` jobs with `continue-on-error` + `sleep 60` index wait | none (manual one-shot avoids the recurring failure) |
| Targets | linux-x86_64 + darwin (arm64/x86_64); dropped arm64-linux (OpenSSL) | same 3; arm64-linux documented as a future addition (rustls, no OpenSSL blocker) |
