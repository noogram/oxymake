#!/usr/bin/env bash
# DAG visualization: ox dag outputs text, dot, and mermaid formats
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

# Text format (default)
output_text=$($OX dag 2>&1)
[ -n "$output_text" ] || { echo "FAIL: ox dag text produced no output"; exit 1; }

# Dot format
output_dot=$($OX dag --format dot 2>&1)
echo "$output_dot" | grep -q "digraph" || { echo "FAIL: dot output missing digraph"; exit 1; }

# Mermaid format
output_mmd=$($OX dag --format mermaid 2>&1)
echo "$output_mmd" | grep -qi "graph\|flowchart" || { echo "FAIL: mermaid output missing graph header"; exit 1; }
