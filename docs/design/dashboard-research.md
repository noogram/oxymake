# Dashboard UX Research: Workflow Visualization Patterns

> **Full research archived to vault:** `vault/research/oxymake/dashboard-prior-art.md`

## Summary

Comparative analysis of workflow visualization across six systems (Dask, Prefect,
Airflow, Dagster, Snakemake/Panoptes, Nextflow Tower) to inform OxyMake's
dashboard design.

## Top Recommendations for OxyMake

1. **Execution Timeline / Gantt Chart** (HIGH IMPACT, MEDIUM COST) — horizontal
   bars per job on a wall-clock axis. Data already available via `/api/runs/:id/jobs`.
2. **Enhanced DAG with Dagre Layout** (HIGH IMPACT, LOW COST) — replace
   breadthfirst layout with layered (dagre), add tooltips and click-to-highlight.
3. **Live Progress with ETA** (MEDIUM IMPACT, LOW COST) — per-rule elapsed time,
   average-based ETA estimation, overall progress bar.

Future: Grid View (historical), Task Stream (distributed), Resource Usage (instrumented).
