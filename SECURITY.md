# Security Policy

## Supported versions

OxyMake is at an early stage (v0.1.x). Security fixes are applied to the
latest released version only.

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✅ best-effort |
| < 0.1   | ❌ |

## Reporting a vulnerability

Please report security issues **privately**, not via public issues.

- Preferred: GitHub's [private vulnerability reporting](https://github.com/noogram/oxymake/security/advisories/new).
- Or email: `emmanuel@serie-research.dev`.

Include a description, reproduction steps, and the affected version
(`ox --version`). You can expect an initial acknowledgement within a few days,
best-effort — OxyMake is maintained by a single independent researcher.

## Scope

OxyMake executes workflow rules as local shell commands and reads
`Oxymakefile.toml` from the working directory. Treat an `Oxymakefile.toml`
from an untrusted source as you would any executable script: it can run
arbitrary commands by design. The content-addressable cache key includes the
rule source, so tampering with a rule invalidates its cache entry rather than
silently reusing a stale output.

### Shell interpolation is a data/code boundary

`shell` rules are expanded into one command string and run through the
configured shell with `-c`. Placeholder values are inserted without shell
escaping; `{input}` and `{output}` lists are joined with spaces. Consequently,
an interpolated value is shell syntax, not a typed command-line argument.

This matters even when the Oxymakefile is trusted. A file name from a checkout,
archive, upload, shared directory, or another producer can become a concrete
input path when a target or wildcard selects it. Wildcards can be supplied by
the requested target or expanded from configuration. A config value can be
third-party data when the operator imports or generates configuration from an
external source, or passes an externally supplied `--set` value. Outputs and
log paths derived from those values have the same property. By contrast,
literal `shell` text, literal paths, rule `params`, and resource values in a
trusted Oxymakefile are authored by the workflow author unless that author
deliberately routes outside data into them.

Do not interpolate untrusted values into `shell`. Validate or constrain them
to a safe filename/value alphabet before invoking OxyMake, or use an execution
mode and tool interface that accepts structured data rather than a shell
command. Quoting placeholders in an existing shell template is not a general
fix: it must be designed for the selected shell and the value's intended
syntactic context.

## Cache integrity

### Validation strategies

The cache key is content-addressable, but whether *output content* is
verified at lookup time depends on the validation strategy
(`--cache-validation`, `OX_CACHE_VALIDATION`):

- **`mtime+hash` (default)** — trusts metadata (mtime + size) when unchanged;
  re-hashes (BLAKE3) whenever metadata differs. Detects same-size corruption
  or tampering that touches the timestamp. Near-zero overhead on genuinely
  unchanged files.
- **`hash`** — always re-hashes every output. Strongest guarantee; use this
  on a shared or multi-user cache, or in CI.
- **`mtime` (opt-in only)** — Make/Snakemake parity: existence + newer-mtime,
  **content is never verified**. A corrupted or replaced output with a newer
  mtime is served as a hit. Never use `mtime` on a cache directory that other
  users or processes can write to.

### Remote cache fetches

Remote cache backends (`RemoteCache` implementations) must re-verify every
fetched artifact against its content hash after transfer and reject the
entry (delete the local copy, report a miss) on mismatch. The bundled
directory backend does this; third-party backends must honor the same
contract (see `ox-core`'s `RemoteCache::fetch` documentation).

### External-reference verification runs a shell

The external-reference materialization strategy (virtual outputs with a
`check` command) executes its check command via `sh -c` during cache
*verification* — verification is not read-only for those outputs. The check
command comes from the Oxymakefile, so it carries the same trust level as
rule commands, but be aware of it when inspecting caches produced from
untrusted workflow files. This strategy is not currently wired into the run
loop.

### Cache keys are platform-scoped

The cache key includes the host OS and CPU architecture: caches are portable
across machines of the *same* platform (e.g. Linux x86_64 ↔ Linux x86_64).
A cache shared between heterogeneous platforms (e.g. macOS arm64 ↔ Linux
x86_64) never produces false hits — and never hits at all; heterogeneous
cache reuse is future work.
