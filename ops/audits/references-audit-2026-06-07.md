# OxyMake paper — consolidated citation audit

**Date:** 2026-06-07
**Synthesis molecule:** `task-20260607-06b7`
**Source file audited:** `docs/paper/references.bib` (44 entries, canonical superset)
**Lots synthesised:** setup `task-20260607-43f9` (reconciliation + worklist) ·
verification `task-20260607-1327`, `task-20260607-c53b`,
`task-20260607-51e9`, `task-20260607-2f4d` (11 keys each → 44 total).
**Zotero collection (live):** `OxyMake` — key `3ZNVDPBF`.

---

## Résumé exécutif (registre Feynman)

On a passé chaque référence du papier au détecteur de mensonges : pour les 44,
on a comparé ce que dit le fichier `.bib` à ce que dit la source officielle
(Crossref, l'éditeur, arXiv). Pense à 44 cartes d'identité qu'on vérifie une par
une au guichet.

La bonne nouvelle : **aucune référence n'est inventée**. Toutes existent, toutes
les adresses (DOI) mènent au bon endroit — aucun lien mort. Le `.bib` ne contient
aucune carte d'identité fabriquée de toutes pièces.

La mauvaise nouvelle : **une carte a un faux nom dessus**. La référence AFLOW
(zhang) liste un auteur — « Wu, Yiqi » — qui n'a jamais écrit ce papier, et en
oublie trois vrais. C'est une hallucination réelle, encore présente dans le
fichier aujourd'hui. Tant qu'elle est là, l'audit ne peut pas dire « tout est
propre ».

Trois autres cartes ont des listes d'auteurs **tronquées sans le dire** (comme
écrire « Dupont » au lieu de « Dupont, Martin, et 13 autres »). Deux ont été
réparées pendant la vérification ; une (crusoe) reste à corriger.

Enfin, deux problèmes de plomberie : (1) la bibliographie compilée du papier
(`.bbl`) est **périmée** — trois citations s'affichent en `[?]` dans le PDF
arXiv-v1 parce que le `.bbl` date d'avant leur ajout ; un simple rebuild règle
ça. (2) Une confusion de rangement : le setup a créé une collection Zotero
« OxyMake (paper) » mais les 44 refs ont été classées dans l'ancienne « OxyMake ».

**Verdict : FAIL** — 1 hallucination ouverte (zhang/AFLOW) + 1 liste tronquée
ouverte (crusoe) + 1 divergence DOI côté Zotero (mitchellShake) + 3 citations
`[?]` dues au `.bbl` périmé. Zéro DOI cassé, zéro référence fabriquée.
**5 issues ouvertes** au total (détail §2/§3). Toutes corrigeables ; aucune
n'invalide le fond bibliographique du papier.

---

## 1. Tableau complet des 44 références

Légende : **Cité** ✅ = `\cite{}` dans le `.tex` courant · 💀 = mort (dans `.bib`,
non cité) · ⚠️ = cité mais absent du `.bbl` (citation `[?]`).
**Match** : ✅ exact · 🔧 défaut trouvé **et corrigé** · ❌ défaut **ouvert**.

