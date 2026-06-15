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
