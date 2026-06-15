---- MODULE CancelPropagation ----
\* This spec defends invariant INV-3 (Cancel-Propagation vs In-Flight
\* Materialization) for oxymake `ox run` (ADR-013).
\* It models the propagation of `oxc cancel` from CLI intent through
\* the scheduler and bridge to in-flight workers, including the ADR-013
\* distinction between JobSkipped (cache hit) and JobCancelled
\* (intent-driven). This is a Full CSTAFP surface — the oxymake
\* analogue of the AWS DynamoDB 35-step trace lives here (Newcombe
\* et al., CACM 58(4), 2015 — 4 actors, 23 demonstrated steps + 12 of
\* cascade, violates CancelledNeverCached, JobFailedImpliesNoIntent,
\* EvictPrecedesUnregister simultaneously).
\* Justification: ADR-015 (named invariants), ADR-013 (cancelled vs
\* skipped), design panel §5 D-1 + Gödel §3 + Knuth INV-3 §ii.
\* Out of model (see docs/architecture/boundary.md):
\*   - ExecutorHonest                  (bridge.cancel_job truthful)
\*   - ExecutorFailureClassification   (status \in {Done,Failed,Cancelled,Lost})
\*   - StorageDeleteAtomic             (unlink(2) is atomic)

EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Jobs, MaxBytes
ASSUME Jobs # {}
ASSUME MaxBytes \in Nat

VARIABLES intent, claim, observed, status, bridge_ack, bytes_written, cached
vars == <<intent, claim, observed, status, bridge_ack, bytes_written, cached>>

IntentDomain == {"none", "cancel"}
ClaimDomain  == {"unclaimed", "claimed"}
StatusDomain == {"Pending", "Running", "Succeeded", "Failed", "Cancelled"}
AckDomain    == {"none", "acked"}

TypeOK ==
    /\ intent        \in [Jobs -> IntentDomain]
    /\ claim         \in [Jobs -> ClaimDomain]
    /\ observed      \in [Jobs -> BOOLEAN]
    /\ status        \in [Jobs -> StatusDomain]
    /\ bridge_ack    \in [Jobs -> AckDomain]
    /\ bytes_written \in [Jobs -> 0 .. MaxBytes]
    /\ cached        \in [Jobs -> BOOLEAN]

Terminal(j) == status[j] \in {"Succeeded", "Failed", "Cancelled"}

Init ==
    /\ intent        = [j \in Jobs |-> "none"]
    /\ claim         = [j \in Jobs |-> "unclaimed"]
    /\ observed      = [j \in Jobs |-> FALSE]
    /\ status        = [j \in Jobs |-> "Pending"]
    /\ bridge_ack    = [j \in Jobs |-> "none"]
    /\ bytes_written = [j \in Jobs |-> 0]
    /\ cached        = [j \in Jobs |-> FALSE]

\* ---------- Actions ----------

\* Scheduler claims a Pending job and transitions it to Running.
\* Refuses to start a job under cancel intent (intercept-at-frontier).
StartJob(j) ==
    /\ claim[j] = "unclaimed" /\ status[j] = "Pending"
    /\ intent[j] = "none"
    /\ claim' = [claim EXCEPT ![j] = "claimed"]
    /\ status' = [status EXCEPT ![j] = "Running"]
    /\ UNCHANGED <<intent, observed, bridge_ack, bytes_written, cached>>

\* CLI emits the cancel intent (`oxc cancel j`) — once cancel, stays cancel.
IssueCancel(j) ==
    /\ intent[j] = "none" /\ ~Terminal(j)
    /\ intent' = [intent EXCEPT ![j] = "cancel"]
    /\ UNCHANGED <<claim, observed, status, bridge_ack, bytes_written, cached>>

\* Scheduler observes the cancel intent in the state DB and arms the bridge.
SchedulerObservesIntent(j) ==
    /\ intent[j] = "cancel" /\ observed[j] = FALSE
    /\ observed' = [observed EXCEPT ![j] = TRUE]
    /\ UNCHANGED <<intent, claim, status, bridge_ack, bytes_written, cached>>

\* Bridge issues bridge.cancel_job to the executor; honest by the
\* ExecutorHonest substrate axiom (see docs/architecture/boundary.md).
\* Covers both Pending (intercept-before-start) and Running (in-flight)
\* paths — the terminal MUST be Cancelled (ADR-013), never Failed/Succeeded.
BridgeCancel(j) ==
    /\ observed[j] = TRUE /\ status[j] \in {"Pending", "Running"}
    /\ status' = [status EXCEPT ![j] = "Cancelled"]
    /\ UNCHANGED <<intent, claim, observed, bridge_ack, bytes_written, cached>>

