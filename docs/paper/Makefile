PAPER = oxymake-paper
TEX = pdflatex
BIB = bibtex

.PHONY: all clean view metrics

all: $(PAPER).pdf

# metrics.tex is regenerated on every build so the paper always quotes the
# workspace's current reality (SLOC, test count, crate count, commit count, ...).
# Run with OXYMAKE_REGEN_COVERAGE=1 to also refresh \metricCoverage via llvm-cov
# (slow: forces an instrumented rebuild).
metrics:
	bash scripts/regenerate-metrics.sh

$(PAPER).pdf: $(PAPER).tex references.bib metrics
	$(TEX) $(PAPER)
	$(BIB) $(PAPER)
	$(TEX) $(PAPER)
	$(TEX) $(PAPER)

clean:
	rm -f $(PAPER).pdf $(PAPER).aux $(PAPER).bbl $(PAPER).blg \
	      $(PAPER).log $(PAPER).out $(PAPER).toc $(PAPER).fls \
	      $(PAPER).fdb_latexmk $(PAPER).synctex.gz

view: $(PAPER).pdf
	open $(PAPER).pdf