| # | Clé BibTeX | Cité | Titre vérifié | Auteurs OK ? | Année OK ? | DOI vérifié / résout ? | Match | Lot |
|--:|------------|:--:|---------------|:--:|:--:|------------------------|:--:|:--:|
| 1 | `mastermanLandscapeEmergingAI2024` | ⚠️ | ✅ | ✅ | ✅ | ✅ 10.48550/arXiv.2404.11584 (DataCite) | ✅ | 1327 |
| 2 | `wangSurveyLargeLanguage2024` | ⚠️ | ✅ | ✅ (13) | ✅ | ✅ 10.1007/s11704-024-40231-1 | ✅ | 1327 |
| 3 | `zhangAFLOWAutomatingAgentic2025` | ⚠️ | ⚠️ casse « AFLOW » vs « AFlow » | ❌ **« Wu, Yiqi » fabriqué ; 3 auteurs réels omis** | ✅ | n/a (pas de DOI) | ❌ | c53b |
| 4 | `adhikariSurveySchedulingStrategies2019` | ✅ | ✅ | 🔧 prénom 1er auteur **Mala→Mainak** | ✅ | ✅ 10.1145/3325097 | 🔧 | 1327 |
| 5 | `afganGalaxyPlatformAccessible2018` | ✅ | ✅ | ✅ (10 + `and others`, 20 réels) | ✅ | ✅ 10.1093/nar/gky379 | ✅ | c53b |
| 6 | `airflow` | ✅ | ✅ | ✅ Beauchemin + ASF | ✅ | n/a URL live | ✅ | 51e9 |
| 7 | `argoWorkflows` | ✅ | ✅ | ✅ Argoproj | ✅ | n/a URL live | ✅ | 51e9 |
| 8 | `bazel` | ✅ | ✅ | ✅ Google | ✅ | n/a URL live | ✅ | 51e9 |
| 9 | `blake3` | ✅ | ✅ | ✅ (4 designers) | ✅ | n/a URL live | ✅ | 51e9 |
| 10 | `blake3spec` | ✅ | ✅ | ✅ (4) | ✅ | n/a URL live (PDF 297 KB) | ✅ | 51e9 |
| 11 | `buck2` | ✅ | ✅ (casse only) | ✅ Meta Eng. | ✅ | n/a URL live | ✅ | 51e9 |
| 12 | `chueHongFAIR4RSPrinciples2022` | ✅ | ✅ | ✅ (11) | ✅ (DataCite stale 2021 ; Zenodo = 2022 ✓) | ✅ 10.15497/RDA00068 (DataCite) | ✅ | 2f4d |
| 13 | `courtesGuixHPCReproducible2015` | ✅ | ✅ | ✅ (2) | ✅ | ✅ 10.1007/978-3-319-27308-2_47 | ✅ | 2f4d |
| 14 | `crusoeMethodsIncludedStandardizing2022` | ✅ | ✅ | ❌ **8/11, pas de `and others`** (omet Gavrilović, Goble, "CWL Community") | ✅ | ✅ 10.1145/3486897 | ❌ | c53b |
| 15 | `cue` | ✅ | ✅ | ✅ van Lohuizen | ✅ | n/a URL live | ✅ | 2f4d |
| 16 | `deelmanPegasusWorkflowManagement2015` | ✅ | ✅ | ✅ (11) | ✅ | ✅ 10.1016/j.future.2014.10.008 | ✅ | c53b |
| 17 | `dhall` | ✅ | ✅ | ✅ Gonzalez | ✅ | n/a URL live | ✅ | 2f4d |
| 18 | `diTommasoNextflowEnablesReproducible2017` | ✅ | ✅ | ✅ (6) | ✅ | ✅ 10.1038/nbt.3820 | ✅ | c53b |
| 19 | `dolstraPurelyFunctionalSoftware2006` | ✅ | ✅ | ✅ Dolstra | ✅ | n/a (thèse PhD) | ✅ | 1327 |
| 20 | `feldmanMakeProgram1979` | ✅ | ✅ | ✅ Feldman | ✅ | ✅ 10.1002/spe.4380090402 | ✅ | c53b |
| 21 | `gobleFAIRComputationalWorkflows2020` | ✅ | ✅ | ✅ (8) | ✅ | ✅ 10.1162/dint_a_00033 | ✅ | 1327 |
| 22 | `kosterSnakemakeScalableBioinformatics2012` | ✅ | ✅ | ✅ (2) | ✅ | ✅ 10.1093/bioinformatics/bts480 | ✅ | 1327 |
| 23 | `lamportSpecifyingSystems2002` | ✅ | ✅ | ✅ Lamport | ✅ | n/a (livre Addison-Wesley) | ✅ | 2f4d |
| 24 | `mitchellShakeBuildSystem2012` | ✅ | ✅ | ✅ Mitchell | ✅ | ✅ 10.1145/2364527.2364538 (`.bib` correct ; **Zotero porte un DOI différent**) | ❌ (Zotero) | c53b |
| 25 | `molderSustainableDataAnalysis2021` | ✅ | ✅ | 🔧 **10→15 auteurs** (5 du milieu restaurés) | ✅ | ✅ 10.12688/f1000research.29032.2 | 🔧 | 1327 |
| 26 | `moritzRayDistributedFramework2018` | ✅ | ✅ | ✅ (11) | ✅ | n/a (OSDI'18 / arXiv 1712.05889) | ✅ | c53b |
| 27 | `petgraph` | ✅ | ✅ | ✅ contributors | ✅ | n/a URL live | ✅ | 51e9 |
| 28 | `prinsGuixCWLPipelines2018` | ✅ | ✅ | ✅ Prins | ⚠️ clé dit 2018, `year=2020` (README nov 2020 ✓) | n/a URL live | ✅ | 2f4d |
| 29 | `rocklinDaskParallelComputation2015` | ✅ | ✅ | ✅ Rocklin | ✅ | ✅ 10.25080/Majora-7b98e3ed-013 | ✅ (**dupliqué** Zotero `…2015a`) | c53b |
| 30 | `soilandReyesPackagingResearch2022` | ✅ | ✅ | ✅ (16 incl. RO-Crate Community) | ✅ | ✅ 10.3233/DS-210053 | ✅ | 2f4d |
| 31 | `starlark` | ✅ | ✅ | ✅ Bazel contributors | ✅ | n/a URL live | ✅ | 2f4d |
| 32 | `topcuogluPerformanceeffectiveAndLowcomplexity2002` | ✅ | ✅ | ✅ (3) | ✅ | ✅ 10.1109/71.993206 | ✅ | c53b |
| 33 | `vossWDLCromwell2017` | ✅ | ✅ | ✅ (3) | ✅ | ✅ 10.7490/f1000research.1114634.1 | ✅ | 2f4d |
| 34 | `wilkinsonApplyingFAIRPrinciples2025` | ✅ | ✅ | 🔧 2e auteur **Alomairy→Aloqalaa** ; 9→23 (`and others`) | ✅ | ✅ 10.1038/s41597-025-04451-9 | 🔧 | 1327 |
| 35 | `wurmusPiGxReproducibleGenomics2018` | ✅ | ✅ | ✅ (8) | ✅ | ✅ 10.1093/gigascience/giy123 | ✅ | 2f4d |
| 36 | `arrowRust` | 💀 | ✅ | ✅ Apache Arrow contributors | ✅ | n/a URL live | ✅ | 51e9 |
| 37 | `benetIPFSContentAddressed2014` | 💀 | ✅ | ✅ Benet | ✅ | n/a arXiv:1407.3561 | ✅ | 1327 |
| 38 | `mitchellFAIRDataPipeline2022` | 💀 | ✅ | 🔧 **Zotero** : faux co-auteurs « Sheridan » supprimés (`.bib` toujours correct) | ✅ | ✅ 10.1098/rsta.2021.0300 | 🔧 (Zotero) | 51e9 |
| 39 | `mokhovBuildSystemsCarte2018` | 💀 | ✅ | ✅ (3) | ✅ | ✅ 10.1145/3236774 | ✅ | 1327 |
| 40 | `mokhovBuildSystemsCarte2020` | 💀 | ✅ | ✅ (3) | ✅ | ✅ 10.1017/S0956796820000088 | ✅ | 1327 |
| 41 | `newcombeHowAmazonWebServices2015` | 💀 | ✅ | ✅ (6) | ✅ | ✅ 10.1145/2699417 | ✅ | c53b |
| 42 | `pengReproducibleResearchComputational2011` | 💀 | ✅ | ✅ Peng | ✅ | ✅ 10.1126/science.1213847 | ✅ | 51e9 |
| 43 | `souzaPROVAGENTUnifiedProvenance2025` | 💀 | ✅ | ✅ (8) | ✅ | ✅ 10.1145/3731599.3767582 | ✅ | 2f4d |
| 44 | `stoddenEnhancingReproducibilityComputational2016` | 💀 | ✅ | ✅ (9) | ✅ | ✅ 10.1126/science.aah6168 | ✅ | 51e9 |

**Bilan colonne Match :** 37 ✅ exact · 4 🔧 corrigés (adhikari, molder, wilkinson,
mitchellFAIRDataPipeline-Zotero) · 3 ❌ ouverts (zhang, crusoe, mitchellShake-Zotero).
**DOI :** 27 DOI résolvent tous correctement · 0 cassé · 0 pointant ailleurs.

---

## 2. HALLUCINATIONS / MISMATCHES

### 2.1 OUVERT — `zhangAFLOWAutomatingAgentic2025` · hallucination d'auteur (HIGH)

État vérifié dans `docs/paper/references.bib` au 2026-06-07 : **non corrigé.**

```
author = {Zhang … Wang, Jinlin and Wu, Yiqi and Wu, Chenglin}   ← 12 auteurs
```

- **« Wu, Yiqi » n'existe pas** dans la liste autoritative (arXiv 2410.10762,
  ICLR 2025 Oral). Auteur fabriqué.