\* Worker writes b bytes to materialization m_j (pre-SIGTERM). Bounded for TLC.
WorkerWriteByte(j) ==
    /\ status[j] = "Running" /\ bytes_written[j] < MaxBytes
    /\ bytes_written' = [bytes_written EXCEPT ![j] = @ + 1]
    /\ UNCHANGED <<intent, claim, observed, status, bridge_ack, cached>>

\* Worker finalizes terminally. Per ADR-013, Failed/Succeeded only when
\* intent is "none" — cancel paths flow through BridgeCancel, not here.
\* This encodes the corrected post-ADR-013 design: cancel never masquerades
\* as IoError (cf. Gödel §3 steps 13–15 — the bug the design prevents).
WorkerFinalize(j, s) ==
    /\ status[j] = "Running" /\ s \in {"Succeeded", "Failed"}
    /\ intent[j] = "none"
    /\ status' = [status EXCEPT ![j] = s]
    /\ cached' = [cached EXCEPT ![j] = (s = "Succeeded")]
    /\ UNCHANGED <<intent, claim, observed, bridge_ack, bytes_written>>

\* Bridge synchronises terminal status back to state DB.
BridgeAck(j) ==
    /\ Terminal(j) /\ bridge_ack[j] = "none"
    /\ bridge_ack' = [bridge_ack EXCEPT ![j] = "acked"]
    /\ UNCHANGED <<intent, claim, observed, status, bytes_written, cached>>

Next ==
    \E j \in Jobs:
        \/ StartJob(j)
        \/ IssueCancel(j)
        \/ SchedulerObservesIntent(j)
        \/ BridgeCancel(j)
        \/ WorkerWriteByte(j)
        \/ \E s \in {"Succeeded", "Failed"}: WorkerFinalize(j, s)
        \/ BridgeAck(j)

Fairness ==
    /\ \A j \in Jobs: WF_vars(SchedulerObservesIntent(j))
    /\ \A j \in Jobs: WF_vars(BridgeCancel(j))
    /\ \A j \in Jobs: WF_vars(BridgeAck(j))

Spec == Init /\ [][Next]_vars /\ Fairness

\* ---------- Safety invariants ----------

\* INV-3.a CancelMonotone — once intent[j] = "cancel", it stays "cancel"
\* until terminal status (no action ever resets intent to "none").
\* Stated as a property (action-level temporal formula).
CancelMonotone ==
    [][\A j \in Jobs: intent[j] = "cancel" => intent'[j] = "cancel"]_vars

\* INV-3.b NoZombieRunning — a job whose cancel has been acked by the
\* bridge is never stuck in Running.
NoZombieRunning ==
    \A j \in Jobs:
        (intent[j] = "cancel" /\ bridge_ack[j] = "acked")
            => status[j] # "Running"

\* INV-3.c CancelledNeverCached — the heart of INV-3 (Knuth §ii):
\* a Cancelled job never has its (partial) materialization committed.
CancelledNeverCached ==
    \A j \in Jobs: (status[j] = "Cancelled") => (cached[j] = FALSE)

\* INV-3.d JobFailedImpliesNoIntent — the ADR-013 distinction in force:
\* a Failed status is never a masked cancel (Gödel §3 step 15 — the
\* 35-step counter-trace bug this design eliminates by construction).
JobFailedImpliesNoIntent ==
    \A j \in Jobs: (status[j] = "Failed") => (intent[j] = "none")

\* ---------- Liveness ----------

\* INV-3.L CancelEventuallyAcked — every cancel intent is eventually
\* acked by the bridge (closes the propagation chain).
CancelEventuallyAcked ==
    \A j \in Jobs: (intent[j] = "cancel") ~> (bridge_ack[j] = "acked")

\* ---------- Counter-trace correspondence (Gödel §3) ----------
\* Steps 1–8 of the 23-step trace live in S1 (EvictionRace, bd-tracked).
\* Steps 9–13 live here: 9–10 = IssueCancel(j); 11 = absence of
\* SchedulerObservesIntent(j); 13 = WorkerFinalize(j,"Failed"). Step 13
\* is the load-bearing one — WorkerFinalize requires intent[j]="none",
\* so once IssueCancel fires (step 9), Failed is disabled and only
\* BridgeCancel can terminate j (status="Cancelled", cached=FALSE).
\* JobFailedImpliesNoIntent and CancelledNeverCached therefore hold by
\* construction; if TLC ever reports a counter-example, log it in
\* spec/tla/TRACES.md per ADR-015 sunset clause.
====
