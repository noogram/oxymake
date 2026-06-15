# OxyMake Functional Test Report

**Date:** 2026-03-25
**Version:** ox 0.1.0
**Platform:** macOS Darwin 24.6.0
**Binary:** `~/.cargo/bin/ox` (symlink to `oxymake`)

---

## Test Results Summary

| # | Test | Result |
|---|------|--------|
| 1 | `ox init` | PASS |
| 2 | `ox lint` (valid) | PASS |
| 3 | `ox lint` (invalid TOML) | PASS |
| 4 | `ox plan` | PARTIAL (see notes) |
| 5 | `ox run --dry-run` | PASS |
| 6 | `ox run` (execution) | PASS |
| 7 | `ox run` (idempotent/cached) | PASS |
| 8 | `ox run -j N` (parallel) | **FAIL** |
| 9 | `ox run` (chained rules) | PASS |
| 10 | `ox run` (failure handling) | PASS |
| 11 | `ox run -k` (keep-going) | PASS |
| 12 | `ox run --json` | **FAIL** |
| 13 | Stub commands (status, cancel, invalidate) | PASS (expected stubs) |
| 14 | `--help` for all subcommands | PASS |
| E1 | `{input}`/`{output}` placeholders in shell | **FAIL** |
| E2 | `--rule` filter | **FAIL** |
| E3 | `--set` config override | **FAIL** |
| E4 | `-f` alternate file | PASS |
| E5 | `--where` tag filter | PASS |
| E6 | Static rules (no config vars) | PASS |
| E7 | Multiple outputs per rule | PASS |
| E8 | Specific target on CLI | PASS |
| E9 | `ox init --force` | PASS |
| E10 | `ox init` (existing project, no --force) | PASS (proper error) |

**Overall: 15 PASS, 1 PARTIAL, 5 FAIL**

---

## Detailed Test Results

### Test 1: `ox init` -- PASS

```bash
cd /tmp && mkdir -p oxymake-functional-tests/test-init && cd oxymake-functional-tests/test-init
ox init
```

