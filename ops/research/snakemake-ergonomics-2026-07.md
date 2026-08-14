# Ergonomie et intégration : Snakemake et les moteurs de workflow à DSL Python — enquête 2026-07-25

Recherche documentaire uniquement. **Aucune modification du papier.** Le but est
d'établir ce qui est *sourcé* sur les axes ergonomie / intégration, par
opposition aux critères fonctionnels (cache, provenance, scaling) déjà couverts
ailleurs.

## Conclusion courte

Trois des quatre axes ont un socle documentaire solide et citable ; le
quatrième n'en a pas.

- **Axe 1 (le Snakefile est un quasi-Python, pas un format de données)** — le
  mieux documenté. Preuve primaire directe : le mainteneur de `snakefmt` écrit
  publiquement que son parseur est bricolé et devient ingérable, et l'équipe
  Ruff refuse d'ajouter le dialecte. Preuve empirique complémentaire : l'étude
  SSDBM 2024 sur 1 602 dépôts a dû analyser les Snakefiles **par recherche de
  mots-clés ligne à ligne**, pas par AST.
- **Axe 2 (coût de déploiement d'un runtime Python vs binaire)** — documenté,
  mais **par assemblage** : la doctrine du lien statique en calcul scientifique
  est argumentée et publique ; le coût Python est attesté par la documentation
  officielle de Snakemake elle-même (conda/mamba recommandé) et par des tickets
  ; il n'existe pas, à ma connaissance, de mesure publiée comparant le temps de
  démarrage ou l'empreinte de Snakemake à un moteur compilé.
- **Axe 3 (formats déclaratifs standards vs DSL programmatique)** — bien
  documenté **des deux côtés**, avec une littérature évaluée par les pairs pour
  le déclaratif (CWL/CACM) et des critiques nettes du déclaratif (verbosité
  CWL, « coder en assembleur ») et du non-déclaratif (Borretti). C'est l'axe le
  plus équilibré, donc le plus sûr à citer.
- **Axe 4 (comparaisons explicites binaire compilé vs Python sur ces axes)** —
  **pas de source solide.** Aucun travail évalué par les pairs, aucun billet
  d'ingénieur identifiable ne compare un moteur de workflow Rust/Go à Snakemake
  sur l'ergonomie ou l'intégration. Les analogues (Buck2, Sprocket) sont
  utilisables mais ne sont pas des comparaisons directes.

Convention de marquage, reprise de `use-cases-2026-07.md` :

- **documenté** : source primaire ou discussion technique identifiable ;
- **opinion isolée** : un avis unique, pas un consensus ;
- **folklore** : répété sans source traçable — à ne pas citer.

---

## Axe 1 — Le Snakefile est une syntaxe quasi-Python, pas un format de données

### 1.1 Le fait de base, énoncé par le projet lui-même

**documenté.** Snakemake ne cache pas que le Snakefile *est* du Python exécuté.

> « the DSL is implemented as an extension to a generic programming language
> (Groovy and Python), access to the full power of the underlying programming
> language is maintained »
> — Mölder et al., *Sustainable data analysis with Snakemake*, F1000Research
> 10:33 (2021, v3 en 2025), DOI `10.12688/f1000research.29032.2`,
> <https://pmc.ncbi.nlm.nih.gov/articles/PMC8114187/> — article évalué par les
> pairs, auteurs = concepteurs de l'outil.

Et le choix est revendiqué comme une qualité :

> « The workflow definition language of Snakemake is designed to allow maximum
> readability, which is crucial for transparency and adaptability. »
> — *ibid.*

Le matériel pédagogique officiel (The Carpentries) l'énonce sans ambiguïté :
la leçon s'intitule littéralement « Snakefiles are Python code », et insiste
sur le fait que *tout* le Snakefile est exécuté à chaque invocation.

> « The entire Snakefile is executed whenever you run snakemake. »
> — *Getting Started with Snakemake — Snakefiles are Python code*,
> <https://carpentries-incubator.github.io/workflows-snakemake/05-snakemake-python/index.html>
> — support de cours communautaire (Carpentries Incubator), non évalué par les
> pairs mais officiel et stable.

Conséquence factuelle, non polémique : **charger un workflow, c'est exécuter du
code arbitraire.** Ce n'est pas une faille, c'est la sémantique documentée. Mais
cela signifie qu'aucun outil tiers ne peut *lire* un workflow sans l'exécuter.

> ⚠️ **Je n'ai trouvé aucune documentation de sécurité Snakemake traitant
> explicitement de ce point** (modèle de menace, exécution de workflows non
> fiables). L'absence est notable ; ne pas la présenter comme une négligence,
> seulement comme un point non documenté.

### 1.2 La difficulté de parsing par des outils tiers — la meilleure preuve

**documenté, source primaire, mainteneur identifié.**

Michael Hall (`mbhall88`), co-mainteneur de `snakefmt` (le formateur officiel du
projet Snakemake), ouvre le 14 août 2024 une discussion chez Astral (Ruff) pour
demander d'étendre le parseur Python de Ruff à la syntaxe Snakemake :

> « essentially just passed the code to black, converting snakemake-specific
> syntax into python analogs, and then converting back after the formatting. »

> « We currently have lots of hacky fixes which are building up into a
> headache. »

— astral-sh/ruff, Discussion #12882, 2024-08-14,
<https://github.com/astral-sh/ruff/discussions/12882> — discussion GitHub
publique, auteur = mainteneur de l'outil concerné.

La réponse de Micha Reiser (mainteneur Ruff), même jour :

> « I don't think snakemake has enough community today to warrant the added
> complexity on our side. »

> « Forking a parser comes with its own challenge if you want to stay up to
> date with new Python syntax. »

C'est la formulation la plus nette du coût d'intégration : **le dialecte est
assez proche de Python pour donner envie de réutiliser l'outillage Python, et
assez éloigné pour que cet outillage refuse de le supporter.** Les deux camps
le disent, publiquement, sans acrimonie.

Symétriquement côté IDE, la demande de meilleur support est ouverte depuis 2019
et non résolue :

> « parsing snakemake file is not very good »
> — microsoft/vscode-python, issue #6438, ouverte le 2019-07-03,
> <https://github.com/microsoft/vscode-python/issues/6438> — issue GitHub
> publique. Source plus faible (rapport d'utilisateur, pas de réponse
> mainteneur citable) ; à utiliser en appui, pas en pointe.

### 1.3 Preuve empirique : les chercheurs eux-mêmes analysent les Snakefiles par grep

**documenté, évalué par les pairs — c'est la pièce la plus forte du dossier.**

Pohl, Elfaramawy, Cao, Kehr, Weidlich, *How do users design scientific
workflows? The Case of Snakemake*, arXiv:2309.14097 (2023-09-25) ; version
étendue à Nextflow publiée à SSDBM 2024, DOI `10.1145/3676288.3676290`.

Méthode déclarée pour l'analyse des fonctionnalités de langage sur 1 602 dépôts
GitHub :

> « we queried GitHub, collected the source code of Snakemake workflows, and
> parsed the code **line by line to search for specific key words**. »

> « This part of our analysis exploits the data for **1431 of the 1602
> repositories**, i.e., all repositories for which the snakefile could be
> queried directly at GitHub »

> « For **3436 out of the 3550** encountered include statements, such a
> resolution was possible »

Autrement dit : une équipe de recherche en bases de données, pour étudier des
workflows Snakemake, recourt à la recherche textuelle et à une résolution
approximative des `include`. Ce n'est pas une critique du projet — c'est une
mesure de ce que coûte l'analyse statique d'un DSL semi-programmatique.

Chiffres de la même étude, utiles pour l'axe 3 (voir §3.4) :

| Fait mesuré | Valeur | Base |
|---|---|---|
| Dépôts avec fichiers de configuration non vides | 712 / 1431 (≈ 50 %) | 1 303 fichiers de config |
| Taille médiane d'un fichier de config | 32 lignes (moy. 64, P75 71) | 1 303 fichiers |
| Workflows dont des opérateurs shell sont pilotés par la config | 236 / 828 (29 %) | appariement par motif |
| Workflows contenant du contrôle de flux shell (`if`, `for`) | 141 / 828 | Table 3 |
| Dépôts utilisant des *input functions* (Python) | 42 / 1431 (68 occurrences) | — |
| Dépôts utilisant des *checkpoints* | 94 / 1431 (251 occurrences) | — |
| Dépôts utilisant l'instruction `notebook` | 23 / 1431 (47 occurrences) | — |

Lecture honnête de ce tableau : **le recours au Python arbitraire est réel mais
minoritaire** (input functions : 3 % des dépôts), tandis que **la configuration
déclarative est massivement utilisée** (50 % des dépôts). C'est un argument
*pour* le format déclaratif, pas *contre* Snakemake : les utilisateurs
externalisent déjà spontanément le paramétrage dans des fichiers de données.

### 1.4 Génération par un programme ou un agent

**faible / partiellement documenté.**

Alam & Roy, *From Prompt to Pipeline: Large Language Models for Scientific
Workflow Development in Bioinformatics*, arXiv:2507.20122 (v2, 2025-08-18),
évaluent GPT-4o, Gemini 2.5 Flash et DeepSeek-V3 sur **Galaxy et Nextflow**.

> Le papier n'évalue **ni Snakemake ni CWL**, ne publie **pas de taux de succès
> chiffrés**, et **ne formule aucune affirmation sur la difficulté propre à la
> syntaxe DSL** pour un modèle. Les défauts recensés sont fonctionnels
> (sélection d'outils incomplète, étapes de prétraitement manquantes,
> normalisation des identifiants de chromosomes, gestion insuffisante de la
> conteneurisation).

**À ne pas citer comme preuve que les DSL sont durs à générer.** Le seul usage
défendable est négatif : *à notre connaissance, aucune étude publiée ne mesure
l'effet de la nature du langage de workflow sur la fiabilité de sa génération
automatique.*

Côté génération programmatique (non-LLM), la seule source utile est la
discussion CWL de §3.3 : elle établit que **générer un workflow est un cas
d'usage réel et anticipé**, pas une lubie d'agent.

---

## Axe 2 — Coût de déploiement d'un runtime Python vs un binaire statique

### 2.1 Ce que dit Snakemake

**documenté.** La documentation officielle recommande conda/mamba plutôt que
pip, précisément à cause des dépendances non-Python :

> « Snakemake can be installed with pip, however, it has non-python
> dependencies that require manual installation for full functionality. »

> « The recommended way to install Snakemake is via conda/mamba because it
> enables Snakemake to handle software dependencies of your workflow. »

— *Installation*, documentation Snakemake,
<https://snakemake.readthedocs.io/en/stable/getting_started/installation.html>
(constant de la v6 à la v9) — documentation officielle.

Il faut être juste : **le projet revendique aussi la portabilité de ce choix.**
La page *Distribution and Reproducibility* affirme qu'un Snakefile ne demande
qu'une installation Python. Les deux affirmations coexistent : le *moteur* est
portable partout où Python tourne ; c'est l'installation *complète et
fonctionnelle* qui passe par un gestionnaire d'environnement.

> ⚠️ Je n'ai pas pu récupérer le texte exact de la page *deployment.html*
> (HTTP 429 au moment de l'enquête). **Vérifier la citation avant tout usage
> dans le papier.**
> <https://snakemake.readthedocs.io/en/stable/snakefiles/deployment.html>

Trace de friction concrète, faible mais réelle :

> « AttributeError: 'str' object has no attribute 'name' » sur deux machines,
> Snakemake 7.8.5, Python 3.9 et 3.10 — l'utilisateur demande que chaque
> release publie un environnement conda exporté pour identifier les versions
> de dépendances fautives.
> — snakemake/snakemake issue #1899, 2022-10-08,
> <https://github.com/snakemake/snakemake/issues/1899> — **opinion isolée**,
> pas de réponse mainteneur visible. Anecdote, pas donnée. Ne pas citer seul.

### 2.2 L'argument du lien statique en calcul scientifique

**documenté, ingénieur identifiable, argumenté contradictoirement.**

Roman Cheplyaka, *A case for static linking in scientific computing*,
2016-09-09,
<https://ro-che.info/articles/2016-09-09-static-binaries-scientific-computing>.

L'argument central, transposable ligne à ligne au débat runtime/binaire :

> « When a program is linked statically, it executes the same algorithm
> wherever it is run, whereas a dynamic executable executes the code from the
> version of the dynamic library that happens to be installed on a particular
> computing node. »

Qualité de la source : l'auteur **cite et discute la contradiction** (Ulrich
Drepper, mainteneur glibc, hostile au lien statique) et concède des limites
(pas de correctifs de sécurité centralisés, certaines applications difficiles à
lier statiquement). C'est un billet argumenté, pas un plaidoyer.

Contrepoint utile : sur HPC, la taille des binaires est jugée négligeable
devant la taille des données — ce qui désamorce l'objection classique du
gonflement.

### 2.3 Temps de démarrage d'un interpréteur Python

**documenté comme phénomène général, non mesuré pour Snakemake.**

Le coût de démarrage d'un CLI Python est un problème connu et instrumenté :
Python 3.7+ fournit `-X importtime` / `PYTHONPROFILEIMPORTTIME` exactement pour
cela (documentation CPython ; voir aussi les notes de Victor Stinner,
<https://pythondev.readthedocs.io/startup_time.html>). Le constat usuel — le
démarrage devient perceptible quand l'outil est invoqué souvent et importe des
bibliothèques lourdes — est **documenté pour les CLI Python en général**.

> ⚠️ **Aucune mesure publiée du temps de démarrage de Snakemake n'a été
> trouvée.** Ne pas extrapoler. Si le papier veut un chiffre, il faut le
> mesurer soi-même et le présenter comme une mesure propre, avec méthode et
> matériel.

### 2.4 Conteneurisation

**documenté comme pratique, pas comme coût chiffré.** Bioconda /
BioContainers et l'intégration conda/Apptainer des moteurs (Snakemake,
Nextflow, Galaxy) sont l'état de l'art documenté. Je n'ai trouvé **aucune
mesure publiée et vérifiable** de taille d'image ou de temps de démarrage
comparant un moteur Python conteneurisé à un binaire statique. Tout chiffre en
circulation sur ce point relève du **folklore** tant qu'il n'est pas remesuré.

---

## Axe 3 — Formats déclaratifs standards (TOML/YAML/JSON) : le pour et le contre

C'est l'axe le mieux équilibré, donc le plus sûr pour un papier qui refuse le
dénigrement : **les deux positions ont des défenseurs sérieux et publiés.**

### 3.1 Pour : la thèse CWL, évaluée par les pairs

**documenté, peer-reviewed.**

Crusoe, Abeln, Iosup, Amstutz, Chilton, Tijanić, Ménager, Soiland-Reyes,
Gavrilović, Goble, *Methods Included: Standardizing Computational Reuse and
Portability with the Common Workflow Language*, CACM 65(6), 2022,
DOI `10.1145/3486897` ; préprint arXiv:2105.07028.

> « many competing workflow systems exist, severely limiting portability of
> such workflows, thereby hindering the transfer of workflows between different
> systems, between different projects and different settings, leading to vendor
> lock-ins and limiting their generic re-usability. »

> « The CWL standards provide a common but reduced set of abstractions that are
> both used in practice and implemented in many popular workflow systems. The
> CWL language is declarative »

Le mot qui porte tout l'argument est **`reduced`** : le pouvoir expressif est
volontairement amputé pour que plusieurs moteurs indépendants puissent
implémenter le même standard. C'est exactement la thèse « format de données
plutôt que programme », énoncée par ses auteurs, dans une revue à comité de
lecture.

> ⚠️ La version CACM est derrière un 403 ; les citations ci-dessus proviennent
> du préprint arXiv (2021-05-14, rév. 2021-08-04). Vérifier contre la version
> publiée avant citation dans le papier.

### 3.2 Pour : YAML défendu par des ingénieurs identifiables

**documenté, récent.** Rich Iannone et Tomasz Kalinowski (Posit), *In Defense
of YAML*, 2026-05-21,
<https://opensource.posit.co/blog/2026-05-21_in-defense-of-yaml/>.

> « The YAML that people complain about is YAML 1.1. The specification that
> actually governs the language today is a different, safer, more predictable
> document. »

Point important et nuancé pour un papier qui utiliserait TOML : les auteurs
**ne prétendent pas** que YAML domine partout — « for shallow structures, TOML's
explicitness excels ». Cette phrase est directement utilisable pour justifier
TOML sans caricaturer YAML.

### 3.3 Contre : le déclaratif pur devient de l'assembleur

**documenté, discussion de communauté, participants nommés.**

Fil *Generators of CWL workflows*, liste `common-workflow-language`,
janvier 2018 (3–23 janvier), participants dont Andrey Tovchigrechko, Lourens
Veen, Jeff Gentry —
<https://groups.google.com/g/common-workflow-language/c/VGxvXPOQrWY>.

> « CWL has been initially conceived as a kind of workflow exchange language.
> Something that can be passed to different engines, and easy to write parsers
> for. »

> « It was not assumed to be easily writable by humans. Like a kind of a
> universal assembler language that could be executed by different CPU models. »

> « In reality, the generators were never a focus of development, and the
> enthusiasts were left with writing CWL by hand ("coding in assembler"). »

> « pipeline developers come to try it, get horrified by the verbosity compared
> to, say, NextFlow, and abandon the idea. »

**C'est le contre-argument le plus fort du dossier, et il vise directement la
position d'OxyMake.** Un format déclaratif gagne la parsabilité et perd
l'écriture à la main. La réponse honnête n'est pas de le nier mais de le
nommer : *le format est fait pour être écrit par un humain qui accepte la
verbosité, ou généré par un outil.*

### 3.4 Contre : « Some Data Should Be Code »

**documenté, ingénieur identifiable.** Fernando Borretti, *Some Data Should Be
Code*, <https://borretti.me/article/some-data-should-be-code> (≈ début 2026 ;
discuté sur Lobsters, <https://lobste.rs/s/6n7rzd/some_data_should_be_code>).

Thèse : les formats de configuration (YAML de GitHub Actions, Makefiles)
dérivent par accrétion vers des langages de programmation de fait, et il vaut
mieux l'assumer.

> « If I'm building software (and a build system is software) I'd almost always
> prefer doing so in a carefully designed, stable, general purpose language. »

Contre-arguments recensés dans la discussion, à citer pour l'équilibre :

- générer la configuration déplace les bugs dans le générateur et « makes
  changes harder to mentally model » ;
- de bonnes abstractions de domaine évitent de réimplémenter des graphes de
  dépendances à répétition ;
- **Starlark (Bazel/Buck) est le compromis explicite** : syntaxe de langage,
  Turing-incomplétude assumée « to avoid unbounded recursion/iteration ».

Argument complémentaire, formulé dans la même discussion : YAML « can't factor
repeated parts into functions (anchors and reusable-workflow indirection are a
pale imitation), so CI pipelines drift into copy-paste ». **C'est la faiblesse
réelle et documentée du choix déclaratif ; elle doit apparaître dans le papier
si l'axe est traité.**

### 3.5 Position empirique : les utilisateurs Snakemake externalisent déjà

**documenté** (voir §1.3) : 712 dépôts sur 1 431 embarquent un fichier de
configuration ; 29 % des workflows analysés pilotent leurs commandes shell
depuis ce fichier. Les *input functions* (le Python vraiment arbitraire) ne
touchent que 42 dépôts sur 1 431.

Formulation défendable et non polémique : *dans la pratique observée, la
majeure partie de la variabilité d'un workflow est déjà exprimée en données ;
la partie irréductiblement programmatique est minoritaire.*

---

## Axe 4 — Comparaisons explicites moteur compilé (Rust/Go) vs moteur Python

**Verdict : aucun travail ni billet ne fait cette comparaison sur les axes
ergonomie/intégration.** C'est le trou du dossier. Ce qui existe est
analogique, et doit être présenté comme tel.

### 4.1 Buck2 — le meilleur analogue, source officielle

**documenté, ingénierie identifiable (Meta / Neil Mitchell et al.).**

- *Build faster with Buck2: Our open source build system*, Engineering at Meta,
  2023-04-06,
  <https://engineering.fb.com/2023/04/06/open-source/buck2-open-source-large-scale-build-system/>
- *Why Buck2*, documentation officielle, <https://buck2.build/docs/about/why/>

Rationale du cœur en Rust, avec les alternatives explicitement pesées (Java
comme Buck1, Haskell comme Shake, Go) :

> « One of the advantages of using Rust is the absence of GC pauses »

Meta reconnaît le contre-argument : Java offrait « better memory profiling
tools ». Résultat mesuré, interne et donc à citer avec précaution :

> « in our internal tests, we observed that Buck2 completed builds 2x as fast
> as Buck1. »

**Le point le plus intéressant pour OxyMake n'est pas la performance, c'est
l'architecture** — cœur compilé + couche de configuration dans un langage
restreint :

> « the Buck2 binary is entirely language agnostic »

> « the most important and complex rule (such as in C++), don't have access to
> magic internal features »

C'est un précédent documenté et de grande échelle pour la séparation
*moteur compilé / description en langage restreint*. Il valide la forme, pas
les chiffres.

### 4.2 Sprocket — un moteur de workflow bio-informatique en Rust

**documenté, projet réel, mais sans argumentaire comparatif publié.**

St. Jude Rust Labs, *Sprocket*,
<https://github.com/stjude-rust-labs/sprocket> — « a bioinformatics workflow
engine built on top of the Workflow Description Language (WDL) », écrit en
Rust. Objectifs affichés : « a high-performance workflow execution engine
capable of orchestrating massive bioinformatics workloads » (cible 20 000+
jobs concurrents) et « a suite of modern development tools ».

Deux observations honnêtes :

1. **Le sous-commandes trahissent le bénéfice du format non-programmatique** :
   `check`, `lint`, `format`, `validate`, `inputs`, plus un serveur LSP. Ces
   outils sont faciles à construire **parce que WDL est un langage dédié et
   analysable statiquement**, pas parce que le moteur est en Rust. C'est
   l'argument de l'axe 1, illustré positivement.
2. **Le binaire compilé ne supprime pas tous les problèmes de déploiement** :
   « the prebuilt Sprocket for Linux may not work on every distribution due to
   library dependencies ». À citer si le papier prétend que le binaire résout
   la portabilité — la nuance est dans le README du projet lui-même.

### 4.3 Ce qui n'existe pas

- Aucune étude évaluée par les pairs comparant un moteur de workflow compilé à
  un moteur Python sur l'ergonomie, l'intégration ou le déploiement.
- Aucun benchmark public de démarrage / empreinte mémoire Snakemake vs moteur
  compilé.
- Les billets « Rust vs Python » génériques trouvés (dev.to, tech-insider,
  Medium) sont **du contenu marketing ou de blog non attribuable, sans
  méthode** : chiffres du type « 25-100x », « 50x more throughput », sans
  protocole. **Folklore. Ne rien en citer.**

### 4.4 Contexte de marché, pour information

L'enquête *State of the Workflow 2024* de Seqera (608 utilisateurs Nextflow,
48 pays), rapportée dans Genome Biology 2025
(<https://genomebiology.biomedcentral.com/articles/10.1186/s13059-025-03673-9>),
indique une baisse de la part de Snakemake de 27 % (2021) à 17 % (2024).

> ⚠️ **Source structurellement biaisée** : enquête menée par l'éditeur de
> Nextflow auprès de ses propres utilisateurs. La reprise dans Genome Biology
> ne corrige pas le biais d'échantillonnage. **Ne pas utiliser dans le papier
> pour caractériser l'adoption de Snakemake.**

### 4.5 Sur la littérature d'évaluation d'ergonomie

Loach, Smith, Bacon, *A scoping review of approaches to evaluating workflow
management systems for bioinformatics users*, Briefings in Bioinformatics
27(4), 2026, DOI `10.1093/bib/bbag396`.

> 21 articles retenus sur 7 ans, dont **seulement 4** portant sur les besoins
> des utilisateurs non-informaticiens.

Constat central, utile parce qu'il *nuance* toute affirmation ergonomique :

> « Papers focused on developers…considered a text-based interface to be more
> usable while those considering noncomputational users…preferred a GUI. »

> « Usability (as 'Ease of Use') was scored highest for GUIs, while these
> interfaces were given the lowest scores for Flexibility. »

Lecture pour le papier : **l'ergonomie d'un moteur de workflow n'a pas de
mesure absolue ; elle dépend du public visé.** Toute affirmation ergonomique
doit nommer son utilisateur cible. C'est aussi la meilleure protection contre
l'accusation de dénigrement.

---

## Ce qui est utilisable dans le papier, et ce qui ne l'est pas

Contrat de style rappelé : factuel, non exagéré, pas de dénigrement d'un projet
voisin ; toute critique sourcée et équilibrée.

### ✅ Utilisable tel quel — solide, sourcé, non polémique

| Affirmation | Source | Pourquoi c'est sûr |
|---|---|---|
| Un Snakefile est exécuté comme du Python à chaque invocation ; le langage est une extension d'un langage généraliste | Mölder et al. 2021 (peer-reviewed, auteurs de l'outil) + Carpentries | Énoncé *par le projet lui-même*, revendiqué comme une qualité. Aucun risque de dénigrement. |
| L'outillage Python tiers ne parse pas nativement ce dialecte ; le formateur officiel maintient un parseur de contournement, et Ruff a décliné le support | Ruff Discussion #12882 (2024) | Les deux mainteneurs concernés le disent publiquement, sans animosité. Citer les deux répliques ensemble. |
| Étudier des workflows Snakemake à grande échelle se fait par recherche de mots-clés ligne à ligne, avec une couverture partielle (1431/1602 dépôts ; 3436/3550 `include`) | Pohl et al., arXiv:2309.14097 / SSDBM 2024 | Fait méthodologique publié, pas un jugement. Le plus fort du dossier. |
| Dans la pratique observée, la configuration déclarative est très répandue (712/1431 dépôts) et le Python arbitraire minoritaire (input functions : 42/1431) | *ibid.* | Chiffré, vérifiable, et **flatteur** pour les utilisateurs : ils font déjà ce que le format déclaratif propose. |
| Un standard déclaratif échange du pouvoir expressif contre la portabilité entre moteurs (« a common but reduced set of abstractions ») | Crusoe et al., CACM 2022 | Formulé par les auteurs du standard. Pose le compromis sans le trancher. |
| Le déclaratif pur a un coût d'écriture à la main réel et reconnu par sa propre communauté | Fil CWL 2018 | **À inclure obligatoirement** pour l'équilibre : montre qu'on connaît le prix de notre propre choix. |
| Un cœur compilé avec la description dans un langage restreint est une architecture éprouvée à grande échelle | Buck2 (*Why Buck2*, blog Meta 2023) | Documentation officielle, précédent industriel majeur. |
| Le lien statique améliore la reproductibilité en calcul scientifique | Cheplyaka 2016 | Argumenté, contradiction discutée, limites concédées. |
| L'ergonomie d'un moteur dépend du public visé (développeurs ≠ non-informaticiens) | Loach et al., Brief Bioinform 2026 | Revue de portée. Protège toute affirmation ergonomique du papier. |

