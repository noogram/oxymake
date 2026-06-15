---
name: CWL import request
about: Request CWL (Common Workflow Language) import support in ox-translate.
title: "[CWL] <short description of your workflow>"
labels: ["adapter:cwl", "trigger-candidate"]
assignees: []
---

> **Why this template exists.** The `ox-translate cwl` adapter is **deferred
> under a composite trigger** (proposal:
> [`docs/proposals/ox-translate-cwl-trigger.md`](../../docs/proposals/ox-translate-cwl-trigger.md)).
> A real, named external request with a concrete CWL workflow is one of the
> two conditions that unlock implementation. Please fill in every section
> below — incomplete requests cannot move the trigger.

## Who you are

- **Name** (real name, not handle):
- **Organization / project** (optional):
- **Contact** (email, GitHub handle, or similar):

> *Anonymous requests do not satisfy the named-adopter clause and will not
> advance the trigger.*

## The CWL workflow you want to run

- **Workflow source** (link to a public repo, file, or paste the YAML):
- **Approximate size** (number of steps / tools, total LOC):
- **CWL version** (v1.0 / v1.1 / v1.2):
- **Domain** (bioinformatics, ML, ETL, other):

> *"It would be nice to support CWL someday" does not satisfy the
> concrete-workflow clause. We need a workflow you actually want to run on
> Oxymake.*

## Why Oxymake (not your current CWL runner)

What does Oxymake offer for your use case that cwltool / Toil / Arvados /
other CWL runners do not?

## Constraints

- **Execution backend you need** (local / Guix / containers / HPC):
- **Reproducibility requirements** (none / soft / strict):
- **Deadline** (if any):

## Anything else

Free-form context. Prior conversations with the maintainer? A link to
a discussion thread? Constraints we should know about?

---

**Maintainer note.** When this issue is opened with all sections complete
*and* `ox-exec-guix` has shipped (see proposal §"Condition 1"), both
trigger conditions are satisfied — nucleate
`task-ox-translate-cwl-implementation` and link back to this issue. If
`ox-exec-guix` has not yet shipped, label this issue
`trigger-pending:ox-exec-guix` and revisit when the backend ships.
