# Synthèse finale GO/NO-GO — Mission de finalisation OxyMake

**Date** : 2026-06-10 · **Molécule** : task-20260610-5a5d (M8)
**Audit d'origine** : `ops/audits/code-review-premortem-2026-06-09.md` (SHA cible `63b1fe9`)
**Sources** : fixes.md de F1a (task-20260610-d970), F1b (2dac), F2 (d1c6), F3 (8f1b),
F4 (b5a7) ; rescope-report M1 (97bc) ; makingof-report M2 (aa22) ; doc-premortem M9
(1cdf) ; virgin-quickstart M7 (7ec6) ; findings eb4c (gates temporelles) ; outcomes
171c (README) ; outcomes des vagues antérieures bd89/cfeb/055a/b539/7680. Chaque
verdict ci-dessous a été **contre-vérifié dans git** (code et .tex aux branches
concernées), pas seulement lu dans les rapports.

---

## 1. Résumé exécutif

L'audit avait pointé 55 défauts graves ; 47 sont aujourd'hui fixés, chacun avec son
test écrit avant le fix et son commit nommé — le moteur tient. Mais la pièce
maîtresse, la clé de cache, garde ses quatre trous d'origine : ce chantier-là n'a
jamais été confié à personne, il est tombé entre les chaises de la planification.
Et le papier raconte encore deux choses que le code ne fait pas (un index « implémenté »
qui est en réalité un parcours linéaire, une coordination multi-sessions qui ne
gouverne aucune exécution) — une soirée de texte suffit à les rendre honnêtes.
Enfin, tout ce travail vit sur onze branches non fusionnées : le merge, la
reconstruction du PDF et du tarball arXiv sont encore devant nous.

## 2. Verdict

**PAS-PRÊT pour la séquence merge → flip → tag v0.1.0 → arXiv — mais la liste
restante est courte et bornée.**

Ce qui est prêt : les 5 chantiers de fixes (31 findings, TDD, gates DoD verts sur
chaque branche), le quickstart machine-vierge (verdict M7 : « PRÊT » sur le parcours
README), la doc CLI (M9 : « la doc tient seule »), les gates temporelles purgées
(eb4c), le repositionnement README (171c), le making-of honnête (M2).

### Bloquants restants (dans l'ordre de traitement)