### ⚠️ Utilisable avec précaution — vérifier ou requalifier avant citation

- **La page Snakemake *deployment.html*** — non récupérée (HTTP 429). Vérifier
  le texte exact avant de citer la revendication de portabilité.
- **Les citations CACM** — issues du préprint arXiv:2105.07028, pas de la
  version publiée. Recouper.
- **Buck2 « 2x as fast as Buck1 »** — test *interne* Meta, non reproductible.
  Si cité, dire « selon les tests internes de Meta ».
- **Le coût de démarrage d'un interpréteur Python** — documenté en général,
  **jamais mesuré pour Snakemake**. Si le papier veut un chiffre, le mesurer
  soi-même et le présenter comme mesure propre, avec méthode et matériel.
- **Le caveat Sprocket** (binaire prébuilt Linux dépendant des bibliothèques
  système) — à citer *si et seulement si* le papier affirme que le binaire
  résout la portabilité. C'est une auto-limitation honnête, pas une attaque.

### ❌ À ne pas utiliser

- **« Les DSL Python sont durs à générer pour un LLM »** — Alam & Roy
  n'évaluent pas Snakemake, ne chiffrent pas, et ne formulent pas cette
  affirmation. Aucune source ne l'établit. Formulation de repli acceptable :
  *à notre connaissance, aucune étude publiée ne mesure cet effet.*