- **3 auteurs réels omis** : Bingnan Zheng, Bang Liu, Yuyu Luo.
- Liste réelle = **14** auteurs ; queue correcte après « Wang, Jinlin » :
  *Zheng, Bingnan ; Liu, Bang ; Luo, Yuyu ; Wu, Chenglin*.
- Casse du titre : autoritatif « AF**l**ow », `.bib` « AF**L**OW ».
- **Action :** corriger `references.bib` **et** l'item Zotero `zhangAFLOW…`.
  C'est la seule vraie hallucination du corpus → bloque le verdict PASS.

### 2.2 OUVERT — `crusoeMethodsIncludedStandardizing2022` · liste tronquée (MEDIUM)

État vérifié : **non corrigé** — 8 auteurs, pas de marqueur de troncature.

- `.bib` s'arrête à 8 (…Soiland-Reyes) ; Crossref en liste **11** : ajoute
  **Bogdan Gavrilović, Carole Goble, « The CWL Community »**.
- **Action :** restaurer la liste complète, ou ajouter `and others`. Sans
  marqueur, c'est une troncature silencieuse (mismatch, pas hallucination).

### 2.3 OUVERT — `mitchellShakeBuildSystem2012` · divergence DOI côté Zotero (LOW)

- `.bib` DOI **10.1145/2364527.2364538** = **correct** (Crossref → « Shake before
  building »). L'item Zotero `mitchellShakeBuildingReplacing2012` porte un DOI
  **différent** : 10.1145/2398856.2364538.
