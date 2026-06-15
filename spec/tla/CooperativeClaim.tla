---- MODULE CooperativeClaim ----
\* This spec defends invariant INV-2 (Multi-Session Reclaim-Claim
\* Atomicity) for oxymake ADR-012.
\* It models 4 sessions with heartbeat-driven liveness detection and
\* TTL-based reclaim. This is a Full CSTAFP surface (independently-
\* failing peers, cross-peer safety relation NoDoubleRunning).
\* Justification: ADR-015 (named invariants), ADR-012 (multi-session
\* reclaim), Newcombe et al. (CACM 2015 — the DynamoDB 35-step trace
\* lives in this same hazard class).
\* Out of model (see docs/architecture/boundary.md):
\*   - ExecutorHonest, NoOOMCascade
\*   - StateDbAtomicCommit (assumed: SQLite COMMIT all-or-nothing,
\*     so the reclaim_stale_jobs transaction at session.rs §159–172
\*     is atomic w.r.t. concurrent writes; the hazard the spec
\*     exercises is the find_stale → reclaim gap, not the reclaim
\*     transaction itself).
\*     DEPLOYMENT CAVEAT (2026-06-10 pre-pub pass): this axiom holds
\*     on a local filesystem with working POSIX locks. It is FALSE on
\*     NFS/Lustre/GPFS, where file locking is unreliable — exactly the
\*     shared-workspace deployments where multi-session execution is
\*     most tempting. The axiom is discharged by the engine-level
\*     requirement that .oxymake/ reside on local disk (paper §6.2,
\*     "SQLite on network filesystems"), not by SQLite alone. Point
\*     the state DB at NFS and every invariant below loses its premise.
\* Note: variable `alive` extends the four state elements named in
\* ADR-015's preamble template — Crash(s) requires a state predicate
\* to be enforceable; the deviation is documented here so the choice
\* is grep-able.

EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Sessions, Jobs, TTL, MaxClock, NoSession,
          SessionFilterEnabled
ASSUME NoSession \notin Sessions
ASSUME SessionFilterEnabled \in BOOLEAN

\* `local_running` and `done_by` were added in the 2026-06-10 pre-pub
\* pass (premortem finding H18): without a per-session *belief*
\* variable, the original NoDoubleRunning was structurally true — the
\* single-valued claim_session function cannot express two sessions
\* each believing they own a job, so TLC could never have found the
\* zombie-terminalization bug class. The belief variable makes the
\* hazard expressible; DoneByClaimHolder below is the invariant that
\* an unguarded terminal write violates (see TRACES.md entry 2026-06-10
\* and the session-filter fix in ox-state db.rs complete_job/fail_job).
VARIABLES heartbeat, status, claim_session, clock, alive,
          local_running, done_by

vars == <<heartbeat, status, claim_session, clock, alive,
          local_running, done_by>>

\* Type invariant.
TypeOK ==
    /\ heartbeat \in [Sessions -> Nat]
    /\ status \in [Jobs -> {"pending", "running", "done"}]
    /\ claim_session \in [Jobs -> Sessions \cup {NoSession}]
    /\ clock \in Nat
    /\ alive \in SUBSET Sessions
    /\ local_running \in [Sessions -> SUBSET Jobs]
    /\ done_by \in [Jobs -> Sessions \cup {NoSession}]

Init ==
    /\ heartbeat = [s \in Sessions |-> 0]
    /\ status = [j \in Jobs |-> "pending"]
    /\ claim_session = [j \in Jobs |-> NoSession]
    /\ clock = 0
    /\ alive = Sessions
    /\ local_running = [s \in Sessions |-> {}]
    /\ done_by = [j \in Jobs |-> NoSession]

\* clock advances — necessary for staleness to become detectable.
Tick ==
    /\ clock < MaxClock
    /\ clock' = clock + 1
    /\ UNCHANGED <<heartbeat, status, claim_session, alive,
                   local_running, done_by>>

\* Heartbeat(s) — alive session updates heartbeat_at to current clock.
Heartbeat(s) ==
    /\ s \in alive
    /\ heartbeat' = [heartbeat EXCEPT ![s] = clock]
    /\ UNCHANGED <<status, claim_session, clock, alive,
                   local_running, done_by>>