- **« Snakemake perd des parts »** (27 % → 17 %) — enquête Seqera, éditeur d'un
  concurrent direct, auprès de ses propres utilisateurs. Biais structurel.
- **Tout chiffre « Rust est 25–100x plus rapide que Python »** ou « 50x more
  throughput » — blogs sans méthode. Folklore.
- **Toute mesure de taille d'image conteneur ou de temps de démarrage
  comparant Snakemake à un binaire** — rien de publié et vérifiable n'existe.
- **L'issue Snakemake #1899** seule — anecdote d'un utilisateur, sans réponse
  mainteneur. Inutilisable comme preuve d'un problème systémique.
- **Un argument de sécurité sur l'exécution de code arbitraire au chargement** —
  le *fait* est documenté, mais aucune source ne le qualifie de problème de
  sécurité. En faire un argument serait une extrapolation hostile.

### Note de cadrage pour la rédaction

Le dossier soutient une formulation **positive et symétrique**, pas une
critique. Forme suggérée : *les moteurs à DSL semi-programmatique optimisent
l'expressivité pour l'auteur humain ; les formats déclaratifs optimisent
l'analysabilité pour les outils tiers ; les deux compromis sont documentés et
assumés par leurs communautés respectives, y compris dans leurs coûts (§3.3
pour le nôtre).* OxyMake se place sur le second versant, et le dit en citant le
prix qu'il paie.