- **Action :** aligner le DOI Zotero sur la valeur `.bib`. `.bib` non touché.

### 2.4 CORRIGÉ pendant la vérification (pour mémoire)

| Clé | Défaut | Correction appliquée | Lot |
|-----|--------|----------------------|-----|
| `adhikariSurveySchedulingStrategies2019` | prénom 1er auteur **Mala** (faux) | → **Mainak** dans `.bib` + Zotero | 1327 |
| `wilkinsonApplyingFAIRPrinciples2025` | 2e auteur **Alomairy** (faux) ; 9 auteurs (incomplet) | → **Aloqalaa** ; `and others` ajouté (23 réels) ; Zotero corrigé | 1327 |
| `molderSustainableDataAnalysis2021` | 10 auteurs, 5 du milieu omis sans marqueur | liste **15** restaurée (`.bib` + Zotero) | 1327 |
| `mitchellFAIRDataPipeline2022` | **Zotero** : co-auteurs « Sheridan » fabriqués | remplacés par la liste vérifiée ; `.bib` était déjà correct | 51e9 |

---

## 3. RÉCONCILIATION tex / bbl / bib

Source : `task-20260607-43f9/reconciliation.md`. Arithmétique : citées 35,
bibitems `.bbl` 37, entrées `.bib` 44. (32 citées∩rendues) + 5 orphelins = 37 ✓ ;
44 − 35 = 9 mortes ✓.