\* Claim(s, j) — atomic conditional UPDATE.
\* Models session.rs `try_claim_job(j)`:
\*   UPDATE jobs SET status='running', session_id=?
\*   WHERE id=? AND status='pending'
\* The SQL precondition is `status='pending'`, NOT session liveness.
\* A session that appears stale (or is interrupted) can still race to
\* claim if its UPDATE arrives before the row is re-pending — this is
\* exactly the late-claim arm of the INV-2 hazard.
\* The winning session also records the job in its local belief set —
\* its in-process scheduler state, which no other process can reset.
Claim(s, j) ==
    /\ status[j] = "pending"
    /\ status' = [status EXCEPT ![j] = "running"]
    /\ claim_session' = [claim_session EXCEPT ![j] = s]
    /\ local_running' = [local_running EXCEPT ![s] = @ \cup {j}]
    /\ UNCHANGED <<heartbeat, clock, alive, done_by>>

\* IsStale(s) — predicate used by FindStale and Reclaim.
IsStale(s) == clock - heartbeat[s] >= TTL

\* FindStale — read-only query; modelling it as a distinct event makes
\* the find_stale → reclaim gap visible in TLC traces.
FindStale ==
    /\ \E s \in Sessions : IsStale(s)
    /\ UNCHANGED vars

\* Reclaim(s_old) — re-checks staleness (mirroring the SQL WHERE
\* clause) but cannot prevent a concurrent Heartbeat from s_old
\* arriving immediately after — the zombie revival scenario.
\* Models session.rs `reclaim_stale_jobs` (one transaction).
\* Deliberately does NOT touch local_running[s_old]: reclaim runs in
\* another process and cannot reach into the stale session's memory.
\* From this point on, s_old's belief and the DB diverge — that
\* divergence is the zombie window the terminal-write guard closes.
Reclaim(s_old) ==
    /\ IsStale(s_old)
    /\ status' = [j \in Jobs |->
                    IF claim_session[j] = s_old /\ status[j] = "running"
                    THEN "pending"
                    ELSE status[j]]
    /\ claim_session' = [j \in Jobs |->
                    IF claim_session[j] = s_old /\ status[j] = "running"
                    THEN NoSession
                    ELSE claim_session[j]]
    /\ UNCHANGED <<heartbeat, clock, alive, local_running, done_by>>

\* Terminalize(s, j) — a session that believes it is running j issues
\* the terminal UPDATE (complete_job / fail_job in ox-state db.rs):
\*   UPDATE jobs SET status='completed'
\*   WHERE id=? AND status='running' AND session_id=?
\* The `AND session_id=?` arm is the zombie guard: a session whose
\* claim was reclaimed (and re-claimed by a peer) gets rows=0 and its
\* stale result never lands. The guard is reified as the constant
\* SessionFilterEnabled — TRUE models the shipped code, FALSE models
\* the pre-guard WHERE clause (status='running' only). Under FALSE,
\* TLC violates DoneByClaimHolder in a few steps
\* (CooperativeClaimUnguarded.cfg, TRACES.md 2026-06-10). Either way
\* the session drops j from its belief set: the in-process scheduler
\* moves on after the write attempt.
Terminalize(s, j) ==
    /\ j \in local_running[s]
    /\ local_running' = [local_running EXCEPT ![s] = @ \ {j}]
    /\ IF status[j] = "running"
          /\ (claim_session[j] = s \/ ~SessionFilterEnabled)
       THEN /\ status' = [status EXCEPT ![j] = "done"]
            /\ done_by' = [done_by EXCEPT ![j] = s]
       ELSE UNCHANGED <<status, done_by>>
    /\ UNCHANGED <<heartbeat, claim_session, clock, alive>>

\* Crash(s) — session disappears without further heartbeat (NTP skew,
\* suspend/resume, OS kill). The runtime cannot distinguish crashed
\* from suspended; modelling it as a state mutation lets TLC explore
\* the interleavings where a "dead" session never resurfaces.
\* local_running[s] survives: a suspended process resumes with its
\* belief intact (the zombie scenario), and modelling crash as belief
\* erasure would hide exactly the interleavings INV-2 defends against.
Crash(s) ==
    /\ s \in alive
    /\ alive' = alive \ {s}
    /\ UNCHANGED <<heartbeat, status, claim_session, clock,
                   local_running, done_by>>

