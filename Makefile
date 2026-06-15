# Top-level Makefile — orchestrates the metrics-as-single-source pipeline
# (ADR-016).
#
# Targets that build user-visible artefacts (paper PDF, README, benchmark
# RESULTS.md) depend on metrics/metrics.json so the canonical file is
# regenerated before any artefact that quotes it is rebuilt.
#
# Most workspace tasks are still driven by `cargo`. This Makefile owns
# only the cross-artefact metric coherence concern.

.PHONY: metrics metrics-check metrics-readme metrics-paper clean-metrics

METRICS_JSON := metrics/metrics.json

# Regenerate the canonical metrics file from live workspace measurements.
# Perf metrics (dag_resolution_*, throughput_*, ...) are preserved across
# runs ; the bench harness owns those.
metrics: $(METRICS_JSON)

$(METRICS_JSON): scripts/regenerate-metrics.sh
	bash scripts/regenerate-metrics.sh

# Rewrite metric-driven regions of README.md from metrics/metrics.json.
metrics-readme: $(METRICS_JSON)
	bash scripts/regenerate-readme-metrics.sh

# Regenerate docs/paper/metrics.tex from metrics/metrics.json.
metrics-paper: $(METRICS_JSON)
	$(MAKE) -C docs/paper metrics

# CI gate : re-run the producer in a fresh state and fail if the result
# differs from the committed metrics.json. Mirrors `cargo fmt --check`.
#
# Keys that monotonically grow on every commit (commits, dev_days) are
# excluded from the comparison — they would always fail the gate in CI
# (which runs after the commit lands). Operators refresh those values
# in dedicated metrics-only commits ; the gate only checks the
# code-derived stable keys (sloc, tests, crates, doc_files) plus any
# perf metrics committed under the bench harness.
#
# Operators bumping a metric value commit the regenerated file. Editing
# metrics.json by hand to substitute a fresh measurement is allowed —
# the JSON's `measured_at` field records the date — but artefacts that
# consume it MUST be re-rendered in the same PR.
metrics-check:
	@if [ ! -f $(METRICS_JSON) ]; then \
	    echo "metrics-check: $(METRICS_JSON) missing — run 'make metrics'" >&2; \
	    exit 1; \
	fi
	@cp $(METRICS_JSON) $(METRICS_JSON).check-before
	bash scripts/regenerate-metrics.sh
	@jq 'del(.commits, .dev_days)' $(METRICS_JSON).check-before > $(METRICS_JSON).check-before.stable
	@jq 'del(.commits, .dev_days)' $(METRICS_JSON)               > $(METRICS_JSON).check-after.stable
	@if ! diff -q $(METRICS_JSON).check-before.stable $(METRICS_JSON).check-after.stable >/dev/null 2>&1; then \
	    echo "metrics-check: $(METRICS_JSON) drifted on stable keys — committed values differ from live measurement" >&2; \
	    diff -u $(METRICS_JSON).check-before.stable $(METRICS_JSON).check-after.stable || true; \
	    mv $(METRICS_JSON).check-before $(METRICS_JSON); \
	    rm -f $(METRICS_JSON).check-before.stable $(METRICS_JSON).check-after.stable; \
	    exit 1; \
	fi
	@mv $(METRICS_JSON).check-before $(METRICS_JSON)
	@rm -f $(METRICS_JSON).check-before.stable $(METRICS_JSON).check-after.stable
	@echo "metrics-check: $(METRICS_JSON) is fresh on stable keys"

clean-metrics:
	rm -f $(METRICS_JSON)