**Expected:** Creates `Oxymakefile.toml` and `.oxymake/` directory.
**Actual:** Exactly as expected. Output:
```
Initialized OxyMake project in .
  Created: Oxymakefile.toml
  Created: .oxymake/
```
The generated `Oxymakefile.toml` contains a sample workflow with `{input}` and `{output}` placeholders (see Bug #1).

### Test 2: `ox lint` (valid) -- PASS

```bash
ox lint
```

**Expected:** "Oxymakefile is valid (2 rules)"
**Actual:** Exactly as expected. Exit code 0.

### Test 3: `ox lint` (invalid TOML) -- PASS

```bash
echo "invalid toml [[[" > Oxymakefile.toml
ox lint
```

**Expected:** Error message with line number.
**Actual:** Clear error with line and column:
```
error: parse error in Oxymakefile.toml: TOML parse error at line 1, column 9
```
Exit code 1.

### Test 4: `ox plan` -- PARTIAL

```bash
ox plan                # FAIL - "no rule produces output matching `data/A.csv`"
ox plan --level rules  # PASS - shows rule graph
ox plan --level jobs   # FAIL - same error as default
```

**Expected:** Shows the execution plan at various detail levels.
**Actual:** `--level rules` works and shows the rule graph. The default level (`optimized`) and `--level jobs` fail because they attempt to resolve leaf source files and error when no rule produces them. This is a fundamental issue: `ox plan` cannot show the job-level plan for any workflow that has external source files.

### Test 5: `ox run --dry-run` -- PASS

```bash
ox run --dry-run
```

**Expected:** Shows jobs that would execute.
**Actual:**
```
Dry run: 2 job(s) would execute for 2 target(s)
  [process-B] rule=process outputs=[results/B.txt]
  [process-A] rule=process outputs=[results/A.txt]
```

### Test 6: `ox run` (execution) -- PASS

```bash
ox run
```

**Expected:** 2 jobs succeed, output files created.
**Actual:**
```
Completed: 2 succeeded, 0 failed, 0 skipped, 0 cancelled (0.0s)
```
Output files contain correctly sorted data.

### Test 7: `ox run` (idempotent) -- PASS

```bash
ox run  # second run
```

**Expected:** 0 jobs to run (all cached).
**Actual:**
```
Completed: 0 succeeded, 0 failed, 0 skipped, 0 cancelled (0.0s)
```
Caching works correctly.

### Test 8: `ox run -j N` (parallel) -- FAIL

```bash
# 4 jobs, each sleeps 0.5s
time ox run -j 4   # Expected ~0.5s, Got 2.0s
time ox run -j 1   # Expected ~2.0s, Got 2.0s
```

**Expected:** With `-j 4`, 4 independent jobs sleeping 0.5s each should complete in ~0.5s.
**Actual:** Both `-j 4` and `-j 1` take exactly 2.0s. The `-j` flag is accepted but parallelism is not implemented. Jobs always execute sequentially.

### Test 9: `ox run` (chained rules) -- PASS

```bash
# step1: uppercase, step2: count characters
ox run
```

**Expected:** 4 jobs (2 per step), proper dependency ordering.
**Actual:**
```
Completed: 4 succeeded, 0 failed, 0 skipped, 0 cancelled (0.0s)
```
Mid-stage files contain uppercased text. Final files contain character counts. Dependency chaining works correctly.

### Test 10: `ox run` (failure) -- PASS

```bash
# rule with `exit 1`
ox run
```

**Expected:** Error, exit code 1.
**Actual:**
```
error: executor error: 1 job(s) failed out of 1 (0s elapsed)
```
Exit code 1.

### Test 11: `ox run -k` (keep-going) -- PASS

```bash
# 2 jobs: "ok" succeeds, "fail" exits 1
ox run -k
```

**Expected:** "ok" succeeds, "fail" fails, run continues.
**Actual:**
```
Completed: 1 succeeded, 1 failed, 0 skipped, 0 cancelled (0.0s)
```
Exit code 1. The `out/ok.txt` was created with correct content. Keep-going works.

### Test 12: `ox run --json` -- FAIL

```bash
ox run --json
```

**Expected:** NDJSON event stream.
**Actual:** Normal human-readable output:
```
Completed: 2 succeeded, 0 failed, 0 skipped, 0 cancelled (0.0s)
```
No JSON output on either stdout or stderr. The `--json` flag is accepted but has no effect.

### Test 13: Stub Commands -- PASS (expected)

```bash
ox status      # "ox status: not yet implemented"
ox cancel      # "ox cancel: not yet implemented"
ox invalidate  # "ox invalidate: not yet implemented"
ox dag         # "ox dag: not yet implemented"
ox logs        # "ox logs: not yet implemented"
ox history     # "ox history: not yet implemented"
ox snapshot    # "ox snapshot: not yet implemented"
ox gate        # "ox gate: not yet implemented"
ox clean       # "ox clean: not yet implemented"
```

All return "not yet implemented" with exit code 0. These are documented stubs.

### Test 14: `--help` for all subcommands -- PASS

All 12 subcommands (run, plan, status, cancel, invalidate, dag, logs, history, snapshot, gate, lint, init, clean) accept `--help` and display usage information.

### Test E1: `{input}`/`{output}` Placeholders -- FAIL

```bash
shell = "cat {input} > {output}"
```

**Expected:** `{input}` expands to the rule's input file path, `{output}` to the output path.
**Actual:** `{input}` and `{output}` are treated as literal strings. The shell receives `cat {input}` which fails with `cat: {input}: No such file or directory`.

Only config-variable placeholders (like `{sample}`) are substituted. This is critical because the `ox init` template uses `{input}` and `{output}` in its generated Oxymakefile, meaning the default project is broken out of the box.

### Test E2: `--rule` Filter -- FAIL

```bash
ox run --rule step1  # Expected: only step1 jobs run
```

**Expected:** Only jobs from the `step1` rule execute.
**Actual:** All 4 jobs (step1 and step2) executed. The `--rule` flag is accepted but has no filtering effect.

### Test E3: `--set` Config Override -- FAIL

```bash
ox run --set samples='["A"]' --dry-run
```

**Expected:** Only 1 job (sample A).
**Actual:** 2 jobs shown (both A and B). The `--set` flag is accepted but has no effect.

### Test E4: `-f` Alternate File -- PASS

```bash
ox run -f custom.toml
```

Works correctly. Custom Oxymakefile paths are supported.

### Test E5: `--where` Tag Filter -- PASS

```bash
ox run --where group=alpha
```

Jobs with matching tags executed successfully. (Note: since all jobs had the tag, I could not verify that non-matching jobs are excluded.)

### Test E6: Static Rules -- PASS

Rules with no config variables work correctly.

### Test E7: Multiple Outputs -- PASS

Rules producing multiple output files work correctly.

### Test E8: Specific Target -- PASS

```bash
ox run results/A.txt
```

Only the targeted file was built. `results/B.txt` was not created.

---

## Bugs Found

### Bug #1 (Critical): `{input}` and `{output}` placeholders not substituted in shell commands

**Severity:** Critical
**Location:** Shell command expansion
**Description:** The `{input}` and `{output}` placeholders in `shell` commands are not expanded to actual file paths. Only config-variable placeholders (e.g., `{sample}`) are substituted.
**Impact:** The `ox init` template generates a broken Oxymakefile that fails on first run. Users following the generated template will get `cat: {input}: No such file or directory`.
**Workaround:** Use explicit paths with config variables instead:
```toml
shell = "sort data/{sample}.csv > results/{sample}.txt"
```

### Bug #2 (Major): Parallel execution not working (`-j N`)

**Severity:** Major
**Location:** Job executor / scheduler
**Description:** The `-j N` flag is accepted but all jobs execute sequentially regardless of the value. 4 independent jobs sleeping 0.5s each take 2.0s with both `-j 1` and `-j 4`.
**Impact:** No performance benefit from parallel execution. Large workflows cannot utilize multiple cores.

### Bug #3 (Major): `ox plan` fails at default level for workflows with source files

**Severity:** Major
**Location:** Plan resolver
**Description:** `ox plan` (default `--level optimized`) and `ox plan --level jobs` fail with "no rule produces output matching ..." when the workflow has leaf source files (files not produced by any rule). Only `--level rules` works.
**Impact:** Users cannot preview the job-level execution plan, which defeats the purpose of `ox plan`.

### Bug #4 (Moderate): `--json` flag produces no JSON output

**Severity:** Moderate
**Location:** Output formatter
**Description:** `ox run --json` produces the same human-readable text as `ox run`. No NDJSON events are emitted to either stdout or stderr.
**Impact:** CI/CD integrations and programmatic consumers cannot parse run results.

### Bug #5 (Moderate): `--rule` filter has no effect

**Severity:** Moderate
**Location:** Job filter / scheduler
**Description:** `ox run --rule step1` executes all jobs, not just those from the named rule.
**Impact:** Users cannot selectively run specific rules.

### Bug #6 (Moderate): `--set` config override has no effect

**Severity:** Moderate
**Location:** Config parser
**Description:** `ox run --set samples='["A"]' --dry-run` shows all samples, not just "A". The flag is accepted but the value is not applied.
**Impact:** Users cannot override config values from the command line for quick testing or CI parameterization.

---

## Previously Stub Commands (Now Implemented)

The following subcommands were originally stubs returning "not yet implemented"
but have since been fully implemented during the Mar 29 -- Apr 1 development sprint:

| Command | Status |
|---------|--------|
| `ox status` | Implemented (execution status, Ray polling) |
| `ox cancel` | Implemented |
| `ox invalidate` | Implemented |
| `ox dag` | Implemented (dot, mermaid formats) |
| `ox logs` | Implemented |
| `ox history` | Implemented (per-rule duration tracking) |
| `ox snapshot` | Implemented (create/list/diff/delete) |
| `ox gate` | Implemented (approve/reject) |
| `ox clean` | Implemented |

---

## Features That Work Well

1. **`ox init`** -- Clean project scaffolding (template content has a bug, but the mechanism works)
2. **`ox lint`** -- Validates TOML syntax and rule structure with clear error messages
3. **`ox run`** -- Core execution works reliably for shell-based rules
4. **`ox run --dry-run`** -- Accurate preview of what would execute
5. **`ox run -k`** -- Keep-going mode correctly continues independent branches
6. **Config variable expansion** -- `{sample}`, `{name}`, etc. expand correctly in both paths and shell commands
7. **Dependency chaining** -- Multi-step pipelines resolve and execute in correct order
8. **Content-based caching** -- Unchanged inputs are correctly skipped on re-run
9. **Specific targets** -- `ox run results/A.txt` builds only what is needed
10. **`-f` flag** -- Alternate Oxymakefile paths work
11. **`--where` tag filter** -- Tag-based filtering appears to work
12. **Multiple outputs** -- Rules producing multiple files work correctly
13. **`ox init --force`** -- Overwrites existing project correctly
14. **Error reporting** -- Failed jobs report clearly with exit code 1

---

## Recommendations

### Priority 1 (Critical)
1. **Fix `{input}`/`{output}` substitution** -- This breaks the out-of-box experience. Either implement these placeholders or update `ox init` to generate a working template that uses explicit paths.

### Priority 2 (High)
2. **Implement parallel execution** -- The `-j` flag exists but does nothing. This is a core feature for any build system.
3. **Fix `ox plan` resolution** -- `plan` should not require leaf source files to have producing rules. It should treat them as given inputs, just like `ox run --dry-run` does.

### Priority 3 (Medium)
4. **Implement `--json` NDJSON output** -- Important for CI/CD integration.
5. **Implement `--rule` filter** -- Users need to selectively run parts of a workflow.
6. **Implement `--set` override** -- Config overrides are essential for parameterized workflows.
7. **Implement `ox clean`** -- Users need a way to reset state without manually deleting files.

### Priority 4 (Low)
8. Implement remaining stubs (dag, logs, history) as time permits.