> ⚠️ Il n'y avait, à l'heure de l'audit, **aucun `.bbl` sur disque** : il vivait
> dans le tarball de soumission arXiv d'origine (gelé). D'où sa péremption.
> (Ce tarball d'origine a depuis été remplacé par `oxymake-arxiv-source.tar.gz`,
> au `.bbl` régénéré.)

### 3.1 Citées-mais-non-définies (`.bbl` périmé → `[?]`) — **HIGH, 3 clés**

| Clé | `.tex` | `.bib` | `.bbl` | Effet |
|-----|:--:|:--:|:--:|-------|
| `mastermanLandscapeEmergingAI2024` | ✅ | ✅ | ❌ | `[?]` |
| `wangSurveyLargeLanguage2024` | ✅ | ✅ | ❌ | `[?]` |
| `zhangAFLOWAutomatingAgentic2025` | ✅ | ✅ | ❌ | `[?]` |

Les 3 citations IA-agents ajoutées **après** le gel du `.bbl` arXiv-v1. Métadonnées
présentes dans `.bib` → problème purement d'artefact périmé.
**Fix :** re-`bibtex`/`biber` + rebuild → le `.bbl` se régénère.

### 3.2 Bibitems orphelins (`.bbl` mais plus cités) — LOW, 5 clés

`mokhovBuildSystemsCarte2018`, `mokhovBuildSystemsCarte2020`,
`newcombeHowAmazonWebServices2015`, `pengReproducibleResearchComputational2011`,
`stoddenEnhancingReproducibilityComputational2016`. Sous-ensemble des mortes (§3.3).
Rendus dans v1, `\cite{}` retiré depuis. Un rebuild les retire automatiquement.

### 3.3 Entrées mortes (`.bib` jamais citées) — LOW, 9 clés

`arrowRust`, `benetIPFSContentAddressed2014`, `mitchellFAIRDataPipeline2022`,
`mokhovBuildSystemsCarte2018`, `mokhovBuildSystemsCarte2020`,
`newcombeHowAmazonWebServices2015`, `pengReproducibleResearchComputational2011`,
`souzaPROVAGENTUnifiedProvenance2025`, `stoddenEnhancingReproducibilityComputational2016`.
Inoffensives au rendu (BibTeX les ignore) mais gonflent la surface de vérif.
**Décision auteur :** re-citer ou élaguer. (Toutes vérifiées ✅ malgré tout.)

### 3.4 Pas d'hallucination de clé, pas de bibitem fantôme

- **Finding A** (cité mais absent du `.bib`) : **AUCUN**. ✅
- **Finding E** (bibitem sans `@entry` source) : **AUCUN**. ✅

### 3.5 Une action règle 3.1 + 3.2

**Régénérer le `.bbl` depuis le `.tex` + `.bib` courants** efface les 3 `[?]` et
les 5 orphelins. Les 9 mortes (§3.3) sont une décision éditoriale, pas un bug.

---

## 4. ZOTERO

**Collection live confirmée :** `OxyMake` — clé **`3ZNVDPBF`**.
**Toutes les 44 refs citées/présentes dans le `.bib` sont dans `3ZNVDPBF`.**
Aucune manquante. (25 préexistantes, 19 créées par les lots 51e9 + 2f4d ; PDF
attachés selon §5.)

### 4.1 ⚠️ Divergence de collection (housekeeping, à trancher par l'opérateur)

Le setup `task-20260607-43f9` a **créé une nouvelle collection** « OxyMake (paper) »
clé **`755N3T92`**. Mais les **4 lots de vérification ont tous classé / vérifié
les items dans l'ancienne `OxyMake` `3ZNVDPBF`** (les workers ont noté que
« OxyMake (paper) » n'existait pas sous ce nom au moment de la vérif). Résultat :
la collection `755N3T92` créée au setup est probablement **vide ou partielle** ;
la collection de travail réelle est `3ZNVDPBF`.
**Action :** soit fusionner/supprimer `755N3T92`, soit y déplacer les 44 items si
l'on veut une collection « paper » distincte. Sans impact sur le rendu du papier.

