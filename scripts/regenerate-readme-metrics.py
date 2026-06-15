#!/usr/bin/env python3
"""Rewrite metric-driven regions of README.md from metrics/metrics.json.

Per ADR-016, README is a consumer of the canonical metrics file. This
script rewrites content between tagged anchor blocks ; prose around the
anchors is operator-owned and never touched.

Anchor syntax (HTML comments — invisible in rendered Markdown) :

    <!-- METRICS:BEGIN <metric-key> -->
    <managed line, rewritten by this script>
    <!-- METRICS:END -->

The renderer is intentionally per-anchor (not a generic template
substitution) because each managed line has its own surface phrasing
that the operator chose deliberately ; only the *value* should move.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ANCHOR_RE = re.compile(
    r"(<!-- METRICS:BEGIN (\S+) -->)(.*?)(<!-- METRICS:END -->)",
    re.DOTALL,
)


def line_for(key: str, metrics: dict) -> str:
    """Return the managed line that belongs between BEGIN/END markers.

    Falls back to a visible placeholder when the metric is not yet
    populated, so the renderer never silently emits stale prose.
    """
    entry = metrics.get(key)
    if not entry or "value" not in entry:
        return (
            f"*(metric `{key}` not yet populated in metrics/metrics.json — "
            f"see ADR-016)*"
        )
    value = entry["value"]
    if key == "dag_resolution_10k_ms":
        # Pair with dag_resolution_100k_ms when present ; otherwise just
        # quote the 10K number.
        big = metrics.get("dag_resolution_100k_ms", {}).get("value")
        if big is not None:
            # Render 1024 ms -> "1s" (approximate, headline form).
            seconds = round(float(big) / 1000, 1)
            return f"- **Fast** — 10K-job DAG resolved in {value}ms, 100K in ~{seconds}s"
        return f"- **Fast** — 10K-job DAG resolved in {value}ms"
    return str(value)


def main(metrics_path: str, readme_path: str) -> int:
    metrics = json.loads(Path(metrics_path).read_text())
    text = Path(readme_path).read_text()

    def sub(m: re.Match) -> str:
        begin, key, _body, end = m.group(1), m.group(2), m.group(3), m.group(4)
        return f"{begin}\n{line_for(key, metrics)}\n{end}"

    new = ANCHOR_RE.sub(sub, text)
    if new == text:
        print(f"regenerate-readme-metrics: {readme_path} unchanged", file=sys.stderr)
        return 0
    Path(readme_path).write_text(new)
    print(f"regenerate-readme-metrics: rewrote {readme_path}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))