| # | Bloquant | Nature | Effort |
|---|----------|--------|--------|
| R1 | **B2 — contenu des scripts hors clé de cache.** Éditer `script.py` → résultat d'avant servi du cache. Vérifié à HEAD intégration : `execution_source()` sérialise le bloc execution (donc le *chemin* du script), `mtime+hash` ne re-valide que les inputs déclarés — le script n'en est pas un. Falsifie la 1ʳᵉ phrase du papier §4.1. | code | S–M |
| R2 | **B1 + H4 + H5 — clé non-injective, env spec littérale, shell_executable hors clé.** `key.rs` identique au SHA de l'audit (vérifié par hash de fichier) : concaténation sans framing ni binding chemin↔contenu ; `env_hash` hashe la spec sérialisée (tag Docker mutable, chemin requirements) ; `shell_executable` absent. **Fenêtre unique** : tout changement post-tag invalide les caches utilisateurs. Fixer maintenant ou signer l'acceptation (§6). | code | M (chantier unique, un seul changement de format) |
| R3 | **B3 — le papier affirme « We implemented a ProducerIndex (a hash-map from output-pattern prefix…) amortized O(R+P) » (§sec:scale-study).** Le code (resolver.rs:190-240, HEAD intégration) est un `Vec` parcouru linéairement par cible — regex précompilées, O(R·P) à constante réduite. Ni 055a ni M1 ne l'ont réécrit. Falsifiable par inspection en 30 s. | texte papier | S (une soirée) |
| R4 | **B7 — le papier vend la coordination multi-sessions comme livrée** (§« Daemon-Free Cooperative Execution » : « Multiple concurrent ox run processes coordinate through optimistic-lock SQL transitions »). Vérifié à la branche F4 : `claim_job` toujours en write-behind résultat jeté (`let _ =`, run.rs:1504-1524), zéro appel `heartbeat`/`complete_session` dans run.rs. Deux terminaux = chaque job exécuté deux fois. F4 (H16/H17) a durci la *couche état*, pas le câblage. Réécrire le claim au conditionnel architectural honnête (option S de l'audit). | texte papier | S (une soirée) |
| R5 | **B9 — partiellement restant.** F4 a déclaré « EvictPrecedesUnregister retiré » mais n'a purgé que l'occurrence du tableau TLC en annexe. Restent, vérifiées sur la branche F4 elle-même : la ligne INV-3c (tex l.854) et « This cascade satisfies EvictPrecedesUnregister » (l.1256) — attribuées à CancelPropagation.tla qui ne le *mentionne* qu'en commentaire. | texte papier | S |
| R6 | **Intégration : 11 branches non fusionnées** (aucune n'est dans `feat/release-scaffolding-v0.1.0`). Conflit .tex garanti entre M1 (97bc) et F4 (b5a7) — les deux ont modifié `oxymake-paper.tex` depuis des bases différentes. Le tarball arXiv régénéré par M1 est **périmé** par rapport au tableau TLC de F4 : à régénérer après le merge. Signature `complete_job`/`fail_job` changée par F4 (trait StateBackend) — ripple possible. | mécanique git | ½ j |
| R7 | **F11 (M7) — référé contre référé** : `cs release-audit` gate C flagge 3 occurrences du contact mainteneur public que `forbid-strings.yml` exempte explicitement. Flipper avec son propre détecteur de dérive rouge en permanence institutionnalise la fatigue d'alarme. À réconcilier avant flip. | tooling | S |
| R8 | **F16 (M7) — tripwire calendaire armée côté GitHub.** eb4c a bien supprimé `hibernation-check.yml` (vérifié sur sa branche), mais le workflow vit encore sur `origin/main` avec son cron mensuel (06:00 UTC le 1ᵉʳ). La vue locale d'origin/main date du 30 mai : **vérifier si le firing du 2026-06-01 a déjà auto-commité** le fichier confidentiel décrit par M7 §5, et pousser la purge d'eb4c sur origin/main avant le 2026-07-01. | opérateur/remote | S |

Hors bloquant mais à dire : la re-mesure Linux/x86_64 reste pendante (le papier
l'annonce explicitement à deux endroits — acceptable en acceptation consciente, §6).

## 3. Tableau — chaque bloquant/HIGH de l'audit d'origine

Statuts : **FIXÉ** (commit) · **DESCOPÉ** (pourquoi) · **RESTANT**. Les commits
réfèrent aux branches chantier (non encore fusionnées, cf. R6) sauf mention « mergé ».

### CRITICAL (B1–B12)

| ID | Finding | Statut |
|----|---------|--------|
| B1 | Clé de cache non-injective (framing, slots, binding chemin↔contenu) | **RESTANT** — key.rs inchangé depuis le SHA de l'audit (seul le constructeur ContentHash a changé, H12). Jamais assigné à un chantier. |
| B2 | Contenu des scripts hors clé | **RESTANT** — idem, vérifié à HEAD intégration. |
| B3 | ProducerIndex O(R+P) décrit, inexistant | **RESTANT** (texte) — le code est celui que l'audit décrivait ; le papier n'a pas été requalifié. |
| B4 | Course cancel/completion → job annulé caché | **FIXÉ** `b89575d` (F1a) + court-circuit retry. |
| B5 | Collision silencieuse de JobId | **FIXÉ** `ac9ccb7` (F1b) — percent-escape + `DuplicateJobId`. |
| B6 | Temp file déterministe partagé + zéro fsync | **FIXÉ** `ef60095` (F1a). |
| B7 | Claim multi-session ne gate pas l'exécution | **RESTANT** (texte ou câblage) — état vérifié branche F4 : write-behind intact, heartbeat jamais appelé. H16/H17 (F4) ont durci la couche état seulement. |
| B8 | Chaîne cancel inexistante (SIGTERM, PID premier-venu) | **FIXÉ** `c5ae3e7` (F3) — SIGTERM graceful + `job_session_pid` (session propriétaire). |
| B9 | EvictPrecedesUnregister attribué à la mauvaise spec | **PARTIEL** — `23941ea` (F4) a purgé l'annexe ; restent tex l.854 (ligne INV-3c) et l.1256. |
| B10 | « Content-Addressable by Default » vs défaut mtime | **FIXÉ** `42b887b` (cfeb, **mergé**) — décision opérateur : défaut → `mtime+hash`. |
| B11 | Deux suites de benchmark contradictoires, fausse provenance | **FIXÉ** `0c959a9` (b539, **mergé**) — une suite, un chiffre ; chiffres rafraîchis dans le papier par M1 `b2f9ce2`. |
| B12 | SLURM : TIMEOUT/OOM classé succès → cache empoisonné | **FIXÉ** `149730c` (F2) — état terminal mappé avant l'exit code. |

### Bloquants premortem (PM#1–4)

| ID | Finding | Statut |
|----|---------|--------|
| PM#1 | `ox run` plante sur le fixture maison (bloc `run:`) | **FIXÉ** (7680, **mergé** `41d903d`) — préambule input/output/params/wildcards injecté + 2 bugs traducteur latents. |
| PM#2 | `bench_lib.py` non déclaré — contre-exemple livré armé | **FIXÉ** `0c959a9` (b539, mergé) + scoping « for declared inputs » partout (`28d0806`, 055a, mergé) + §threat-model sans sandbox. |
| PM#3 | Le benchmark mesure mtime, pas le content-addressing | **FIXÉ** — matrice git-checkout mesurée (b539) + §6.3 du papier (M1 `b2f9ce2`). La mesure a **falsifié la prémisse** (Snakemake 7.32.4 re-exécute 0 job) → rescope éditorial assumé (option a, décision opérateur). |
| PM#4 | « Cross-machine portable » contredit par OS+ARCH dans la clé | **FIXÉ (texte)** `42b887b` (cfeb, mergé) — claims bornés same-platform. **Re-mesure Linux/x86_64 RESTANTE** (acceptation consciente §6, le papier l'annonce). |
| PM#5* | Badge CI vire au rouge tout seul (drift-tripwire, hibernation) | **FIXÉ** `c6b3bc8` (eb4c) — décision opérateur : toutes gates calendaires supprimées, reviews indexées sur les releases `v*`. ⚠️ Voir R8 : la purge doit atteindre origin/main avant le cron du 2026-07-01. |

\* PM#5 figurait au plan d'action (§A.8) comme décision atomique, pas dans la liste numérotée.

### HIGH (H1–H39)

| ID | Finding | Statut |
|----|---------|--------|
| H1 | Contrainte regex capturante décale l'indexation | **FIXÉ** `81c5204` (F1b) — groupes nommés. |
| H2 | Wildcard répété sans égalité | **FIXÉ** `8a3d1fb` (F1b). |
| H3 | Récursion resolver non bornée (SIGSEGV) | **FIXÉ** `c96d25b` (F1b) — `MAX_RESOLVE_DEPTH` + erreur typée. |
| H4 | `env_hash` = spec littérale, pas le contenu | **RESTANT** — vérifié à HEAD intégration (serde_json de la spec). Même fenêtre que B1. |
| H5 | `shell_executable` hors clé | **RESTANT** — aucune occurrence dans l'assemblage de clé. Même fenêtre que B1. |
| H6 | TOCTOU split-lock find_ready_jobs | **FIXÉ** `afdcb71` (F1a). |
| H7 | `is_cached` re-hashe pendant l'écriture (torn read) | **FIXÉ** `c0376ee` (F1a) — registre PendingWrites. |
| H8 | Budget ressources fuit sur abort → hang | **FIXÉ** `01b5eed` (F1a) — guard Drop. |
| H9 | Tombstone mark_skipped coupe la fermeture transitive | **FIXÉ** `8587c5e` (F1b) — pontage à travers les skipped. |
| H10 | `unreachable!()` atteignable | **FIXÉ** `8587c5e` (F1b). |
| H11 | ResourceValue untagged : round-trip instable | **FIXÉ** `a3bf70a` (F1b) — Deserialize normalisant + proptests. |
| H12 | ContentHash/ComputationHash `pub String` sans validation | **FIXÉ** `8893ba4` (F1b) — champ privé + from_hex + Deserialize validant. |
| H13 | Écriture sans confinement workspace (`../`) | **FIXÉ** `7e7b6c0` (F1a) — défense lexicale + canonique. |
| H14 | Échec de persistance ignoré → run « succès » | **FIXÉ** `8c1803f` (F1a) — flush_disk_writer fail le run. |
| H15 | Pas de busy_timeout sur state.db/cache.db | **FIXÉ** `91ee414` (bd89, **mergé**) — 30 s + récupération corruption + `ox clean --state`. |
| H16 | complete_job/fail_job sans filtre session | **FIXÉ** `3eae6a5` (F4) — `AND session_id` + chemins reconcile_* explicites. |
| H17 | `ox clean` sans garde sessions vivantes | **FIXÉ** `d5fe4a0` (F4) — refus + `--force`. |
| H18 | Invariants TLA+ invérifiables par construction | **FIXÉ** `fb3a8ba` (F4) — invariants falsifiables + **2 configs rouges committées** (la preuve que le vert vérifie quelque chose). |
| H19 | Comptes TLC non reproductibles | **FIXÉ** `23941ea` (F4) — run-tlc.sh épinglé sha256 + 5 sorties de référence + table papier reconstruite. |
| H20 | ox.lock en fs::write direct | **FIXÉ** `0b5c9d8` (F4) — tmp+rename. |
| H21 | Cache distant : store non-atomique + fetch sans re-hash | **FIXÉ** en deux moitiés — fetch `42b887b` (cfeb, mergé), store `eae9fe8` (F4, répare les entrées empoisonnées). |
| H22 | Warm-fork : SIGKILL du template seul | **FIXÉ** `42170ac` (F2) — process-group + killpg. |
| H23 | Jobs warm jamais dans self.running → cancel no-op | **FIXÉ** `ea23cdd` (F2). |
| H24 | Ray : cancel(job_id) no-op | **FIXÉ** `0400123` (F2) — index job→driver, cascade `ray job stop`. |
| H25 | `&job_name[..255]` panique non-ASCII (SLURM) | **FIXÉ** `b7be604` (F2). |
| H26 | build_call_args : nommé avant anonyme → SyntaxError | **FIXÉ** `d16da2f` (F2, les deux crates). |
| H27 | `ox run -j 0` pend | **FIXÉ** `07aa289` (F3) — rejet clap. |
| H28 | `--report-json` documenté stable, inexistant | **FIXÉ** `b311227` (F3) — flag ajouté (contrat documenté honoré). |
| H29 | `include` collecté, développé nulle part | **FIXÉ** `aa355e1` (F3) — expansion transitive dans parse_workflow, toutes surfaces. |
| H30 | Truncate par octets → panic UTF-8 (Snakemake) | **FIXÉ** `83224e6` (F3). |
| H31 | Brace-matching WDL avale les sections | **FIXÉ** `64d3525` (F3) — heredocs opaques + diagnostic. |
| H32 | Types WDL non mappés sans escalation | **FIXÉ** `fac6a61` (F3). |
| H33 | scatter → zip vs product | **REQUALIFIÉ + FIXÉ** `bffea1b` (F3) — le finding tel qu'énoncé ne tenait pas (zip correct sur scatter simple) ; le vrai trou (scatter imbriqué N×M silencieux) est escaladé. Transparence du chantier notée. |
| H34 | 3 copies divergentes de la résolution de cibles | **FIXÉ** `2300d61` (F3) — canonique dans `ox_format::targets`. |
| H35 | `ox_clean` MCP ignore `what`, chemin legacy | **FIXÉ** `e0ae900` (F3). |
| H36 | Version Snakemake non pinnée dans le reproducer | **FIXÉ** (b539, mergé `03f8e92`). |
| H37 | Dérive de protocole bench (tailles, warmup) | **FIXÉ** (b539, mergé). |
| H38 | metrics bind 0.0.0.0 sans auth | **FIXÉ** `b6bd35a` (F4) — 127.0.0.1 par défaut, opt-in + warning. |
| H39 | README : env backends en features livrées | **FIXÉ** `f3ffad4` (F4) — délégation documentée, S3/GCS retirés des claims, Known limitations. |

**Bilan : 47 FIXÉ · 1 requalifié-fixé · 7 RESTANT (B1, B2, B3, B7, B9-partiel, H4, H5) · 0 descope explicite.**
Le point dur : les 7 restants n'ont **pas** été descopés — ils n'ont simplement été
confiés à personne (B1/B2/H4/H5 : le « chantier clé de cache » du plan §A.2 n'a été
exécuté qu'à 1/5, la part texte PM#4) ou déclarés faits à tort (B9). C'est un trou
de planification, pas une décision.

## 4. Candidats issue-jour-1

Consolidés dans **`docs/issues-day1.md`** (fichier compagnon de ce rapport, prêt à
transformer en issues GitHub après le flip) : 10 findings ouverts de M7 (F7–F15),
4 descopes de M9, 1 chip de F2 (clippy --all-targets), 2 chips de M1/bd89, la
re-mesure Linux, et 6 issues-parapluie pour les ~63 MEDIUM/LOW de l'annexe §7 de
l'audit (l'acceptation consciente §8.C promettait « tracés en issues publiques dès
le jour 1 » — les parapluies tiennent cette promesse).

## 5. Checklist opérateur — gestes finaux dans l'ordre

Chaque geste porte sa précondition vérifiable. Ne pas réordonner 5→8 (le papier
dépend du merge ; le tag dépend du flip dans la séquence de lancement choisie par 171c).

1. **Désamorcer la tripwire calendaire côté GitHub (R8).**
   Geste : `git fetch origin` puis vérifier qu'aucun commit auto du workflow
   hibernation n'a atterri sur origin/main au 2026-06-01 (`git log origin/main --since=2026-06-01 --oneline`) ;
   si oui, le révoquer avant tout le reste.
   *Précondition vérifiable : `git log` propre + le prochain firing (2026-07-01) tombe après l'étape 6.*

2. **Merger les 11 branches dans `feat/release-scaffolding-v0.1.0`.**
   Ordre suggéré (minimise les conflits ; les ⊃ notent les branches qui en contiennent d'autres) :
   `b5a7` (F4, basée sur le tip d'intégration) → `2dac` (F1b) → `d1c6` (F2) →
   `1cdf` (M9 ⊃ F3/8f1b) → `7ec6` (M7 ⊃ F1a/d970) → `eb4c` → `aa22` (M2 ⊃ 171c) →
   `97bc` (M1, **en dernier** : conflit .tex avec F4 à résoudre à la main — garder
   l'union : rescope M1 + table TLC F4) → `5a5d` (ce rapport).
   Attention au ripple H16 : `complete_job`/`fail_job` ont changé de signature (trait StateBackend).
   *Précondition vérifiable : les 4 gates DoD verts sur le tip mergé (`cargo check/test/clippy/fmt`).*

3. **Soirée papier (R3, R4, R5).** Requalifier §sec:scale-study (« precompiled-regex
   index; asymptotic complexity remains O(R·P) » + retoucher les 5 échos « amortized
   O(R+P) ») ; réécrire §Daemon-Free Cooperative Execution au conditionnel architectural
   (la couche état implémente et teste le protocole ; le câblage comme gate d'exécution
   est du staged work) ; purger les 2 occurrences restantes d'EvictPrecedesUnregister
   (l.854, l.1256). Rebuild : `make` dans docs/paper.
   *Précondition vérifiable : 0 citation indéfinie, 0 overfull hbox ; grep `O(R+P)`/`O(R{+}P)` et `EvictPrecedes` ne rendent plus que des usages honnêtes.*

4. **Décision atomique clé de cache (R1, R2).** Soit lancer le chantier unique
   B1+B2+H4+H5 (un seul changement de format de clé, property-tests, ~2-3 j) **avant
   le tag** — c'est la dernière fenêtre sans invalider les caches utilisateurs — soit
   signer les phrases d'acceptation du §6 (B2 n'a pas de phrase honnête : le fixer
   quoi qu'il arrive, il est S–M seul).
   *Précondition vérifiable : décision enregistrée (ADR ou note d'audit) + si fix : proptests d'injectivité verts.*

5. **Régénérer le tarball arXiv depuis le tex mergé** (celui de M1 est périmé par la
   table TLC de F4) + vérification en isolation (untar dans /tmp, `pdflatex ×2`, 0 undefined)
   + re-stamping `.ots` du PDF final (le sceau actuel scelle un PDF périmé — vigilance M1).
   *Précondition vérifiable : compile isolée propre ; nombre de pages reporté dans le champ Comments à jour.*

6. **Merge `feat/release-scaffolding-v0.1.0` → `main`, push origin.**
   Réconcilier d'abord le référé release-audit avec l'exemption forbid-strings (R7).
   *Précondition vérifiable : `scripts/release-checklist.sh` → « READY for the flip » (gates pre-flip 1-12 PASS) ; `cs release-audit --dry-run` vert ; working tree clean.*

7. **Flip public** (GitHub settings → visibility), puis dérouler les gates post-flip
   (2, 4, 13, 14 du release-checklist : branch protection, required checks).
   Séquencement amont recommandé par 171c (docs/LAUNCH-SEQUENCE.md) : primer 5-10
   utilisateurs Snakemake en privé avant le flip si la fenêtre le permet.
   *Précondition vérifiable : étape 6 complète ; release-checklist post-flip PASS.*

8. **Tag : `just tag-release 0.1.0`.**
   Le recipe vérifie lui-même : tree clean, branche main. Le workflow release.yml
   gate ensuite sur test+clippy+cargo-deny (SHA-pinnés, toolchain 1.94.1).
   *Précondition vérifiable : `spec/tla/REVIEWS.md` contient une entrée REVIEW ≥ date du tag (item 16 du release-checklist — l'entrée 2026-06-10 d'eb4c la satisfait si mergée).*

9. **cargo publish — ⚠️ précondition manquante aujourd'hui.** Aucun crate nommé
   `oxymake` n'existe dans le workspace (le binaire est `ox`, crate `ox-cli`).
   RELEASING.md prévoit la réservation du nom + un crate placeholder optionnel ;
   les noms PyPI/Homebrew sont des gestes registry séparés (171c).
   *Précondition vérifiable : nom crates.io réservé ; décision placeholder vs publication des 24 crates prise (runbook RELEASING.md §71).*

10. **Dépôt arXiv** avec le tarball de l'étape 5 (pas celui de M1). Abstract ≤ 1920
    caractères : la version M1 fait 1 864 ✓ — re-vérifier après la soirée papier.
    *Précondition vérifiable : tarball = sortie de l'étape 5 ; cross-list cs.SE conforme à arxiv-metadata.txt.*

11. **Zotero/HAL.** Une fois l'ID arXiv assigné : pipeline référence complet
    (Zotero + PDF + raw/ + wiki/ + index.json, per CLAUDE.md global) ; dépôt HAL
    optionnel au choix de l'opérateur.
    *Précondition vérifiable : ID arXiv assigné ; entrée Zotero visible côté cloud (discipline split-brain bibion).*

12. **Jour 1 post-flip** : créer les issues depuis `docs/issues-day1.md`
    (`gh issue create` par bloc) — c'est la contrepartie écrite de l'acceptation
    consciente §6. Puis tap Homebrew + réservation PyPI (gestes registry, 171c).
    *Précondition vérifiable : nombre d'issues ouvertes = nombre d'entrées du fichier.*

## 6. Phrases d'acceptation consciente (format §8.C de l'audit)

Pour ce qui reste **si** l'opérateur choisit de publier sans les fixer :

- **Les ~63 MEDIUM/LOW de l'annexe §7** : « Nous publions avec ces défauts connus,
  tracés en issues publiques dès le jour 1 (issues-parapluie de docs/issues-day1.md) ;
  aucun ne casse une promesse du papier ni ne corrompt de données sur le chemin
  nominal mono-session. »
- **Re-mesure Linux/x86_64 pendante** : « Nous publions des mesures mono-plateforme
  (M4 Max) en le disant dans l'abstract et en tête de §6 ; le re-run Linux est un
  engagement public, pas un non-dit. »
- **B1 (injectivité de la clé)** : « Nous publions une clé dont l'injectivité repose
  sur la forme des entrées, pas sur un framing prouvé ; les collisions exigent des
  entrées construites, le chemin nominal n'en produit pas ; le changement de format
  est planifié pour 0.2.0 avec invalidation de cache annoncée. » ⚠️ Cette phrase
  coûte : elle concède publiquement que la fenêtre « sans casse » se referme.
- **H4/H5 (env spec, shell hors clé)** : « Nous publions en documentant que la clé
  hashe la *déclaration* d'environnement, pas son contenu résolu, et que le shell
  exécutant n'y entre pas ; les utilisateurs qui changent d'env ou de shell doivent
  `--forcerun` — limitation tracée en issue jour 1. »
- **B2 (contenu des scripts)** : **pas de phrase honnête disponible.** Elle
  contredirait frontalement la première phrase du §4.1 du papier (« the source of
  truth is file content »). Fixer avant le tag (S–M, le mécanisme param_files existe).
- **B7 (claim multi-session)** : **pas de phrase d'acceptation — c'est une réécriture,
  pas une acceptation.** Le claim tel qu'imprimé est réfutable en deux terminaux et
  30 secondes. La version honnête (protocole implémenté et testé dans la couche état,
  câblage comme gate = staged work) reste une contribution défendable.
- **Stubs env/storage** : « Nous publions des crates stubs déclarés comme tels dans
  le papier et le README (fait, H39/f3ffad4) ; la délégation `uv run`/`conda run` de
  l'exécuteur local est documentée comme telle. »
- **Cold-run plus lent que Snakemake** : « Nous publions le 0.80×/0.44×/0.70× tel
  quel (chiffres 2026-06-10) — posture du papier inchangée, on ne la dilue pas. »
- **Bus-factor 1** : « Nous publions en mono-mainteneur best-effort, politique
  écrite dans le README (fait, eb4c §3) avec contact sécurité ; la lenteur annoncée
  est un contrat tenu. »
- **Model-checking borné** : « Nous publions des specs bornées avec leurs bornes
  nommées, un runner reproductible épinglé, et **deux configs rouges committées qui
  prouvent que le vert vérifie quelque chose** (fait, H18/H19) ; "formally-specified"
  décrit ce qui est commité, pas plus. »

## 7. Anti-lissage — contradictions et déclarations à corriger

1. **F4 a déclaré B9 « FIXÉ » à tort** : seule l'occurrence du tableau d'annexe a
   été purgée ; les lignes tex 854 et 1256 attribuent toujours INV-3c à
   CancelPropagation.tla, qui ne définit pas l'invariant (mention en commentaire
   seulement, vérifié sur la branche F4). Reclassé PARTIEL ici.
2. **eb4c et M7 semblent se contredire** (eb4c : « hibernation-check.yml supprimé » ;
   M7 §5 : « T2 a déjà expiré, le workflow va commiter »). Ce n'est pas une vraie
   contradiction : M7 a audité une branche qui ne contient pas eb4c. Mais la note
   M7 §5 qualifiant la deadline drift-tripwire d'« intentionnelle, documentée »
   contredit la décision opérateur **binding** enregistrée par eb4c (suppression
   totale des gates calendaires) — le rapport M7 est périmé sur ce point, foi à eb4c.
3. **Le chantier clé de cache n'a jamais existé** : le plan d'action §A.2 de l'audit
   (« B1+B2+H4+H5+PM#4, un seul changement de format, maintenant ou jamais ») n'a
   été couvert qu'à hauteur de PM#4-texte (cfeb). Aucun prompt de chantier F ne
   mentionne B1/B2/H4/H5. Trou de planification — surfacé ici, pas lissé.
4. **B3/B7 tombés entre 055a et M1** : 055a (honesty pass) a traité abstract,
   sandbox, model-checking ; M1 (rescope) a traité la prémisse falsifiée. Chacun
   pouvait croire l'autre en charge des deux dernières réécritures « une soirée
   chacune » du plan §A.5. Aucune ne l'était.
5. **TDD déclaré vs vérifiable** : F1a/F1b/F2/F4 affirment rouge-avant-fix pour
   chaque finding, avec noms de tests cités — cohérent avec les messages de commit.
   F3 déclare une demi-exception assumée (moitié handler SIGTERM de B8, test écrit
   pendant le fix mais démontrablement rouge sans lui). Aucun « FIXÉ sans test »
   détecté dans les cinq chantiers.
6. **Requalification honnête de H33 par F3** (zip correct sur scatter simple ; le
   vrai trou était le scatter imbriqué) — c'est l'inverse du lissage, noté au crédit.
7. **M2 a corrigé sa propre exagération** (« five → four molecules » après relecture
   du rapport de citations) — idem.

---

*Rapport produit par task-20260610-5a5d (M8). Compagnon : `docs/issues-day1.md`.
Toutes les assertions code/papier ci-dessus ont été contre-vérifiées par git aux
branches nommées le 2026-06-10 ; les rapports de chantier seuls n'ont pas fait foi.*

---

## ADDENDUM — clôture des restes (2026-06-10, session opérateur)

Les 8 restes du verdict §2 sont traités ; le verdict **PAS-PRÊT est levé**.

| Reste | Traitement | Preuve |
|---|---|---|
| R1+R2 (clé de cache B1/B2/H4/H5) | Chantier unique `task-20260610-562e` : format **v2 injectif** — version tag, framing par champ, paires (path,hash), contenu des scripts + env par contenu + shell dans la clé ; property-tests d'injectivité + golden-key | commit `737bfc1` (+ adaptation API `2a946ae`) |
| R3+R4+R5 (papier) | `task-20260610-4f81` : O(R+P) requalifié en O(R·P)-constante-réduite (index = future work), claim multi-session au conditionnel architectural, EvictPrecedesUnregister purgé | commit `025ecda` |
| R6 (intégration) | Train de merges complet (14 branches), conflits .tex/README/release.yml résolus, PDF + tarball arXiv régénérés, build d'isolation vérifié (38 p., 0 undefined) | merges `4742eee`…`eef9ffb` |
| R7 (release-audit vs forbid-strings) | `task-20260610-04fe` : exemptions réconciliées, règle documentée dans RELEASING.md | commit `e77e06a` |
| R8 (cron calendaire sur origin/main) | Vérifié : **aucun tir, aucun auto-commit, aucune fuite** (dernier commit origin/main = 30 mai) ; la purge d'eb4c part avec le push final — à pousser avant le 2026-07-01 | fetch + log du 2026-06-10 |

Gates finaux sur l'arbre intégré : **check ✓ test ✓ (0 échec) clippy ✓ fmt ✓**.
Le golden-key a été rafraîchi une fois (octets de fixture changés par l'adaptation
API, pas le format v2 — aucun cache n'existe encore dans la nature).

**Verdict final : PRÊT** pour la séquence opérateur — merge `feat→main` → push
(porte la purge du cron) → flip public → `just tag-release 0.1.0` →
`cargo publish -p oxymake` → dépôt arXiv (`oxymake-arxiv-source.tar.gz`).
Acceptations conscientes inchangées (§6) ; `docs/issues-day1.md` prêt à matérialiser.
