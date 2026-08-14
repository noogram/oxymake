# Cross-engine cache-behaviour harness

This is a deliberately small, rerunnable falsification experiment for the
cache semantics of `ox`, `cwltool`, and an Arvados deployment. It does not
claim that CWL-the-spec has a cache policy: the compared objects are engines.

The harness is evidence for, not a replacement for, a deployment-specific
Arvados/Keep measurement. It records the engine version, commands, output
hashes, sizes, raw mtimes, and a plain-language verdict in a fresh
`results/<UTC timestamp>/` directory. Generated results are ignored by Git.

## Experiment A: same-size, same-mtime corruption

The workflow always creates the 23-byte payload `immutable-cache-output`.
Each engine is warmed once. The harness replaces the first byte of the cached
or cache-validated output with `X`, then restores its original mtime. It reruns
the identical workflow and classifies the resulting output:

- `clean_output_observed`: the engine re-executed or restored a verified copy;
- `poisoned_output_served`: the changed byte survived cache reuse.

That wording is intentional: the trace alone does not distinguish a rerun from
a verified restore. Inspect `first.log` and `second.log` before asserting the
mechanism.

Run a local comparison from this directory:

```bash
./run.sh --engine all
```

Requirements are Bash 4+, standard POSIX file tools, and `ox` and/or
`cwltool` on `PATH`. Missing engines are recorded as `SKIPPED`, which makes a
partial local run useful without silently turning it into a cross-engine claim.
Run individual cases with `--engine ox-default`, `ox-hash`, or `cwltool`.

The OxyMake cases measure both documented policies: the default `mtime+hash`
and `hash`, which verifies output content on every cache hit. OxyMake's local
cache validates the workspace output rather than a hidden output copy; that is
the file corrupted in these two cases. The cwltool case uses `--cachedir` and
corrupts the unique cached `result.txt`; it stops if that layout is not unique,
so a cwltool layout change cannot yield a misleading observation.

## Experiment B: delivery envelope

Capture actual local facts, rather than assigning an unmeasured disk or setup
cost to a different machine:

```bash
./measure-envelope.sh
```

The report identifies the locally installed `ox` executable and size, installed
tool versions, and the minimum local filesystem requirement. It also makes the
non-equivalence explicit: an Arvados/Keep result must name the deployment and
its running API, Keep, database, dispatch, and worker services. A client wheel
or `arvados-cwl-runner` alone is not an Arvados/Keep deployment.

For a deployment you administer, set `ARVADOS_API_HOST` and
`ARVADOS_API_TOKEN`, then submit the same CWL tool:

```bash
./arvados-run.sh
```

It records the submission output only. Collect container UUIDs, service
inventory, persistent-volume usage, and Keep collection identifiers using that
deployment's administrator tooling, then cite those immutable records beside
the harness result. Do not place credentials or deployment logs in this public
example.

## Interpreting and publishing results

Never publish a row for an engine that says `SKIPPED` or `BLOCKED`. Pin the
versions from `versions.txt` and retain the entire timestamped result directory.
If `cwltool` reports a clean output, the experiment refutes any claim that its
cache blindly serves this same-size/same-mtime corruption for that tested
version and configuration. If it serves poisoned output, report the exact
version and cache-directory evidence, not a general claim about CWL.

All files in this example are public rerunnable inputs; no result has been
pre-filled.