### 4.2 Doublon Zotero

`rocklinDaskParallelComputation2015` **et** `…2015a` = même papier (même DOI
10.25080/Majora-7b98e3ed-013). Candidat dé-duplication.

---

## 5. PDF MANQUANTS (prêts pour passe scihub/scidb opérateur-gatée)

`bulk_scihub_fetch` exige `COSMON_OPERATOR_GESTURE=1` — **non disponible aux
workers**. Liste prête pour une passe opérateur :

### 5.1 needs-scihub (8 refs — pas d'OA valide trouvé)

| Clé | DOI | Note |
|-----|-----|------|
| `newcombeHowAmazonWebServices2015` | 10.1145/2699417 | CACM, pas d'OA |
| `feldmanMakeProgram1979` | 10.1002/spe.4380090402 | SPE 1979 |
| `diTommasoNextflowEnablesReproducible2017` | 10.1038/nbt.3820 | Nature Biotech |
| `deelmanPegasusWorkflowManagement2015` | 10.1016/j.future.2014.10.008 | FGCS |
| `mitchellShakeBuildSystem2012` | 10.1145/2364527.2364538 | ICFP, pas d'OA |
| `rocklinDaskParallelComputation2015` | 10.25080/Majora-7b98e3ed-013 | URL OA 404 |
| `topcuogluPerformanceeffectiveAndLowcomplexity2002` | 10.1109/71.993206 | IEEE TPDS ; OA non-PDF valide |
| `vossWDLCromwell2017` | 10.7490/f1000research.1114634.1 | poster F1000, aucun OA / 8 sources |

### 5.2 OA-retry (1 ref — PAS de scihub, OA existe, endpoints transitoirement KO)

| Clé | DOI | Note |
|-----|-----|------|
| `pengReproducibleResearchComputational2011` | 10.1126/science.1213847 | OA via PMC3383002 ; EuropePMC 500 + core.ac.uk refus → **réessayer**, pas scihub |

### 5.3 Déjà OA / attaché

35 refs restantes : PDF attaché (OA arXiv / Zenodo / EuropePMC / core.ac.uk /
publisher) **ou** n/a (logiciel/spec/site/blog/livre — pas de concept de PDF).

---

## Verdict final

**FAIL** — 0 DOI cassé, 0 référence fabriquée, mais **5 issues ouvertes** :

| # | Issue | Sévérité | Surface |
|--:|-------|----------|---------|
| 1 | `zhang/AFLOW` — auteur « Wu, Yiqi » fabriqué + 3 omis + casse titre | **HIGH** | `.bib` + Zotero |
| 2 | `.bbl` périmé → 3 citations `[?]` (masterman, wang, zhang) | **HIGH** | rendu PDF |
| 3 | `crusoe` — liste tronquée 8/11 sans `and others` | MEDIUM | `.bib` |
| 4 | `mitchellShake` — DOI divergent côté Zotero | LOW | Zotero |
| 5 | Collection Zotero `755N3T92` (setup) vs `3ZNVDPBF` (travail) | LOW | Zotero housekeeping |

**Le PASS est à un geste :** corriger l'auteur `zhang/AFLOW` (#1), rebuild le
`.bbl` (#2), compléter `crusoe` (#3). #4/#5 sont du housekeeping Zotero hors
chemin de rendu. Aucune issue n'invalide le fond bibliographique — 37/44 refs
exactes, 4 déjà corrigées, 27/27 DOI résolvent.

---

## ADDENDUM — landing opérateur (2026-06-07, session porte-ouverte)

Suite de l'audit, exécutée dans la session opérateur (geste `COSMON_OPERATOR_GESTURE=1`
actif). Corrige deux verdicts du rapport ci-dessus et clôt les issues.

