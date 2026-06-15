# Launch Sequence

> Operator runbook for the OxyMake public launch. Derived from the publication
> premortem (**silence is worse than a teardown — prime the tribe before you
> fire**). This is a *sequence*, not a
> checklist to fire all at once. Each step gates the next.

## The one story

OxyMake's spreadable story is exactly one sentence:

> *"Your pipeline rebuilds everything after a `git checkout`, even when nothing
> changed. OxyMake stops that — the content of your data decides what re-runs."*

Everything that competes with that story is demoted on the landing page:

- **"Built by an agent fleet"** now lives in [`docs/MAKING-OF.md`](MAKING-OF.md),
  not the README. For a trust-critical cache engine it arms the wrong-subject HN
  teardown ("59k SLOC by AI agents, reviewed by one human") before the cache
  story earns any credibility. The contributor graph stays public; the fuse is
  not served on the landing page.
- **The formal/TLA+ paper** is a Documentation link, not a headline — it
  reassures *after* the try, not before.

## The smallest viable audience

One person: the **computational biologist / data scientist with a large
Snakemake DAG on a shared SLURM cluster who versions workflows in git** and
feels the phantom-rebuild pain daily. They live in the Snakemake Slack, on
r/bioinformatics, in nf-core GitHub issues. Reach *them*, in *their* channels,
*before* HN.

## Sequence (do not fire all at once)

### 0. Prerequisites (the BLOQUANT technical fixes must already be in)

Positioning cannot save a product that crashes on its own demo or a benchmark
that's refutable. These land first (separate molecules):

- `run:` crash fix (`task-20260609-7680`)
- benchmark integrity (`task-20260609-b539`)
- cache-key correctness (`task-20260609-cfeb`)

Plus the packaging in this branch: a **downloadable binary** must exist —
`cargo install` (Rust 1.85+, source build) kills the conda/pip onboarding.
GitHub Release tarballs are built by `.github/workflows/release.yml`; the
Homebrew formula template is in `packaging/homebrew/`; the PyPI launcher is in
`packaging/pypi/`.

### 1. Prime the tribe privately

Reach **5–10 Snakemake users one-to-one** before anything is public. Go through
connectors (nf-core maintainers, reproducibility-tutorial authors) rather than
cold-posting. Goal: **≥3 testimonials on their real DAGs** — concrete, e.g.
*"saved me half a day on my RNA-seq pipeline."* Without this the launch goes
cold, and cold is the failure mode that's worse than a teardown.

### 2. Public repo + testimonials + binary

Flip the repo public **with** the testimonials already in hand and the
downloadable binary live. A repo that goes public with social proof and a
five-minute onboarding is a repo someone can adopt; one that goes public bare is
a repo that confirms the "even the agents abandoned it" suspicion.

### 3. Show HN — problem-framed title

Title frames the *problem*, not the construction:

> *"Show HN: Your pipeline rebuilds after `git checkout` — this stops that"*

The primed users are present in the thread to anchor it on the cache story. The
README has already pre-empted the two five-minute teardowns (sequential `-j`,
unshipped Kubernetes, the cold-run caveat travelling *with* the 33.3× number) —
a claim you correct yourself is honesty; one the reader corrects is a lie.

### 4. arXiv — decoupled, cross-listed

Submit the paper a few weeks **after** the HN launch (decoupled so neither
artifact's failure drags the other). Cross-list **cs.SE** alongside cs.DC — the
Nix/Guix / reproducible-builds tribe already loves content-addressing and is the
paper's natural reader.

## Channel order, in one line

> Snakemake Slack & r/bioinformatics (through connectors) → public repo +
> testimonials + binary → Show HN (problem-framed) → arXiv (decoupled, cs.SE
> cross-list).
