#!/usr/bin/env bash
# Extract every [style=python] lstlisting from the paper source and verify
# each one compiles with `python -m py_compile`. Fails the build if any
# Python snippet shown in the paper is not valid Python.
set -euo pipefail

paper="$(dirname "$0")/../oxymake-paper.tex"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

awk -v dir="$tmpdir" '
  /\\begin\{lstlisting\}\[style=python\]/ { n++; f = dir "/snippet" n ".py"; capture = 1; next }
  /\\end\{lstlisting\}/ { capture = 0 }
  capture { print > f }
' "$paper"

count=0
for f in "$tmpdir"/snippet*.py; do
  [ -e "$f" ] || break
  python3 -m py_compile "$f"
  count=$((count + 1))
done

echo "check-python-listings: $count snippet(s) compiled OK"