---

## Inventaire des sources

| # | Source | Type | Date | URL |
|---|---|---|---|---|
| S1 | Mölder et al., *Sustainable data analysis with Snakemake*, F1000Res 10:33 | peer-reviewed | 2021 (v3 2025) | <https://pmc.ncbi.nlm.nih.gov/articles/PMC8114187/> |
| S2 | Pohl et al., *How do users design scientific workflows? The Case of Snakemake*, arXiv:2309.14097 ; SSDBM 2024 | peer-reviewed | 2023-09-25 / 2024 | <https://arxiv.org/abs/2309.14097> |
| S3 | Ruff Discussion #12882 (mbhall88 / MichaReiser) | discussion mainteneurs | 2024-08-14 | <https://github.com/astral-sh/ruff/discussions/12882> |
| S4 | vscode-python issue #6438 | issue GitHub | 2019-07-03 | <https://github.com/microsoft/vscode-python/issues/6438> |
| S5 | Carpentries Incubator, *Snakefiles are Python code* | support officiel | — | <https://carpentries-incubator.github.io/workflows-snakemake/05-snakemake-python/index.html> |
| S6 | Snakemake, *Installation* | doc officielle | v6→v9 | <https://snakemake.readthedocs.io/en/stable/getting_started/installation.html> |
| S7 | Snakemake, *Distribution and Reproducibility* | doc officielle | — ⚠️ non récupérée | <https://snakemake.readthedocs.io/en/stable/snakefiles/deployment.html> |
| S8 | snakemake issue #1899 | issue GitHub | 2022-10-08 | <https://github.com/snakemake/snakemake/issues/1899> |
| S9 | Cheplyaka, *A case for static linking in scientific computing* | billet argumenté | 2016-09-09 | <https://ro-che.info/articles/2016-09-09-static-binaries-scientific-computing> |
| S10 | Crusoe et al., *Methods Included*, CACM 65(6) / arXiv:2105.07028 | peer-reviewed | 2022 / 2021 | <https://arxiv.org/abs/2105.07028> |
| S11 | Fil *Generators of CWL workflows*, google-groups | discussion communauté | 2018-01 | <https://groups.google.com/g/common-workflow-language/c/VGxvXPOQrWY> |
| S12 | Borretti, *Some Data Should Be Code* (+ fil Lobsters) | billet ingénieur | ≈2026 | <https://borretti.me/article/some-data-should-be-code> |
| S13 | Iannone & Kalinowski (Posit), *In Defense of YAML* | billet ingénieurs | 2026-05-21 | <https://opensource.posit.co/blog/2026-05-21_in-defense-of-yaml/> |
| S14 | Meta, *Build faster with Buck2* | blog ingénierie officiel | 2023-04-06 | <https://engineering.fb.com/2023/04/06/open-source/buck2-open-source-large-scale-build-system/> |
| S15 | *Why Buck2* | doc officielle | — | <https://buck2.build/docs/about/why/> |
| S16 | St. Jude Rust Labs, *Sprocket* | dépôt officiel | — | <https://github.com/stjude-rust-labs/sprocket> |
| S17 | Loach, Smith, Bacon, Brief Bioinform 27(4) | peer-reviewed | 2026 | <https://doi.org/10.1093/bib/bbag396> |
| S18 | Alam & Roy, arXiv:2507.20122 | préprint | 2025-08-18 | <https://arxiv.org/abs/2507.20122> |
| S19 | Genome Biology 2025 (enquête Seqera) | peer-reviewed ⚠️ biaisé | 2025 | <https://genomebiology.biomedcentral.com/articles/10.1186/s13059-025-03673-9> |
| S20 | Stinner, *Python Startup Time* | notes développeur CPython | — | <https://pythondev.readthedocs.io/startup_time.html> |