Next ==
    \/ Tick
    \/ \E s \in Sessions : Heartbeat(s)
    \/ \E s \in Sessions, j \in Jobs : Claim(s, j)
    \/ FindStale
    \/ \E s \in Sessions : Reclaim(s)
    \/ \E s \in Sessions, j \in Jobs : Terminalize(s, j)
    \/ \E s \in Sessions : Crash(s)

Spec == Init /\ [][Next]_vars

\* ---- Safety invariants ----

\* INV-2-Safety-1: NoDoubleRunning, in two layers.
\*
\* DB layer — HONESTY NOTE (2026-06-10 pre-pub pass, premortem H18):
\* the original formulation
\*   Cardinality({s : status[j]="running" /\ claim_session[j]=s}) <= 1
\* is TRUE BY TYPING: claim_session is a function, the set can never
\* have two elements, and TLC checking it proves nothing about the
\* system. It is kept below (NoDoubleRunningDb) as the formal statement
\* of the DB-side relation, with its vacuity named.
\*
\* Belief layer — the real content of INV-2: two sessions can
\* simultaneously BELIEVE they run the same job (zombie + reclaim +
\* re-claim), and the model reaches such states. Belief-level
\* exclusivity is therefore deliberately NOT an invariant — asserting
\* NoDoubleRunningBelief makes TLC produce the zombie window in a few
\* steps. What the system actually guarantees is DoneByClaimHolder:
\* only the current claim holder's terminal write lands.
NoDoubleRunningDb ==
    \A j \in Jobs :
        Cardinality({s \in Sessions :
                       status[j] = "running" /\ claim_session[j] = s}) <= 1

\* NOT an invariant — documented refutable formulation. Add it to the
\* INVARIANTS clause to watch TLC exhibit the zombie window.
NoDoubleRunningBelief ==
    \A j \in Jobs :
        Cardinality({s \in Sessions : j \in local_running[s]}) <= 1

\* INV-2-Safety-4: DoneByClaimHolder — the falsifiable core of INV-2.
\* A job only ever reaches "done" through the session that held its
\* claim at that moment. This is exactly what the `AND session_id=?`
\* arm of complete_job/fail_job enforces; drop the
\* `claim_session[j] = s` conjunct in Terminalize and TLC refutes this
\* in a few steps (see TRACES.md, 2026-06-10).
DoneByClaimHolder ==
    \A j \in Jobs :
        status[j] = "done" => done_by[j] = claim_session[j]

\* INV-2-Safety-2: AtMostOneClaimer — claimed iff running-or-done.
\* Captures Claim atomicity: there is never a half-state where status
\* is updated but claim_session is not (or vice-versa). Reclaim and
\* Claim each update both fields in lockstep; Terminalize keeps the
\* claim on the done row (the DB retains session_id for audit).
AtMostOneClaimer ==
    \A j \in Jobs :
        (status[j] \in {"running", "done"}) <=> (claim_session[j] \in Sessions)

\* INV-2-Safety-3: ReclaimImpliesStale — enforced as the precondition
\* `IsStale(s_old)` on the Reclaim action. A state-only formulation is
\* undecidable (the past staleness witness is not in the current state),
\* so the property is carried by the action's enabling guard rather
\* than by an INVARIANT clause.

\* ---- Liveness ----

\* EventualClaim — every pending job is eventually claimed, provided
\* at least one session remains alive. Without alive sessions, the
\* liveness fails trivially; the disjunction with `alive = {}` makes
\* the property non-vacuous in the face of universal Crash.
EventualClaim ==
    \A j \in Jobs :
        (status[j] = "pending" /\ alive # {}) ~>
            (status[j] = "running" \/ alive = {})

\* Fair specification — WF on Tick + WF on Claim per (alive session,
\* job) pair. Required for EventualClaim to hold under TLC.
FairSpec ==
    /\ Init
    /\ [][Next]_vars
    /\ WF_vars(Tick)
    /\ \A s \in Sessions :
       \A j \in Jobs :
         WF_vars(s \in alive /\ Claim(s, j))

====
