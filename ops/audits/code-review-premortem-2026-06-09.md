# Synthèse — Revue de code (5 lots) × Premortem pré-publication

**Date** : 2026-06-09 · **SHA cible des revues** : `63b1fe9b6576f2096d00b79966487b23b1743fc7`
**Sources** :

| Source | Molécule | Périmètre | Verdict du lot |
|---|---|---|---|
| R1 | task-20260609-7653 | Cœur (ox-core + sites transverses cache/clé) | **NO-GO** en l'état |
| R2 | task-20260609-e56c | Concurrence & état + conformance TLA+ (ox-state, ox-cache, ox-lock, spec/tla) | **NO-GO** en l'état |
| R3 | task-20260609-1f3a | Exécuteurs (local, Ray, SLURM) | GO-WITH-FIXES |
| R4 | task-20260609-710c | Surface publique & parsing (ox-format, ox-cli, ox-translate, ox-api, ox-mcp) | GO-WITH-FIXES |
| R5 | task-20260609-f967 | Périphérie & reproductibilité des claims (dashboards, metrics, benchmarks) | **NO-GO** en l'état |
| PM | security premortem | Premortem 5 personas (kahneman, adversary, torvalds, feynman, godin) | 4 bloquants + top-10 modes d'échec |

*Note d'inventaire : le dossier du premortem contient `synthesis.md` + 5 réponses persona ; il n'existe pas d'`outcomes.md` (mentionné dans le brief mais jamais produit). La synthèse couvre les 5 récits.*

---

## 1. Résumé exécutif

