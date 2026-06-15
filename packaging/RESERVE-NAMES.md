# Reserving the `oxymake` name across registries

> **Goal — namelock.** Stop anyone else from taking the name `oxymake` on the
> three package registries our audience uses. The real product is the **`ox`**
> binary, shipped via [GitHub Releases](https://github.com/noogram/oxymake/releases);
> these registry entries only **hold the name** and point users to that binary.
>
> **This file documents the reservation. It does not publish anything.**
> Publishing claims a public namespace ~permanently and needs registry tokens —
> it is a deliberate **operator gesture** (release phase 2), not a worker action.

Everything here is *prepared and verified* (dry-run / package / build), never
published. Each section gives: the files, the **verify** command (safe, offline-ish,
re-runnable), the **publish** command (operator-only), and the **prerequisites**
(token / login).

The same `oxymake` identity is used everywhere:

| Field | Value |
|-------|-------|
| name | `oxymake` |
| version | `0.0.0` (cargo, npm placeholders) · `0.1.0` (pypi launcher) |
| description | *Next-generation workflow orchestration in Rust* |
| license | `MIT OR Apache-2.0` |
| repository | `https://github.com/noogram/oxymake` |
| homepage | `https://oxymake.dev` |
| author | Emmanuel Sérié `<emmanuel@serie.dev>` |

---

## 1. crates.io (Rust) — `cargo`

The name is held by a thin placeholder crate that ships no code (release Option A,
see `RELEASING.md`). It is the one crate in the workspace with `publish = true`;
all 23 library crates stay `publish = false`.

| | |
|---|---|
| **Files** | `crates/oxymake/Cargo.toml`, `crates/oxymake/src/lib.rs`, `crates/oxymake/README.md` |
| **Version** | `0.0.0` |
| **Verify** | `cargo package -p oxymake` — packages + verify-compiles the crate without publishing |
| **Publish** | `cargo publish -p oxymake` |
| **Prerequisite** | `cargo login <crates.io API token>` (token from <https://crates.io/settings/tokens>); the crates.io account must own / be able to claim the name |

Verified: `cargo package -p oxymake` → *Packaged 6 files … Finished* (clean).

Notes (biblion's healed scars, applied here):
- keywords ≤ 5, each ≤ 20 chars;
- categories are exact crates.io slugs (`development-tools::build-utils`);
- `readme` path resolves under the crate root (cargo only packages files under
  the crate dir).

---

## 2. npm (JavaScript) — `npm`

Cheap insurance against squatting; low audience overlap. A minimal placeholder
package that ships only a README pointing at the `ox` binary.

| | |
|---|---|
| **Files** | `packaging/npm/package.json`, `packaging/npm/README.md` |
| **Version** | `0.0.0` |
| **Verify** | `cd packaging/npm && npm pack --dry-run` (and `npm publish --dry-run` for the full publish simulation) |
| **Publish** | `cd packaging/npm && npm publish` |
| **Prerequisite** | `npm login` (an npmjs.com account); the name `oxymake` must be free on <https://www.npmjs.com/package/oxymake> |

Verified: `npm pack --dry-run` and `npm publish --dry-run` → `+ oxymake@0.0.0`,
2 files (README.md + package.json), no warnings.

The package is intentionally code-free (`files: ["README.md"]`). If a real npm
entry point is ever wanted (a launcher that downloads `ox`, mirroring the pypi
one), it replaces the README in a future minor.

---

## 3. PyPI (Python) — `twine`

The audience's home (Snakemake / Nextflow migrants live in pip + conda), so the
PyPI entry is **more than a placeholder**: it is a working thin launcher that, on
first run, downloads the prebuilt `ox` binary for the host platform, verifies its
SHA-256, caches it, and execs it. No Rust toolchain required.

| | |
|---|---|
| **Files** | `packaging/pypi/pyproject.toml`, `packaging/pypi/oxymake_launcher.py`, `packaging/pypi/README.md` |
| **Version** | `0.1.0` |
| **Backend** | `hatchling` |
| **Verify** | `cd packaging/pypi && python -m build` then `python -m twine check dist/*` |
| **Publish** | `cd packaging/pypi && python -m build && twine upload dist/*` |
| **Prerequisite** | a PyPI API token in `~/.pypirc` or `TWINE_USERNAME=__token__` + `TWINE_PASSWORD=<pypi-token>`; build tooling: `pip install build hatchling twine`; the name must be free on <https://pypi.org/project/oxymake/> |

Verified: `python -m build` → *Successfully built oxymake-0.1.0.tar.gz and
oxymake-0.1.0-py3-none-any.whl*; `twine check dist/*` → both **PASSED**.

> **Fix applied during this preparation:** the distribution name (`oxymake`) does
> not match the launcher module (`oxymake_launcher.py`), so hatchling could not
> auto-detect the wheel contents and `python -m build` **failed the wheel step**.
> Added explicit `[tool.hatch.build.targets.wheel] only-include` /
> `[tool.hatch.build.targets.sdist] include` tables. The build is now green.

> **Phase-2 prerequisite, not a blocker now:** the launcher fetches release
> assets for `v0.1.0`. Publishing to PyPI before a `v0.1.0` GitHub Release exists
> means `pip install oxymake` succeeds but the first `ox` invocation fails to
> download its binary. Either cut the `v0.1.0` release first, or publish a
> code-free `0.0.x` placeholder to PyPI for pure name-hold and let the launcher
> land with the real release. This is an operator decision at publish time.

---

## Order of operations (operator, phase 2)

These are **not** run by workers. When the operator decides to claim the names:

```sh
# 1. crates.io
cargo login <crates-token>
cargo publish -p oxymake

# 2. npm
npm login
( cd packaging/npm && npm publish )

# 3. PyPI  (see the phase-2 note above re: release ordering)
( cd packaging/pypi && python -m build && twine upload dist/* )
```

Each is a one-shot name-hold, not a recurring CI job — see `RELEASING.md`
(§ *Why the reservation is a manual one-shot, not a CI job*).

## Related distribution scaffolding (not a name reservation)

- `packaging/homebrew/oxymake.rb` — Homebrew formula template for the
  `noogram/homebrew-tap`; per-release SHA256 values are filled from the release
  `*.sha256` assets, then committed to the tap repo. Not a registry name-hold.
