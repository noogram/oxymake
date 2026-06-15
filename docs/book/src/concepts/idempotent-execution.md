# Idempotent Execution

If you have used Terraform, you already understand OxyMake's execution
model. `ox run` does not mean "launch these jobs." It means **"ensure
these outputs exist."**

This is a fundamental design choice that affects everything from how you
think about running workflows to how multiple people can work on the same
pipeline simultaneously.

## The Convergent Model

When you run `ox run`, OxyMake looks at each job in the requested subgraph
and makes a decision:

| Current state | What OxyMake does |
|---------------|-------------------|
| Output exists and inputs haven't changed | **Skip** -- nothing to do |
| Job is already running (another session) | **Attach** -- wait for it, don't re-launch |
| Job is pending and unclaimed | **Claim and execute** |
| Job failed in a previous run | **Re-execute** |

The result: running the same command twice does nothing extra. Running it
while another instance is already working cooperates instead of conflicting.

```bash
ox run --rule '/human/'         # Launches the human-cohort jobs
ox run --rule '/human/'         # all skipped (cached), nothing re-runs
ox run --rule '/human/'         # (while first is running) attaches to running jobs
ox run                          # Launches yeast+mouse, attaches to human
```

## The Terraform Analogy

If you are familiar with infrastructure-as-code tools, the mapping is
direct:

| Terraform | OxyMake | Meaning |
|-----------|---------|---------|
| `terraform plan` | `ox plan` | Show what would happen |
| `terraform apply` | `ox run` | Make it so |
| `terraform destroy` | `ox invalidate` | Undo it |

Just as `terraform apply` creates only the resources that don't already
exist, `ox run` executes only the jobs whose outputs are missing or stale.

## Cooperative Sessions

The most powerful consequence of idempotent execution is that multiple
`ox run` processes can work on the same project simultaneously, without
conflicts.

### How It Works

OxyMake uses SQLite (WAL mode) as a coordination layer. When a session
wants to execute a job, it claims it atomically:

```sql
UPDATE jobs SET status = 'running', session_id = ?, locked_by = ?
WHERE id = ? AND status = 'pending';
```

If another session already claimed the job (0 rows affected), the current
session either waits for it (if it needs the output) or moves on to other
work.

### Example: Two Terminals

```bash
# Terminal 1: start the human pipeline
ox run --rule '/human/'
#  Session 1: 2,100 jobs to run

# Terminal 2 (while T1 is running): start the mouse pipeline
ox run --rule '/mouse/'
#  Session 2: 3,423 jobs to run. 0 conflicts with session 1.

# Terminal 3: run everything
ox run
#  Session 3: 10,247 total jobs
#    2,100 running (human, session 1) — attaching
#    3,423 running (mouse, session 2) — attaching
#    1,312 cached (completed by sessions 1+2) — skipping
#    3,412 to run (yeast + remaining) — executing
```

Session 3 does not duplicate work. It attaches to what sessions 1 and 2
are already doing, skips what they have finished, and picks up the rest.

### Stale Session Recovery

If a session crashes (power failure, OOM kill), its jobs are not stuck
forever. Each session sends a heartbeat every few seconds. If the heartbeat
is older than 2 minutes, the session is considered dead, and its running
jobs are reset to `pending` for other sessions to claim.

No manual cleanup required.

## The Lifecycle Commands

The convergent model needs symmetric operations. OxyMake provides five
commands that form a complete algebra of workflow control:

| Command | Meaning | Analogy |
|---------|---------|---------|
| `ox run` | Ensure outputs exist | `terraform apply` |
| `ox cancel` | Stop pursuing outputs | Ctrl+C with precision |
| `ox invalidate` | Forget outputs exist | `make clean` with precision |
| `ox plan` | Show what would happen | `terraform plan` |
| `ox status` | Show what is happening | `kubectl get pods` |

### Cancel

```bash
ox cancel --where cohort=human    # Stop human jobs
ox cancel --rule call             # Stop all variant calls
ox cancel --session 2             # Stop everything session 2 is doing
ox cancel                         # Stop everything
```

Canceled jobs have their partial outputs deleted and their status reset
to `pending`. The next `ox run` will re-execute them.

### Invalidate

```bash
ox invalidate --rule call                  # Delete variant-call outputs + cache entries
ox invalidate --rule call --cascade        # + all downstream outputs
ox invalidate --since "2026-03-22"         # Everything computed after this date
ox invalidate --run 3                      # Everything from run #3
```

The `--cascade` flag is important: invalidating a feature rule without
cascade leaves stale calls that depend on the old feature values.
With `--cascade`, OxyMake traverses the DAG forward and invalidates
everything downstream.

## Why This Matters

The idempotent execution model means:

1. **No accidental double-execution.** Two people running the same command
   cooperate instead of conflicting.
2. **Fearless re-running.** You can always run `ox run` again. If everything
   is up to date, it finishes instantly.
3. **Incremental by nature.** Add new rules, change parameters, re-run.
   Only the affected subgraph recomputes.
4. **Crash-resilient.** Completed work survives process death. Just re-run.
5. **Observable.** `ox status` shows exactly what is happening across all
   sessions.
