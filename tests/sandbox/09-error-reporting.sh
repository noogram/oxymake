#!/usr/bin/env bash
# Error reporting: broken command exits non-zero with clear error
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true

# Overwrite with a broken Oxymakefile
cat > Oxymakefile.toml <<'EOF'
ox_version = "0.1"

[config]
samples = ["a"]

[rule.all]
input = ["out/{sample}.txt"]

[rule.broken]
output = ["out/{sample}.txt"]
wildcard_constraints = { sample = "a" }
shell = """
nonexistent_cmd_xyz_99999
"""
EOF

if $OX run 2>&1; then
    echo "FAIL: broken command should have caused non-zero exit"
    exit 1
fi