Le moteur est sérieux — mais les trois promesses mises en vitrine (la clé de cache qui voit tout changer, la sûreté multi-session vérifiée par TLA+, le benchmark rejouable) ont **chacune** un trou démontrable en dix lignes par un lecteur hostile. Ne pas publier à ce SHA. La bonne nouvelle : la majorité des bloquants se corrige en heures ou en jours (du texte de papier, des gardes d'une ligne, un format de clé) — et c'est la **dernière fenêtre** pour changer la clé de cache sans casser les caches des futurs utilisateurs, le papier le dit lui-même. Le premortem a vu quatre pièges que les cinq relectures de code ont ratés (le fixture du README qui plante au premier `ox run`, le contre-exemple de cache livré armé dans le propre benchmark) ; les relectures ont vu le trou que le premortem a raté (le protocole multi-session « vérifié » n'est branché nulle part). Après le lot bloquant ci-dessous : publication défendable.

## 2. Verdict global

**GO-WITH-FIXES — ce qui signifie : NO-GO au SHA `63b1fe9`, GO après le lot bloquant.**

Trois lots sur cinq disent NO-GO en l'état, et leurs raisons ne se recouvrent pas : R1 (la clé de cache, artefact central du papier, est cassée de quatre façons), R2 (le claim multi-session est réfutable en deux terminaux), R5 (le benchmark que le README fait tourner contredit le chiffre du papier). Aucun de ces trois n'est un problème de fond insoluble — chacun a un fix borné (code ou texte) — d'où GO-WITH-FIXES et non NO-GO structurel. Mais le lot bloquant n'est pas cosmétique : il faut le traiter **avant** le flip public, pas en 0.1.1.

### Liste des bloquants (détail en §3 et §4)

**Bloquants code — la clé de cache (fenêtre unique : tout changement post-release invalide les caches)**
1. Clé non-injective (framing, slots Option, binding chemin↔contenu) — B1
2. Contenu des scripts hors clé (seul le chemin est hashé) — B2
3. `shell_executable` et contenu d'environnement hors clé — H5/H6 (escaladés : même fenêtre que B1/B2)

**Bloquants code — intégrité des artefacts et de l'exécution**
4. Course cancel/completion : résultat d'un job annulé enregistré au cache — B4
5. Collision silencieuse de JobId : un job jamais exécuté, run « succès » — B5
6. Temp file déterministe partagé entre sessions + aucun fsync — B6
7. SLURM : job TIMEOUT/OOM classé succès → cache empoisonné — B8
8. `ox run -j 0` pend pour toujours (flag documenté stable) — H27
9. Panic UTF-8 du traducteur Snakemake sur Snakefile réel non-ASCII — H30

**Bloquants claim/papier (fix par le texte possible en une soirée chacun)**
10. ProducerIndex O(R+P) décrit dans le papier, inexistant dans le code — B3
11. Le claim coopératif multi-session ne gate pas l'exécution ; heartbeat/lifecycle morts ; chaîne cancel inexistante — B7 (réécriture honnête du claim OU câblage)
12. `EvictPrecedesUnregister` attribué à une spec qui ne le définit pas ; comptes TLC non reproductibles ; 2 invariants vacueux — B9/H21/H22
13. « Content-Addressable by Default » alors que le défaut est mtime sans hash — B10 (décision atomique : changer le défaut OU renommer + avertir dans SECURITY.md)
14. Provenance des benchmarks : deux suites contradictoires, le README pointe la mauvaise, lien « immutable record » mort — B11

**Bloquants premortem (aucun lot de review ne les avait vus)**
15. `ox run` plante sur le fixture maison du repo (bloc `run:` sans préambule input/output/params) — PM#1
16. `bench_lib.py` lu mais non déclaré dans le propre benchmark : contre-exemple « faux cache-hit » livré armé — PM#2
17. Le benchmark tourne en mode mtime : le content-addressing (contribution n°1) n'est mesuré nulle part — PM#3
18. Claim « cross-machine portable » contredit par OS+ARCH dans la clé (`key.rs:48-51`) ; jamais testé hors M4 Max — PM#6

---

## 3. Tableau consolidé — CRITICAL (dédupliqué entre lots)

Effort : S ≤ ½ jour · M = 1–3 jours · L > 3 jours.

| ID | Source | Fichier:ligne | Risque public | Fix | Effort |
|---|---|---|---|---|---|
| B1 | R1+R2 | ox-cache/src/key.rs:31-53 ; ox-cli/src/commands/run.rs:290-323 ; ox-lock/src/writer.rs:52-57 | Clé de cache non-injective de 3 façons constructibles (pas de framing rule/inputs, slots `Option` params/env confondus, hashes triés sans binding chemin↔contenu : échanger le contenu de 2 inputs → même clé → hit périmé). Repro 10 lignes ; démolit « content-addressable » le jour du lancement. Même motif de concaténation dans ox-lock. | Domain-separation (length-prefix ou tag par champ) ; hasher les paires (path, hash) triées par path ; tag de présence sur params/env. Invalide les caches existants → **maintenant ou jamais**. | M |
| B2 | R1 | ox-cli/src/commands/run.rs:213-217 | Le contenu des scripts n'entre pas dans la clé — seul le *chemin* est hashé. Éditer `script.py`, relancer → résultat d'avant servi du cache. Falsifie « the source of truth is file content » (papier §4.1, 1ʳᵉ phrase). | Hasher le contenu du script comme input implicite (le mécanisme `param_files` existe déjà) ; documenter l'exclusion pour le mode `Call`. | S–M |
| B3 | R1 | ox-core/src/resolver.rs:190-236 + 5 occurrences dans le .tex | Le papier décrit un « ProducerIndex (hash-map from output-pattern prefix) … amortized O(R+P) ». Le code : scan linéaire `Vec` → toujours O(R·P), seule la constante a baissé. Falsifiable par inspection en 30 s ; un benchmarkeur qui scale R le mesure. | Soit implémenter le vrai index par préfixe, soit **réécrire le paragraphe** (« precompiled-regex index ; asymptotic complexity remains O(R·P) ») et requalifier la borne aux 5 endroits. | S (texte) / L (code) |
| B4 | R1 | ox-core/src/scheduler.rs:1877 + 824-827 + 2196-2202 | Course cancel/completion : un job marqué Cancelled qui finit avant le kill est réécrit Succeeded sans check du statut, et son output — dérivé d'un upstream *failed* — est enregistré au cache et resservi. Contredit la narrative CancelPropagation (TLA+) : la spec modélise la course, le code la perd. | Dans `handle_completion`, relire le statut sous lock et court-circuiter si Cancelled (ni Succeeded, ni record, ni promote). | M |
| B5 | R1 | ox-core/src/job_graph.rs:216-217 + resolver.rs:1252-1258 | JobId minté par join `-` des valeurs de wildcards (`{sample:"A-1",lane:"2"}` ≡ `{sample:"A",lane:"1-2"}`) + insert sans détection de doublon → un job devient inatteignable, jamais exécuté, run « succès ». | Minting injectif (hash de la map structurée) + erreur `DuplicateJobId` à la construction (défense en profondeur — le check existe déjà pour les outputs). | S |
| B6 | R1 | ox-core/src/disk_writer.rs:192-236 | Deux défauts d'écriture : (a) temp file déterministe `target+".oxytmp"` partagé entre sessions → deux `ox run` concurrents (le scénario *vanté* du papier) interleavent et produisent un fichier chimère ; (b) aucun fsync avant rename → après crash, fichier tronqué au chemin final, consommé comme output valide. | (a) temp name unique par tentative (`NamedTempFile` ou pid+nonce) ; (b) `sync_all()` + fsync du parent — ou adoucir la promesse de l'en-tête du module. | S |
| B7 | R2 | ox-cli/src/commands/run.rs:1503-1531 ; ox-state/src/session.rs:110-172 ; scheduler.rs (zéro réf à StateDb) | **Le claim différenciant du papier ne gouverne pas l'exécution** : `claim_job` appelé en write-behind, résultat jeté (`let _ =`) ; `heartbeat`/`complete_session` jamais appelés (toute session paraît stale après 300 s) ; le reclaim ne re-vérifie pas la staleness en SQL contrairement à ce que la spec affirme refléter. Deux terminaux, deux `ox run` → chaque job exécuté deux fois. Réfutation du claim central en 30 s. | Court terme (recommandé pour la release) : **réécrire le claim** au conditionnel architectural honnête (« the state layer implements and tests the claim protocol; wiring it as the execution gate is staged work ») + corriger le tableau décisionnel (« Attach (wait) » ne correspond à rien). Moyen terme : claim comme garde de dispatch + tâche heartbeat + complete/interrupt session (~30 lignes). | S (texte) / M (câblage) |
| B8 | R2+R4 | ox-cli/src/commands/cancel.rs:245-261 ; run.rs:1394-1396 | La chaîne cancel modélisée par CancelPropagation.tla n'existe pas : `ox cancel` flippe le statut en DB puis SIGTERM au PID de la *première* session active (`find_job_pid` ignore son paramètre job_id) — et `ox run` ne gère que SIGINT : le SIGTERM tue le process entier, orphelinant les sous-process qui continuent d'écrire. Avec les sessions jamais complétées + PID recyclé : peut tuer un process tiers. | Handler SIGTERM dans `ox run` (même chemin graceful que SIGINT) + résoudre la session *propriétaire* du job (la table jobs la porte). | S–M |
| B9 | R2 | docs/paper/oxymake-paper.tex:792,1110-1113 vs spec/tla/ | Le papier attribue l'invariant `EvictPrecedesUnregister` à CancelPropagation.tla **qui ne le définit pas** (mention en commentaire ; la spec qui le possède est « bd-tracked », non commitée). Falsifiable par grep par n'importe quel reviewer. | Corriger le tableau du papier (INV-3c → « pending, EvictionRace.tla ») ou commiter la spec. | S |
| B10 | R1+R2+PM#4 | ox-cache/src/strategy.rs:16 ; lookup.rs:398-428 ; SECURITY.md:29 ; papier §4.1 | Le titre du papier est « Content-Addressable **by Default** » mais le défaut est `Mtime` *stateless* : aucun hash calculé, aucune consultation de cache.db. Une corruption taille-identique + mtime postérieur passe (cache poisoning exploitable sur cache/output partagé — adversary le classe bloquant-sécu). Contredit SECURITY.md. | Décision atomique, pas de 3ᵉ voie silencieuse : défaut → `MtimeHash`, OU garder mtime + renommer la section + avertissement explicite SECURITY.md (« sur cache partagé, utilisez `--cache-validation hash` »). | S (décision+doc) |
| B11 | R5+PM (feynman F3, kahneman) | benchmark/perf/results.md:1-63 ; benchmark/README.md:20-24 ; Justfile:11 | Deux suites de benchmark aux chiffres contradictoires (62×@10k Snakemake 9.21/M2 Max vs 29.8×@10k Snakemake 7.32.4/M4 Max du papier) ; le README public route `just benchmark` vers la suite **supersédée**, dont le results.md prétend faussement être « the immutable record the paper §6.1 cites » et pointe un fichier **absent du repo**. Changement de version Snakemake entre les deux jamais justifié → lu comme cherry-picking. | Une suite, un chiffre : pointer README+Justfile sur `bench/snakemake-vs-oxymake/` (ou faire déléguer `perf/`) ; supprimer la fausse provenance et les liens morts ; journaliser le choix de version dans RESULTS.md. | S |
| B12 | R3 | ox-exec-slurm/src/executor.rs:544-572 ; slurm_cli.rs:260-265 | SLURM : `execute()` classe le succès sur le seul exit code en **ignorant l'état terminal**. TIMEOUT (`0:15`), OOM (`0:9`), PREEMPTED (`0:0`) → exit_code 0 → succès. Pas d'écriture atomique côté SLURM → la sortie partielle d'un job tué est **cachée comme artefact valide**. | Mapper l'état terminal AVANT le code de sortie : tout état ≠ COMPLETED ⇒ échec (`slurm_state_to_job_status` existe déjà, il n'est juste pas appelé sur ce chemin). | S–M |

## 4. Bloquants premortem sans finding de code associé

Ces quatre modes d'échec sont jugés bloquants par le panel premortem et **n'apparaissent dans aucun des 5 lots de review** (voir §6, discipline anti-groupthink) :

| ID | Persona | Localisation | Risque public | Fix | Effort |
|---|---|---|---|---|---|
| PM#1 | torvalds | ox-exec-local/src/executor.rs:121-124 + fixture du repo | `ox translate` réussit, `ox run --dry-run` réussit, puis le **premier vrai `ox run` plante sur le fixture maison** : le bloc `run:` Python est lancé `python3 -c` sans préambule `input/output/params/wildcards` → `TypeError`. La promesse README « tu ne changes rien » est fausse, démontrée dans le terminal de l'utilisateur. | Injecter un préambule Snakemake-compatible dans tout bloc `run:` ; à défaut retirer la rule `run:` du fixture + documenter que `run:` exige une réécriture manuelle. | M (préambule) / S (fixture+doc) |
| PM#2 | feynman | bench/snakemake-vs-oxymake/generate.py | Le job du benchmark lit `bench_lib.py`, **non déclaré comme input** → l'éditer ne ré-exécute rien. Contre-exemple « faux cache-hit » en 5 lignes, livré armé dans le propre benchmark du papier. « Phantom re-runs disappear » n'est vrai que pour les inputs déclarés (contrat de Make 1976, pas la garantie sandboxée de Nix). | Déclarer `bench_lib.py` comme input ; borner le claim de l'abstract (*« for declared inputs »*) ; § « undeclared-input hazard » dans le papier. | S |
| PM#3 | feynman | bench/.../run.sh (aucun `--cache-validation`) | Le benchmark tourne en mode mtime : le 7.58× warm est un win Rust-vs-Python — **le content-addressing, contribution n°1 du papier, n'est mesuré nulle part**. | Note §6.2 disant quel mode est mesuré + ajouter la mesure qui défend la thèse : coût hash vs mtime, et le scénario `git checkout` chiffré (K jobs ré-exécutés par Snakemake, 0 par OxyMake). | M |
| PM#4 | kahneman | ox-cache/src/key.rs:48-51 ; papier l.1509/1521 ; README | Le claim « cross-machine portable » est contredit par sa propre source : OS+ARCH sont *bakés* dans la clé → un cache S3 partagé Mac↔Linux ne hit jamais. Jamais testé : tout a été mesuré sur un seul M4 Max (la CI verte teste ubuntu-only — jamais la plateforme du papier). | Affaiblir le texte (*« portable across machines of the same platform; heterogeneous OS/arch is future work »*) OU `platform` optionnel via flag. + **Relancer le bench une fois sur Linux/x86_64** (désamorce aussi PM#3 et la mono-plateforme). | S (texte) + S (re-mesure) |

## 5. Tableau consolidé — HIGH (dédupliqué)

| ID | Source | Fichier:ligne | Risque public | Fix | Effort |
|---|---|---|---|---|---|
| H1 | R1 | ox-core/src/wildcard.rs:310-323, 66-78 | Contrainte regex insérée comme groupe capturant : une contrainte utilisateur `(foo\|bar)` décale l'indexation positionnelle → wildcards suivants reçoivent silencieusement la mauvaise valeur. | Groupes non-capturants `(?:…)` ou groupes nommés. | S |
| H2 | R1 | ox-core/src/wildcard.rs:105-209, 300-335 | Wildcard répété (`{x}/{x}.txt`) : aucune égalité exigée entre occurrences → mauvais producer accepté, outputs ≠ target. | Backreference ou check post-match (comportement Snakemake). | S–M |
| H3 | R1 | ox-core/src/resolver.rs:479-482 | Récursion littérale sans borne → chaîne de dépendances profonde = stack overflow/SIGSEGV au lieu d'une erreur. | Worklist explicite ou limite de profondeur avec erreur propre. | M |
| H4 | R1 | ox-cli/src/commands/run.rs:301-305 ; model.rs:1133-1161 | `env_hash` hashe la spec littérale (chemin requirements, tag Docker mutable) — le papier promet « uv.lock hash, Docker image digest ». Modifier requirements.txt → même clé → cache périmé. | Hasher le contenu du fichier d'env ; résoudre les tags en digest, ou documenter la divergence. | M |
| H5 | R1 | ox-cli/src/commands/run.rs:254-330 ; ox-exec-local/executor.rs:739-742 | `shell_executable` utilisé à l'exécution mais absent de la clé de cache. Changer bash→zsh → output d'avant servi. **Même fenêtre que B1/B2** (toute dimension manquante doit entrer dans la clé avant la release). | Inclure dans le job_spec_hash. | S |
| H6 | R1 | ox-core/src/scheduler.rs:1704-1793 | TOCTOU split-lock dans `find_ready_jobs` : un job annulé entre les deux locks peut être dispatché après Cancelled. | Un seul lock pour drain+flip ; re-valider Pending avant push sur le chemin gated. | S–M |
| H7 | R1 | ox-core/src/scheduler.rs:560-564 vs 1896-1911 | `is_cached` re-hashe des inputs que le disk_writer peut être en train d'écrire → hash torn → décision de cache non-déterministe (bug irreproductible en boucle). | Attendre l'ack du disk-writer pour les inputs du job, ou hasher depuis le memory_store. | M |
| H8 | R1 | ox-core/src/scheduler.rs:613, 1849, 843-851 | Budget de ressources + permit de sémaphore fuient définitivement sur `abort_all` → le run « tourne » à vide pour toujours : l'utilisateur voit un hang, pas une erreur. | Libération via guard `Drop` porté par la tâche, pas par le message de complétion. | M |
| H9 | R1 | ox-core/src/job_graph.rs:540-560, 462-486 | `mark_skipped` tombstone le nœud Job → la fermeture transitive upstream/downstream se coupe au job skipped. Latent dans `ox run`, **exporté public via ox-api**. La doc affirme le contraire. | Flag `skipped` sur le Job (pas de changement de type), ou traverser les tombstones ; a minima corriger la doc. | S–M |
| H10 | R1 | ox-core/src/job_graph.rs:293-294, 618-620 | `unreachable!()` si un index pointe sur un nœud non-Job — état que `mark_skipped` sait produire. Panic au lieu d'erreur. | Retourner Option/Result comme dag.rs le fait déjà. | S |
| H11 | R1 | ox-core/src/model.rs:1046-1057 | `ResourceValue` untagged : `gpu = 1.0` re-désérialise en `Int(1)` → le spec-hash change à travers un cycle save/load → pipeline déjà caché repart de zéro. | Tagger l'enum ou normaliser ; property-test de round-trip. | S |
| H12 | R1 | ox-core/src/model.rs:1450-1485 | `ContentHash`/`ComputationHash` : tuple `pub String` sans validation — valeurs forgées/tronquées/uppercase acceptées et comparées comme strings (ox-lock décide la validité du cache dessus). Gel de représentation à la 0.1.0. | Champ privé + `from_hex` validant (le modèle existe : `ArtifactMeta::from_hex`). Dernière fenêtre bon marché. | S |
| H13 | R1 | ox-core/src/disk_writer.rs:192-236 | `target` écrit verbatim : `create_dir_all` + rename sans confinement workspace ni check symlink. Un Oxymakefile partagé (input non-fiable) écrit hors sandbox via `../../…`. | Canonicaliser/confiner au root du workspace avant écriture. | M |
| H14 | R1 | ox-core/src/disk_writer.rs:167-186 | Échec de persistance = `eprintln!` + compteur que rien ne consulte → disque plein = tous les outputs perdus, run rapporté succès. | Échouer (ou marquer dégradé) le run si `writes_failed > 0` au flush final. | S |
| H15 | R2+PM (torvalds) | ox-state/src/db.rs:417-434 ; ox-cache/src/lookup.rs:99-103 | Aucun `busy_timeout` sur state.db ni cache.db → sous écrivains concurrents (scénario nominal du papier), le perdant reçoit SQLITE_BUSY : avalé en silence ou run avorté. | `PRAGMA busy_timeout=5000` à l'open des deux DB (2 lignes). | S |
| H16 | R2 | ox-state/src/db.rs:500-545 | `complete_job`/`fail_job` filtrent sur `status='running'` mais pas sur `session_id` → une session zombie peut terminaliser un job re-claimé par une autre, avec SES hashes. | `AND session_id=?` dans le WHERE. | S |
| H17 | R2 | ox-cli/src/commands/clean.rs:255-258 | `ox clean` supprime jobs+sessions sans vérifier qu'aucune session n'est vivante → l'état d'un run en vol disparaît, son audit-trail lira une table vide. | Refuser (ou `--force`) si sessions actives non-stale. | S |
| H18 | R2+PM (feynman F5) | spec/tla/CooperativeClaim.tla:121-124 ; CacheConsistency.tla:23 | Deux invariants model-checkés **invérifiables par construction** : `NoDoubleRunning` trivialement vrai par typage, `CacheKeyDeterminism` tautologique (`ContentKey(r)==r`). « Model-checked, but the model can't express the bug ». | Variable de croyance locale par session ; modéliser une clé non-déterministe possible. | M |
| H19 | R2 | papier:2085-2108 ; spec/tla/* ; CI | Les comptes d'états TLC publiés (5 589/6 222/7 429) ne sont reproductibles depuis aucun artefact du repo (pas de script TLC, pas de job CI, ledgers bootstrap-only). | Commiter `spec/tla/run-tlc.sh` + sortie archivée ; faire pointer les chiffres du papier dessus. | S–M |
| H20 | R2 | ox-lock/src/writer.rs:79-81 | `ox.lock` écrit par `fs::write` direct : crash mid-write = lockfile de *reproductibilité* corrompu. | tmp + rename (l'axiome AtomicRename existe et n'est pas utilisé ici). | S |
| H21 | R2+PM (adversary B) | ox-cache-remote/src/directory.rs:56-94 | `store` non-atomique avec early-return `if exists` → un crash mid-copy laisse un artefact tronqué **jamais réparé**, servi à tous (cache d'équipe NFS empoisonné) ; `fetch` ne re-hashe pas le contenu reçu. Latent (code débranché), armé dès le wiring object_store. | store : tmp+rename ; fetch : `hash_file(dest)==key` (c'est le point entier du content-addressing). | S |
| H22 | R3 | ox-exec-local/src/worker_pool.rs:197-204 ; call_mode.rs:531-623 | Pool warm-fork : sur timeout/cancel on SIGKILL le template seulement — le petit-enfant fork qui exécute la fonction **continue d'écrire dans `results/`**. | Process-group au `ensure_warm` + `killpg` (le chemin froid le fait déjà correctement). | M |
| H23 | R3 | ox-exec-local/src/executor.rs:666-728, 952-985 | Jobs call-mode warm jamais insérés dans `self.running` → `cancel()` est un no-op silencieux. Ctrl-C laisse des jobs Python tourner. | Suivre les dispatches warm dans une map annulable ; router `cancel()` vers le pool. | M |
| H24 | R3 | ox-exec-ray/src/executor.rs:577-583, 710-729 | Après `submit_dag`, le suivi est keyé par run_id mais `cancel(job_id)` cherche job_id → no-op ; le driver Ray et toutes ses tâches continuent. | Indexer les job_id vers la submission du driver ; cascader `ray.cancel`. | S–M |
| H25 | R3 | ox-exec-slurm/src/job_script.rs:35, 193 | Slice byte `&job_name[..255]` → panic sur nom de règle/wildcard non-ASCII (input utilisateur d'un repo public). | Tronquer sur frontière de caractère. | S |
| H26 | R3 | local call_mode.rs:304-333 ; ray call_mode.rs:346-374 | `build_call_args` : input nommé avant input anonyme → `f(name=a, b)` → SyntaxError Python. La feature phare (call-mode) casse sur un mix banal. | Positionnels d'abord, puis keywords. | S |
| H27 | R4 | ox-cli/src/commands/run.rs:73-74 | `ox run -j 0` accepté → le sémaphore ne délivre jamais de permit → **pend pour toujours** (reproduit). Flag documenté stable. | `value_parser` borné `range(1..)` (1 ligne). | S |
| H28 | R4 | STATUS.md:47,57,73,170 vs run.rs:84-86 | `--report-json` documenté **stable** en 4 endroits, avec invocation littérale — le flag n'existe pas (reproduit : erreur clap). La première commande copiée-collée du contrat de stabilité échoue. | Ajouter le flag ou réécrire STATUS.md. | S |
| H29 | R4 | ox-format/src/parse.rs:393-397 + STATUS.md §2 | `include` déclaré stable, collecté par le parseur, **développé par aucun chemin d'exécution** (CLI, API, MCP) — silencieusement inerte. | Implémenter l'expansion (les erreurs existent déjà) ou retirer du contrat stable + rejeter avec message. | S (doc) / M (impl) |
| H30 | R4 | ox-translate/src/snakemake/parser.rs:1054 | `truncate` slice par octets → panic sur toute ligne top-level non-ASCII > 60 octets (reproduit). Exactement la classe d'input pour laquelle le papier promet des « structured warnings ». Candidat CRITICAL du lot. | `char_indices()` boundary (1 ligne). | S |
| H31 | R4 | ox-translate/src/wdl/parser.rs:635, 233 | Brace-matching compte les `{}` du corps de `command` : `mv f.{a,b}`, `${VAR` → sections avalées ou tâche entière sautée **sans diagnostic**. Mis-translation silencieuse sur du WDL réel. | Traiter le corps de command comme span opaque ; diagnostic si profondeur jamais refermée. | M |
| H32 | R4 | ox-translate/src/wdl/parser.rs:457-470 vs papier:1584-1587 | Les inputs WDL `Int`/`Boolean`/`Float` deviennent des params dont la *valeur* est `"# WDL type: Int"` — ni escalation ni diagnostic, alors que le papier cite `Int` dans la phrase sur les « structured escalations ». | Vraie `Escalation` pour tout type non mappé. | S |
| H33 | R4 | ox-translate/src/wdl/parser.rs:40-49, 368-396 | scatter WDL → `expand="zip"`, Snakemake `expand()` → `"product"` : la cardinalité n'est pas préservée en round-trip ; le test ne vérifie que l'étiquette. | Représenter la cardinalité ; escalation pour scatter imbriqué ; test sémantique. | M |
| H34 | R4 | ox-api/builder.rs:239-261 ; ox-mcp/tools.rs:691-721 ; ox-cli/common.rs:82-200 | **Trois copies divergentes** de la résolution de cibles : seul le CLI a le fix `{config.X}` (ox-7a98) ; le MCP n'a ni substitution ni expansion → le même Oxymakefile marche en CLI et échoue via l'API publique et la démo MCP. | Une seule implémentation partagée (ox-core ou ox-format). | M |
| H35 | R4 | ox-mcp/src/tools.rs:586-613 | `ox_clean` MCP ignore son paramètre `what` et nettoie un chemin de cache **legacy** (`cache.json`, créé nulle part) — l'opération destructive du serveur ne fait jamais ce qu'elle annonce et répond succès. | Honorer `what` ; déléguer au vrai clean ; supprimer le chemin mort. | S |
| H36 | R5 | benchmark/perf/run.sh:60-68, 162-164 | Le reproducer ne pinne pas la version Snakemake — il benchmarke ce qui est dans le PATH. Le dénominateur du headline varie de 2.2× entre 7.32.4 et 9.21.0. | Lock à la version du papier ; échouer bruyamment si elle diffère. | S |
| H37 | R5 | benchmark/perf/run.sh:38-40, 116-135 | Dérive de protocole : tailles par défaut ≠ papier (omet 100, ajoute 50000) ; fallback sans hyperfine = zéro warmup + 2 spawns Python (~20-40 ms) autour de mesures de 4-64 ms — l'overhead du chronomètre dépasse le signal. | Aligner les tailles ; exiger hyperfine pour les phases sub-100 ms ; warmup du record. | S–M |
| H38 | R5 | ox-metrics/src/server.rs:51 | `/metrics` bindé sur `0.0.0.0` sans auth : sur un login node HPC partagé, tout pair scrape la structure du pipeline (noms de rules, sessions). ox-dashboard fait correctement 127.0.0.1. | Bind 127.0.0.1 par défaut ; `0.0.0.0` opt-in avec warning. | S |
| H39 | R5 | README.md:369 vs crates/ox-env-* | Le README liste « Environments: system, uv, conda, docker, nix, apptainer » en feature livrée — **zéro impl `EnvironmentProvider` n'existe** (stubs 1 ligne ; 4 crates inexistants). Le papier est honnête, le README non. | Qualifier le bullet (« planned/stub ») ou le déplacer en Known limitations. | S |

## 6. Croisement findings × premortem — le cœur de l'analyse

Le top-10 du premortem décrit *l'entonnoir* (réception → première impression → examen approfondi) ; les findings de code sont *les munitions* de chaque étage. Voici quel finding matérialise quel mode d'échec — et où chaque exercice a vu ce que l'autre a raté.

### Mode par mode

| Mode d'échec premortem (top-10) | Findings de code qui le matérialisent | État |
|---|---|---|
| **#1 `ox run` plante sur le fixture** (torvalds, H×H) | Aucun finding de lot. Adjacent : H26 (SyntaxError call-mode sur mix d'inputs banal), H27 (`-j 0` pend) — la même famille « la 2ᵉ commande casse ». | **Premortem-only.** R3 a lu `executor.rs` intégralement sans tester le parcours README. |
| **#2 Faux cache-hit sur input non déclaré** (feynman, H×H) | B2 est *le même mécanisme* dans le code (contenu de script non hashé = dépendance non déclarée par construction) ; H4/H5 (env, shell hors clé) sont la même classe. PM#2 l'arme dans le propre benchmark du repo. | Convergence forte : 1 mode d'échec, 4 surfaces. À traiter comme un seul chantier « complétude de la clé ». |
| **#3 Le benchmark mesure mtime, pas hash** (feynman, H×H) | B10 (défaut mtime vs titre du papier, vu par R1 *et* R2) donne la base ; H36/H37 (version non pinnée, protocole dérivé) aggravent. Mais **aucun lot n'a remarqué que le bench de record lui-même tourne en mode mtime**. | Premortem complète les lots : R5 a audité la *provenance* du benchmark, pas sa *validité de mesure du claim*. |
| **#4 Cache poisoning** (adversary, M×H, bloquant-sécu) | Le mode le plus densément matérialisé : B10 (mtime ne hashe pas), B12 (SLURM cache la sortie d'un job tué), B4 (record après cancel), B1/B5 (collisions de clé/ID), B6 (fichier chimère/tronqué), H21 (fetch distant sans rehash), H14 (writes perdus en silence). | Convergence totale reviews↔premortem. C'est LE sujet du teardown s'il arrive. |
| **#5 Badge CI vire au rouge tout seul** (kahneman, H×M) | Aucun finding de code — c'est `drift-tripwire.yml` (deadline 2026-09-01 + staleness 60 j sur mainteneur solo). | **Premortem-only.** Décision atomique opérateur : repousser / advisory / assumer. |
| **#6 « Cross-machine portable » réfuté** (kahneman, M×H) | R1 a lu `key.rs` intégralement et a trouvé l'injectivité (B1) **sans voir la contradiction de portabilité** (OS+ARCH dans la clé vs claim du papier). R2 a listé os/arch dans la concaténation sans le croiser au claim. | **Premortem-only** sur le croisement claim↔code, alors que deux lots avaient le fichier sous les yeux. Leçon : les reviewers cherchaient des bugs, pas des contradictions de promesse. |
| **#7 Maintenance solo s'effondre** (kahneman+torvalds, H×M) | Aucun finding de code (politique de maintenance, README). H15 (busy_timeout) et l'absence de récupération de corruption state.db (torvalds) en sont les détonateurs techniques sur HPC. | Éditorial + 2 fixes S. |
| **#8 Teardown HN sur « agent fleet »** (godin, M→H×H) | Aucun — purement éditorial (déplacer le paragraphe en docs/MAKING-OF.md post-launch). | Premortem-only. À séquencer avec #10. |
| **#9 Premier Snakefile communautaire refusé** (torvalds, H×M) | H30 (panic UTF-8 — pire que refusé : crash), H31-H33 (WDL avalé/mal traduit sans diagnostic), R4-MEDIUMs (split sur virgule dans quotes, ports `sample`/`zip` sautés, YAML imbriqué perdu). Les lots fournissent le détail que le premortem prédisait. | Convergence. Le fix UX premortem (rapport de couverture en tête de `ox translate`) + les fixes code R4. |
| **#10 Silence total** (godin, H×M→H) | Aucun finding de code. Séquencement : tribu → repo → Show HN → arXiv découplé ; binaire téléchargeable. | Premortem-only, hors périmètre code. |

### Ce que les reviews ont vu et que le premortem a raté

Symétrie oblige : **le trou le plus grave du repo — le protocole multi-session qui ne gate rien (B7/B8) — n'apparaît dans aucun des 5 récits premortem.** Adversary et feynman ont attaqué la *qualité* des specs TLA+ (tautologies, bornes — H18, confirmé par R2) mais aucun persona n'a découvert que `claim_job` est appelé en write-behind avec résultat jeté, que heartbeat n'est jamais appelé, et que deux `ox run` simultanés exécutent chaque job deux fois. C'est R2 qui l'a trouvé, par lecture de câblage. De même B5 (collision JobId), B12 (SLURM exit-code), H34 (triple copie divergente) sont des découvertes pures de review.

### Verdict anti-groupthink

Les deux exercices ne se sont **pas** lissés l'un l'autre, et c'est leur valeur : 4 bloquants premortem invisibles aux 5 lots (PM#1-4), 1 bloquant de review invisible aux 5 personas (B7). La cause est structurelle : les lots ont cherché *des bugs dans le code*, le premortem a cherché *des écarts entre l'expérience vécue et la promesse* — et personne, dans les deux exercices, n'a simplement **déroulé le quickstart du README sur une machine vierge**. C'est le test manquant ; il aurait trouvé PM#1 (fixture qui plante), H27 (`-j 0`), H28 (`--report-json`), et la moitié de B11 (benchmark) en une heure. À ajouter au plan d'action comme gate de release.

## 7. Annexe — MEDIUM / LOW (compacte)

41 MEDIUM + 22 LOW au total. Aucun ne bloque seul ; les familles, avec les représentants saillants :

**Famille « fallback silencieux du parseur » (R4, ~10 MEDIUM)** — `timeout` invalide → aucun timeout ; `retry=-1` → 4 milliards de retries ; `error_strategy`/`backoff`/`expand` typo → défaut sans warning (`expand="Zip"` → produit cartésien 10⁶ au lieu de zip 10³) ; `[environment]` clé inconnue → aucun env ; `input` table sans `path` → pattern vide ; dédup quadratique sur CSV 500k (DoS local) + collision de clés config écrasées. *Le parseur sait rejeter proprement (lifecycle/materialize le font) — appliquer le même pattern partout.*

**Famille « erreurs avalées » (R1+R2, ~12 MEDIUM)** — `invalidate()` répond succès même si le DELETE a échoué ; EventSink : échec `create_session` → `sid=""` → **toutes** les écritures d'état échouent en silence (FK) ; migration : erreur de lecture de version traitée comme « DB fraîche » ; lockfile : fichier d'env illisible → `spec_hash=None` sans warning ; lectures memory-policy avalées → « missing input » sur le mauvais job ; NDJSON `--json` droppe des events sur broken pipe sans sentinelle (R5).

**Famille « concurrence/état secondaire » (R1+R2, ~8 MEDIUM)** — `register_jobs` ré-attribue le run_id d'autrui (stats dashboard faussées) ; `cancel_jobs` sur-rapporte ; sessions « sync » fuient à chaque `ox status` (cible potentielle du SIGTERM) ; `finalize_job_history` non idempotent (doublons d'audit) ; éviction mémoire sans ack de persistance ; outputs InMemory sans type_hint fusionnent en un nœud ; migration concurrente non sérialisée.

**Famille « exécuteurs » (R3, ~7 MEDIUM)** — pool warm sérialisé derrière un Mutex unique (zéro parallélisme call-mode, en tension avec le benchmark) ; timeout d'écriture stdin laisse un worker corrompu dans le pool ; `job.timeout` ignoré par les boucles de polling SLURM/Ray ; `cd {project_dir}` non quoté (Ray casse sur espace) ; injection Python possible via chemin de script dans le driver Ray ; call-mode SLURM génère du Python invalide ; pas d'`env_clear` (secrets du shell hérités par les jobs — un Oxymakefile public peut exfiltrer). *Les deux derniers méritent un œil sécurité avant le tag.*

**Famille « surface publique » (R4+R5, ~9 MEDIUM)** — validations no-op de ox-format (`check_output_wildcards` vide alors que la doc promet le check) ; `--set` sans `=` ignoré ; wildcard sans liste config → « nothing to build » sans message ; JSON de `test`/`gate` construit au `format!` (échappement incomplet) ; `ox dashboard --bind 0.0.0.0` sans warning ; path traversal lecture de logs MCP via job_id forgé ; `ox_plan` MCP hardcode `cached: 0` ; découverte de fichiers ox-api : profondeur 5 hardcodée + cache mtime racine stale ; XSS latent du champ `status` dashboard ; Justfile `((pass++))` sous `set -e` tue la suite compat au premier succès (R5).

**Famille « traducteur » (R4, ~6 MEDIUM)** — split non quote-aware (`sort -k1,2` coupé en deux) ; export Snakemake re-quote sans échapper `"""` (direction generate jamais testée) ; `message:` annoncé mappé mais jeté ; `wildcard_constraints` global droppé en Info au lieu d'Escalation ; ports nommés `sample`/`zip` silencieusement sautés ; YAML imbriqué perdu. + le claim papier « executed with identical output file trees » n'a **aucun test d'exécution** derrière lui (round-trips structurels seulement, zéro round-trip WDL).

**LOW (22)** — cosmétiques et latents : newtypes d'identité `pub String` à fermer avant 0.1.0 (fenêtre semver — celui-là vaut S maintenant) ; `debug_assert` sur poids d'edges compilé out en release ; doc d'en-tête db.rs fausse (version 2 vs 9) ; action TLA stutter « théâtre » ; README spec/tla « not yet committed » périmé ; underflow usize metrics sur state.db corrompu ; panic `--port` non-numérique ; etc.

## 8. Plan d'action recommandé

### A. Avant le flip public (bloquants — l'ordre suit l'entonnoir premortem)

1. **Quickstart sur machine vierge** comme gate de release (1 h, trouve/valide PM#1, H27, H28, B11).
2. **Clé de cache, chantier unique** : B1 + B2 + H4 + H5 + PM#4 (platform) — un seul changement de format, une seule invalidation, maintenant ou jamais. (M, ~2-3 j avec property-tests)
3. **Fixture & première impression** : PM#1 (préambule `run:` ou fixture+doc), H27 (`-j 0`), H28 (`--report-json`), H30 (panic UTF-8), H25 (panic SLURM UTF-8), H26 (SyntaxError call-mode). (Tous S, ~1 j cumulé)
4. **Intégrité d'exécution** : B4 (course cancel), B5 (JobId), B6 (temp+fsync), B8 (SIGTERM/PID), B12 (SLURM état), H22-H24 (orphelins warm + cancel Ray). (S-M chacun, ~3-4 j)
5. **Papier — réécritures honnêtes** (une soirée chacune) : B3 (ProducerIndex), B7 (claim multi-session au conditionnel architectural — ou câblage M si le temps le permet), B9 (EvictPrecedesUnregister), B10 (décision mtime + SECURITY.md), PM#2 (`bench_lib.py` + « for declared inputs »), PM#3 (note mode mesuré + table git-checkout), PM#4 (texte portabilité).
6. **Benchmarks** : B11 (une suite, un chiffre), H36 (pin Snakemake), + **une re-mesure Linux/x86_64** (désamorce mono-plateforme, le meilleur ratio risque/euro du premortem). (S + S + ½ j machine)
7. **Périphérie qui fuit** : H38 (metrics 127.0.0.1), H39 (README env backends), H15 (busy_timeout). (3×S)
8. **Décisions atomiques opérateur** (pas du code) : défaut mtime vs MtimeHash (B10) ; drift-tripwire (PM, repousser/advisory/assumer) ; paragraphe « agent fleet » → MAKING-OF.md ; séquencement tribu → repo → HN → arXiv.

### B. Avant le tag v0.1.0 (HIGH restants, publiables en issues honnêtes sinon)

- Wildcards : H1, H2 (corruption silencieuse de valeurs — R1 les juge quasi-bloquants).
- Scheduler : H6, H7, H8 (TOCTOU, torn read, fuite de budget→hang).
- Graphe/modèle : H3, H9-H12 (récursion, mark_skipped, unreachable, ResourceValue, ContentHash + newtypes `pub String` — fenêtre semver).
- État : H16, H17, H20, H21 (session_id, clean, ox.lock atomique, DirectoryCache).
- TLA+ : H18, H19 (invariants vacueux, runner TLC committé).
- Surface : H29 (include), H31-H35 (WDL, triple copie, ox_clean MCP), H13, H14, H37.

### C. Accepter consciemment (avec la phrase d'acceptation)

- **Les ~63 MEDIUM/LOW de l'annexe** : « Nous publions avec ces défauts connus, tracés en issues publiques dès le jour 1 ; aucun ne casse une promesse du papier ni ne corrompt de données sur le chemin nominal mono-session. »
- **Stubs env/storage/S3/GCS** : « Nous publions des crates stubs déclarés comme tels dans le papier et le README (après H39) ; `environment=uv` sans isolation réelle est documenté, pas silencieux. »
- **Cold-run plus lent que Snakemake** : « Nous publions le 0.80×/0.27×/0.69× tel quel — c'est déjà la posture du papier ("We own this result"), on ne la dilue pas. »
- **Bus-factor 1** : « Nous publions en mono-mainteneur best-effort, politique écrite dans le README avec contact sécurité ; la lenteur annoncée est un contrat tenu, le silence serait un abandon perçu. »
- **Model-checking borné N=2-4** : « Nous publions des specs bornées avec leurs bornes nommées et un encadré vérifié-vs-supposé ; après H18/H19, "formally-specified" décrit ce qui est commité, pas plus. »

---

*Rapport produit par task-20260609-310a (synthèse). Aucun fix de code dans ce commit — le rapport est le seul artefact. Les molécules sources restent la référence pour les trails de preuve complets.*
