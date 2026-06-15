# Environment variables

This page is the **canonical, versioned reference** for environment
variables OxyMake reads and sets. Per `STATUS.md` §6, only the
variables listed under **Stable** are subject to SemVer discipline.

**Last reviewed:** 2026-05-27.

---

## Variables OxyMake reads

### Stable

| Name | Where | Effect | Default |
|------|-------|--------|---------|
| `OX_CACHE_VALIDATION` | `ox run` | Override `--cache-validation`. Values: `mtime`, `mtime+hash`, `hash`. CLI flag wins when both are set. | (uses CLI flag, else `mtime+hash`) |

### Honoured external conventions

These follow widely-used conventions; OxyMake observes them but does
not own them.

| Name | Effect |
|------|--------|
| `NO_COLOR` | When set and non-empty, disables ANSI colour output. See [no-color.org](https://no-color.org). |
| `TERM=dumb` | Disables ANSI colour output. |
| `CI` | When set, treated as "non-interactive terminal" for colour and verbosity heuristics. |
| `HOME` | Used for `~` expansion in Oxymakefile paths. |

### Honoured by executor backends (subject to backend stability)

| Name | Where | Effect |
|------|-------|--------|
| `SLURM_JWT` | SLURM REST executor | JWT token for `slurmrestd`. If set, overrides `token_cmd`. |
| `USER` / `SLURM_USER` | SLURM CLI executor | User identity for job submission. |
| `XDG_CONFIG_HOME` | `ox run` | Config file lookup root. Defaults to `$HOME/.config`. |

These are honoured because the backend is stable for their use, not
because the variable names themselves are an OxyMake-owned surface. If
the SLURM backend's contract changes, expect the variables it reads to
change with it.

---

## Variables OxyMake sets (visible to scripts)

These are exported into the environment of every job step. Scripts may
rely on them. See `STATUS.md` §6 for the stability tier.

### Stable

| Name | Value |
|------|-------|
| `OX_JOB_ID` | Stable string identifier for the running job. Unique within a run. |
| `OX_WC_<wildcard>` | One variable per wildcard in the rule's pattern. Example: a rule with output `results/{sample}.txt` produces `OX_WC_sample=S001` in the job's environment. |

### Unstable

Any other `OX_*` variable observed in a job's environment is
**implementation detail** and may change without a deprecation
window. If you find yourself relying on one, file an issue — that
is a signal we should promote it.

---

## Adding a new variable

When introducing a new environment variable that OxyMake reads or sets:

1. **Pick a tier** — default to **unstable**.
2. **Document it in this file** under the right section.
3. **Update `STATUS.md` §6**.
4. **Add a `CHANGELOG.md` entry** under `[Unreleased] / Added`.

Renaming or removing a *stable* variable requires a minor-version bump
and an entry under **Breaking changes** in `CHANGELOG.md`.

---

## Forward compatibility

Consumers should:

- Read a missing variable as "feature not requested" rather than as an
  error.
- Never depend on the *absence* of a variable for correctness — new
  variables may appear in any release.
- Treat the value of an unstable variable as opaque, even if it looks
  meaningful.
