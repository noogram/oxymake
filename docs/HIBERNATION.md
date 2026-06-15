# Hibernation Protocol

> **Audience:** the operator (or a successor) returning to OxyMake after a
> long absence — or anyone who finds a `.hibernation` file at the repo root.
> **Last reviewed:** 2026-06-10.

## Sleep is a legitimate state, not a failure

Pre-mortem #3 found that the binding constraint on OxyMake is **one operator
with finite energy** (convergence C1). Every other failure mode — spec rot, an
empty named-reader ledger, an unstaffed "community" — is that one constraint
seen through a different surface. The honest conclusion (Decision-relevant
tension #4): trajectory **(c) "sommeil"** — the project sleeping — is a
*plannable, legitimate state*, not a defeat to be hidden. Pretending that
(a) "traction" is the only acceptable outcome is itself the hidden hypothesis
the pre-mortem refutes: a gate no one will enforce.

So this document makes **going to sleep, staying asleep, and waking up** all
explicit and observable.

## How hibernation is declared (operator gesture, not a clock)

> **Operator decision 2026-06-10 (premortem PM#5):** all temporal gates were
> removed from this repository. An earlier version of this protocol used a
> scheduled CI job (`hibernation-check.yml`) firing on commit-staleness timers
> and calendar deadlines. That job is gone: nothing calendar-based may change
> this repository's state or redden its badge. Hibernation is now declared the
> same way everything else in this project is decided — by an explicit,
> dated, attributable operator commit.

To put the project to sleep, the operator (and only the operator) commits, in
one commit:

1. A `.hibernation` file at the repo root:

   ```
   IN HIBERNATION
   reason: <one honest sentence>
   as-of: YYYY-MM-DD
   see: docs/HIBERNATION.md
   ```

2. A banner inside the `HIBERNATION-BANNER` region of `README.md`:

   > 💤 **IN HIBERNATION** — \<reason\>. as of \<date\>. See [docs/HIBERNATION.md](HIBERNATION.md).

`.hibernation` is the single machine-readable signal. Its presence means: the
repo is asleep. Its absence means: awake. Nothing else needs to be consulted
to know which.

This is honest about what it is: a self-declared state. What keeps it from
being silent decay is the **maintenance policy in `README.md`**, which tells
every reader up front that the project is best-effort and single-maintainer —
so an unanswered issue reads as "slow by contract", not as an undeclared
abandonment. The failure mode this protocol guards against is not slowness;
it is *pretending to be awake*.

## Awakening protocol

You are reading this because you came back, or because someone else did. Do
**not** start building. Run the floor check first, exactly as
[`docs/RETURNING.md`](RETURNING.md) prescribes — hibernation does not exempt you
from the canary; it adds a few steps before it.

1. **Read the signal.** Does `.hibernation` exist? If so, open it: it records
   the reason and the date. That tells you what kind of sleep this was.
2. **Run the cognitive canary.** `bash scripts/re-warm.sh` (RETURNING.md §1).
   If the build floor is rotten, fix it before anything else. A repo that
   slept a long time will likely need a `cargo update` and a toolchain bump
   first.
3. **Re-run the drift monitor.** Confirm the Drift Tripwire
   (`.github/workflows/drift-tripwire.yml`) and the latest CI run are green.
   Refresh the attestation table at the head of `OXYMAKE-THESIS.md` — every
   principle whose attestation went stale during sleep is now aspiration, not
   invariant, until re-attested.
4. **Decide consciously, then clear the signal.** If you are genuinely
   resuming: delete `.hibernation` and clear the `HIBERNATION-BANNER` region of
   `README.md` **in the same commit** as your first substantive change — not
   before. Clearing the banner without resuming is exactly the self-waiver this
   protocol exists to prevent.

### What signals a real awakening (vs. a single nostalgic visit)

A drive-by `git pull` is not an awakening. The honest signal that the
project should restart comes from **a named second party**, the one thing
pre-mortem #3 says the project never had (C1, D2):

- a third party opens a GitHub **issue** or **PR** against the repo, or
- a **named contributor** asks to take over (see successor handoff below), or
- the operator returns with a *dated, scoped* intent (a bead with an exit
  criterion), not a vague "maybe I'll poke at it."

If none of these is present, the right move is to leave it asleep. Sleep is a
state, not a backlog.

## Passing to open-source community-driven mode

There is a threshold beyond sleep: the operator decides he is **not coming
back**. This is also legitimate and must be made explicit rather than left to
silent decay (the "abandoned-looking" failure, pre-mortem #3 adversary).

**The threshold gesture.** When the operator concludes he is no longer
maintaining OxyMake, he writes — in plain words, signed and dated — at the top
of `README.md`:

> **I am no longer maintaining this project. See [docs/HIBERNATION.md](docs/HIBERNATION.md). — Emmanuel Sérié, <date>**

No euphemism ("on hold", "exploring options"). The word is *not maintaining*.
This single honest sentence is worth more than any "deferred to a community
effort" alibi, which pre-mortem #3 (godin) identified as the most expensive
silence — a named obligation with no owner.

**Inviting a successor.** To hand the project to someone real:

1. Add a `CODEOWNERS` file naming the new maintainer's GitHub handle on `*`.
2. Write `docs/SUCCESSOR.md`: who is taking over, what they committed to, on
   what date — the *one real second party with one real obligation on one date
   the operator cannot move* that pre-mortem #3 (D2) prescribes as the only real
   fix. A blank `CODEOWNERS` or a "community" with no handle is **not** a
   successor; it is the empty-ledger pathology again.
3. Until a successor is named, the project stays in hibernation, not in a
   fictional community mode. "Community" is a face, never a category.

## Discipline of this file

- ≤ 200 lines, readable in 5 minutes.
- The trigger is an explicit operator gesture — never a clock, never a
  calendar (operator decision 2026-06-10, premortem PM#5).
- One machine signal: `.hibernation` at repo root.
- Referenced from `README.md` and `docs/RETURNING.md` as *the document to
  read if you return after a long absence.*
