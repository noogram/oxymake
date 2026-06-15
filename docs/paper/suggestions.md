# Paper Improvement Suggestions

Generated during continuous integration review (2026-03-27).

## Content Gaps Identified

1. **Evaluation section remains placeholder**: The benchmarks described in Section 6
   have not yet been run. Consider prioritizing at least the DAG resolution benchmark
   (synthetic workflows at 1K/10K/100K scale) since this is the most impactful claim.

2. **Dashboard not described in architecture**: The web dashboard (`ox dashboard`) with
   its REST API, DAG visualization, and SSE event stream is a significant feature that
   deserves its own subsection under Architecture or Implementation. The API surface
   (`/api/dag`, `/api/jobs`, `/api/runs`, `/api/stats/rules`, `/api/gates`, `/api/sse/events`)
   demonstrates the agent-first API principle in practice.

3. **Lockfile deserves more treatment**: The `ox.lock` reproducibility lockfile
   (implemented in `ox-lock`) directly supports the FAIR workflow claims in Section 2.3.
   Consider adding a paragraph in the Design Principles section showing the lockfile
   structure (BLAKE3 hashes of rules, inputs, environments, platform info).

4. **Config expansion is novel**: File-based config expansion (`source = "samples.csv"`)
   allows dynamic workflow configuration from external data sources without breaking
   TOML's static parseability. This addresses the "TOML expressiveness" limitation
   discussed in Section 7.2 and deserves mention.

5. **Translate command bridges adoption**: The `ox translate` command (Snakemake to
   OxyMake) is a practical adoption tool. Worth mentioning in the introduction or
   conclusion as lowering the migration barrier.

## Figure Improvements

6. **Execution timeline figure**: The commented-out `timeline-gantt.png` placeholder
   could be realized once `ox history` provides per-job timing data. This would
   concretely demonstrate the parallelism benefits.

7. **Dashboard screenshot**: A screenshot of the web dashboard showing live DAG
   visualization and status cards would strengthen the agent-first API claims.

## Statistical Updates Needed

8. **Crate count in text**: Various places still reference "18 crates" — now 19 with
   `ox-lock`. (Fixed in this update.)

9. **Command count**: The paper mentions "four primary CLI commands" but there are now
   17 commands. (Fixed in this update.)

10. **Development timeline table**: The timeline table (Table 3) only covers the first
    2 hours of development. Consider adding a Phase 2 section showing the continuous
    integration commits (dashboard, lockfile, wildcard constraints, config expansion,
    translate enhancements) to demonstrate the sustained development methodology.
