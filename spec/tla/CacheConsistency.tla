---- MODULE CacheConsistency ----
\* This spec defends invariants OX-1 (cache key purity) and OX-6
\* (register-before-read) for oxymake `ox run`.
\* It models the steady-state Claim/Materialize/CacheHit interleaving
\* of an intra-process N-worker pool, a CSTAFP-lite surface.
\* Justification: ADR-015 (named invariants) + Newcombe et al. (CACM 2015).
\* Out of model (see docs/architecture/boundary.md):
\*   - ExecutorHonest         (executor returns truthful status)
\*   - UserCodeMatchesRule    (subprocess implements rule contract)
\* Cross-references:
\*   - EvictionRace.tla owns the refcount/eviction race (bd-tracked,
\*     activated when Stage 2 memory-pressure code lands)
\*   - CrashRecovery.tla owns the post-crash reconciliation
\*     (bd-tracked, activated when ADR-014 implementation finalised)

EXTENDS Naturals, FiniteSets, Sequences, TLC

CONSTANTS Workers, Rules, NoClaim, KeyVariants

\* OX-1 at the model level (2026-06-10 pre-pub pass, premortem H18).
\* Earlier revisions refined ContentKey as the identity function, which
\* made CacheKeyDeterminism TRIVIALLY TRUE — the same rule mapped to
\* the same value by definition of `==`, so TLC checking it proved
\* nothing. The key computation is now modelled honestly: each
\* materialisation draws a key variant from KeyVariants, and a worker
\* crash before registration lets a second worker recompute the key
\* for the same rule. CacheKeyDeterminism is then a real claim about
\* the computation history:
\*   - KeyVariants = {1}   — the BLAKE3-determinism axiom holds (key is
\*     a pure function of the rule's declared inputs): TLC passes.
\*     This is the shipped configuration (CacheConsistency.cfg).
\*   - KeyVariants = {1,2} — an undeclared input (mtime, env drift,
\*     non-hermetic tool) leaks into the key: TLC refutes
\*     CacheKeyDeterminism in a few steps. This red run is committed as
\*     CacheConsistencyNondetKey.cfg and archived via run-tlc.sh, so
\*     the invariant is demonstrably falsifiable, not tautological.
\* The completeness risk itself — WHETHER an undeclared input exists —
\* remains out of model (paper §3.1 threat model, boundary.md).
ASSUME 0 \notin KeyVariants  \* 0 is the "no key computed" sentinel

VARIABLES
    claim,         \* Workers -> Rules \cup {NoClaim}: current claim per worker
    materialized,  \* SUBSET (Workers \X Rules): worker has produced bytes
    register_log,  \* Seq(Rules \X KeyVariants): atomic (rule, key) commit log
    cached_seen,   \* SUBSET (Workers \X Rules): worker observed a cache hit
    wkey,          \* Workers -> KeyVariants \cup {0}: key computed by worker
    computed       \* SUBSET (Rules \X KeyVariants): key-computation history

vars == << claim, materialized, register_log, cached_seen, wkey, computed >>

TypeOK ==
    /\ claim \in [Workers -> Rules \cup {NoClaim}]
    /\ materialized \subseteq (Workers \X Rules)
    /\ register_log \in Seq(Rules \X KeyVariants)
    /\ cached_seen \subseteq (Workers \X Rules)
    /\ wkey \in [Workers -> KeyVariants \cup {0}]
    /\ computed \subseteq (Rules \X KeyVariants)

Init ==
    /\ claim = [w \in Workers |-> NoClaim]
    /\ materialized = {}
    /\ register_log = << >>
    /\ cached_seen = {}
    /\ wkey = [w \in Workers |-> 0]
    /\ computed = {}

\* Rules currently visible in the commit log (read off register_log).
RegisteredRules == { register_log[i][1] : i \in DOMAIN register_log }

\* Cooperative claim: at most one worker holds a given rule at any time.
NoConflictingClaim(w, r) ==
    \A w2 \in Workers : (w2 # w) => (claim[w2] # r)

\* Claim: a worker takes exclusive responsibility for a rule that
\* is not yet registered, provided no other worker already holds it.
Claim(w, r) ==
    /\ claim[w] = NoClaim
    /\ r \notin RegisteredRules
    /\ NoConflictingClaim(w, r)
    /\ claim' = [claim EXCEPT ![w] = r]
    /\ UNCHANGED << materialized, register_log, cached_seen, wkey, computed >>

\* Materialize: a worker produces bytes for its claimed rule and
\* computes the rule's cache key. This is the heavy step (subprocess
\* execution under UserCodeMatchesRule). The key variant is drawn from
\* KeyVariants — a singleton when hashing is a pure function of the
\* declared inputs, larger when an undeclared input leaks in.
Materialize(w, r) ==
    /\ claim[w] = r
    /\ <<w, r>> \notin materialized
    /\ \E n \in KeyVariants :
        /\ wkey' = [wkey EXCEPT ![w] = n]
        /\ computed' = computed \cup {<<r, n>>}
    /\ materialized' = materialized \cup {<<w, r>>}
    /\ UNCHANGED << claim, register_log, cached_seen >>

\* RegisterMaterialization: atomic append to the commit log, releasing
\* the claim. This is the step that makes the rule visible to other
\* workers. OX-6 is enforced by ordering: read paths only fire after
\* this step has appended to register_log. The key registered is the
\* one this worker computed at materialisation time.
RegisterMaterialization(w, r) ==
    /\ claim[w] = r
    /\ <<w, r>> \in materialized
    /\ wkey[w] \in KeyVariants
    /\ r \notin RegisteredRules
    /\ register_log' = Append(register_log, <<r, wkey[w]>>)
    /\ claim' = [claim EXCEPT ![w] = NoClaim]
    /\ wkey' = [wkey EXCEPT ![w] = 0]
    /\ UNCHANGED << materialized, cached_seen, computed >>

\* WorkerCrash: a worker dies between claim and registration. Its
\* partial bytes are discarded on restart (materialized entry dropped)
\* and the claim is released, so a peer can re-claim the rule and
\* recompute its key — the interleaving that makes CacheKeyDeterminism
\* falsifiable. The computation history (`computed`) survives: the key
\* WAS derived once, and determinism is a claim about every derivation
\* ever made, not only the ones that reached the log.
WorkerCrash(w) ==
    /\ claim[w] # NoClaim
    /\ materialized' = materialized \ {<<w, claim[w]>>}
    /\ claim' = [claim EXCEPT ![w] = NoClaim]
    /\ wkey' = [wkey EXCEPT ![w] = 0]
    /\ UNCHANGED << register_log, cached_seen, computed >>

\* CacheHit: a worker observes the rule already in the commit log and
\* skips re-materialisation. The enabling clause `r \in RegisteredRules`
\* is the load-bearing premise of OX-6 — see RegisterPrecedesRead below.
CacheHit(w, r) ==
    /\ claim[w] = NoClaim
    /\ r \in RegisteredRules
    /\ <<w, r>> \notin cached_seen
    /\ cached_seen' = cached_seen \cup {<<w, r>>}
    /\ UNCHANGED << claim, materialized, register_log, wkey, computed >>

Next ==
    \E w \in Workers, r \in Rules :
        \/ Claim(w, r)
        \/ Materialize(w, r)
        \/ RegisterMaterialization(w, r)
        \/ WorkerCrash(w)
        \/ CacheHit(w, r)

\* Fairness was retuned for the 2026-06-10 WorkerCrash addition. Crash
\* itself stays unconstrained (any worker may die at any point,
\* arbitrarily often), but liveness needs two progress assumptions
\* that were implicit before crashes existed:
\*   - WF on Materialize / RegisterMaterialization — a worker that
\*     survives with a claim eventually finishes the work and commits
\*     it (otherwise a permanently-claimed rule blocks every peer's
\*     CacheHit and TLC exhibits a lasso);
\*   - SF (not WF) on CacheHit — a crash-looping peer flips its claim
\*     between NoClaim and a rule, so CacheHit is enabled only
\*     intermittently; strong fairness states the intent: a worker
\*     that infinitely often has the chance to observe the registered
\*     rule eventually does.
Fairness ==
    /\ \A w \in Workers, r \in Rules : WF_vars(Materialize(w, r))
    /\ \A w \in Workers, r \in Rules : WF_vars(RegisterMaterialization(w, r))
    /\ \A w \in Workers, r \in Rules : SF_vars(CacheHit(w, r))

Spec == Init /\ [][Next]_vars /\ WF_vars(Next) /\ Fairness

----------------------------------------------------------------------------

\* === Safety invariants ===

\* OX-1 — CacheKeyDeterminism.
\* Every derivation of a rule's cache key — including derivations whose
\* worker crashed before registering — produced the same key. Under
\* KeyVariants = {1} this is the determinism axiom made checkable
\* against all crash/re-claim interleavings; under KeyVariants = {1,2}
\* TLC refutes it (CacheConsistencyNondetKey.cfg), which is the
\* model's witness that the invariant has actual content.
CacheKeyDeterminism ==
    \A r \in Rules :
        Cardinality({n \in KeyVariants : <<r, n>> \in computed}) <= 1

\* OX-6 — RegisterPrecedesRead.
\* No worker observes a cache hit for a rule that has not yet been
\* committed to the register log. The implication is monotone: once
\* a rule is in the log, hits are permitted; before that, they are not.
RegisterPrecedesRead ==
    \A pair \in cached_seen : pair[2] \in RegisteredRules

\* Companion safety: cooperative claim is preserved across interleavings.
\* This is not OX-1/OX-6 itself but a load-bearing premise of both: it is
\* the discipline that keeps the register log linear (no double-register).
NoDoubleRegister ==
    \A i, j \in DOMAIN register_log :
        (i # j) => register_log[i][1] # register_log[j][1]

\* === Liveness ===

\* EventualSkip: under fairness, every worker eventually observes a
\* cache hit for every registered rule.
EventualSkip ==
    \A w \in Workers, r \in Rules :
        (r \in RegisteredRules) ~> (<<w, r>> \in cached_seen)

============================================================================