### ⚠️ Issue #2 était un FAUX POSITIF — rétractée

Le `.bbl` n'a **jamais** produit de citation `[?]`. Les `\cite{masterman,wang,zhang}`
vivent à l'intérieur d'un bloc **`\iffalse ... \fi`** (oxymake-paper.tex L516,
commentaire « QUARANTINED for the FAIR venue … preserved for a future systems/SE
venue »). Vérifié dans le `.aux` : **aucune** `\citation{}` n'est émise pour ces 3
clés. Le grep texte du setup a compté des `\cite` désactivés comme actifs. Rebuild
propre : **0 citation indéfinie, 33 pages**. Le tarball arXiv v1 n'était pas cassé.
→ Issue #2 **supprimée** ; il reste **4** issues, aucune HIGH sur le chemin de rendu.

### ✅ Issues #1 et #3 corrigées dans `references.bib` (committé)

Les 5 défauts métadonnées corrigés et vérifiés à la source (Crossref / arXiv 2410.10762) :
adhikari (Mala→**Mainak**), molder (10→**15**), wilkinson (Alomairy→**Aloqalaa** + `and others`),
zhang/**AFlow** (« Wu, Yiqi » fabriqué retiré ; +Zheng,Liu,Luo), crusoe (8→**11**).
`.bbl` régénéré, PDF reconstruit, **tarball source arXiv** produit (`oxymake-arxiv-source.tar.gz`).

### 📄 PDF — passe scihub/scidb opérateur (6/8 récupérés)

Attachés aux items Zotero : feldman, deelman, diTommaso, topcuoglu (+ mitchellShake
en avait déjà un) = **5 attachés**. `newcombe` (10.1145/2699417) récupéré en cache
mais **entrée morte, pas d'item Zotero**. **Indisponibles partout (scidb + 4 miroirs) :**
`rocklin` (10.25080/Majora-7b98e3ed-013 — SciPy, OA mais URL morte) et `voss`
(10.7490/f1000research.1114634.1 — poster F1000, aucun OA).

### ⚠️ Constat : les write-back Zotero des workers n'ont pas persisté

La collection live `3ZNVDPBF` montre encore wilkinson=« Alomairy », zhang=« Wu, Y. »,
molder=10 auteurs — les corrections annoncées par les lots V1/V2 n'ont **pas pris**
côté Zotero (le `.bib`, lui, est correct). Issues #1/#3/#4 + collection dédiée
`755N3T92` + dédup `rocklin…2015a` → reprises dans une molécule de finalisation Zotero.

**Verdict corrigé : le papier est PASS** (références exactes, 0 citation indéfinie).
Reste de la plomberie **Zotero** (hygiène bibliothèque), hors chemin de rendu du papier.

### ⚠️ Findings C/D (entrées « mortes » / orphelines) — sur-comptées, rectifiées

Le grep du setup matchait `\cite{clé}` mais **pas** la forme à locator
`\cite[\S3--4]{clé}` (argument optionnel). Toutes les citations à locator ont donc
été comptées « mortes » à tort. Recompte correct (locators + multi-clé, hors `\iffalse`) :
**5 des 9 « mortes » sont en réalité CITÉES** — `newcombe`, `peng`, `stodden`,
`mokhovBuildSystemsCarte2018`, `mokhovBuildSystemsCarte2020`. Il ne reste que **4**
entrées réellement non citées : `arrowRust` (aucun Apache Arrow dans le texte →
**élaguée** de `references.bib`), `souzaPROVAGENTUnifiedProvenance2025` (pair avec le
bloc agentic en quarantaine → conservée pour une future version SE-venue),
`benetIPFSContentAddressed2014` et `mitchellFAIRDataPipeline2022` (conservées ;
candidates optionnelles à citer — IPFS ancrerait le content-addressing). Finding D
(« 5 bibitems orphelins ») tombe du même biais : ce sont les mêmes clés à locator,
bien citées. Aucune action de masse ; `newcombe` n'avait pas besoin d'être re-cité.
