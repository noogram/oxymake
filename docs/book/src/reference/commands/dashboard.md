# ox dashboard

Web dashboard for monitoring and DAG visualization.

The `ox dashboard` command starts a local HTTP server that serves an interactive
web UI. The dashboard reads from the OxyMake state database and provides
real-time job status, DAG visualization, and run history.

## Usage

```bash
ox dashboard                        # Start on http://127.0.0.1:9876
ox dashboard --port 8080            # Custom port
ox dashboard --bind 0.0.0.0         # Listen on all interfaces
ox dashboard --db path/to/state.db  # Custom state database
```

## Options

| Flag | Description |
|------|-------------|
| `--db <DB>` | Path to state.db (default: `.oxymake/state.db`) |
| `--port <PORT>` | Port to listen on (default: `9876`) |
| `--bind <BIND>` | Bind address (default: `127.0.0.1`) |

## Features

- **Status cards** — at-a-glance counts of running, succeeded, and failed jobs
- **DAG visualization** — interactive dependency graph
- **Job table** — sortable list of all jobs with status and timing
- **Run history** — browse past runs and their outcomes

## Running vs. orphaned

Like `ox status`, the dashboard reports an **effective** status rather than
`jobs.status` verbatim: a job is `running` only while the session that
claimed it is `active` with a heartbeat younger than the claim lease (90 s,
`OX_SESSION_LEASE_SECS` overrides). A row left behind by a session that was
interrupted, completed, or that stopped heartbeating shows as **orphaned**,
in its own card and with its own badge — never the `RUNNING` badge, and
never counted in the ETA or the jobs/sec strip.

This shows up in the API as:

| Endpoint | Field |
|----------|-------|
| `GET /api/status`, SSE `GET /api/events` | `orphaned` count; `running` excludes it |
| `GET /api/jobs` | `status` is the effective status (`?status=running` never returns an orphan; `?status=orphaned` returns them), plus `declared_status` and `orphan_reason` |
| `GET /api/dag` | node `status` is effective, with `orphan_reason` |
| `GET /api/job/:id` | `status` is effective, with `orphan_reason` |
| `GET /api/stats/rules` | per-rule `orphaned`, split out of `running` |

`orphan_reason` is one of `session_interrupted`, `heartbeat_stale`,
`session_completed`, `session_missing`.

The dashboard is **read-only**: it reports orphaned rows, it never reclaims
them. Reclaiming happens at the start of the next `ox run`. See
[ADR-012](../../../adr/012-cooperative-multi-session.md).

## Examples

```bash
# Start dashboard alongside a long-running workflow
ox run -j 8 &
ox dashboard
# Open http://127.0.0.1:9876 in a browser

# Expose to the local network (e.g. for a shared workstation)
ox dashboard --bind 0.0.0.0 --port 8080
```

## See Also

- [ox top](../commands.md#ox-top) — terminal TUI dashboard
- [ox status](../commands.md#ox-status) — CLI status summary
