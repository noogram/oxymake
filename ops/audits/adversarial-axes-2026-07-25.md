# Audit adversarial non fonctionnel d’OxyMake — 25 juillet 2026

**Auteur de l’audit : Noogram.**  Cet audit attaque les affirmations et les
choix du projet, pas l’intention de ses auteurs. Il examine le dépôt et la
version v3 du papier au 25 juillet 2026. Les verdicts signifient :

- **FONDÉE** : les prémisses nécessaires sont établies par le dépôt ou une
  source primaire ;
- **PARTIELLEMENT FONDÉE** : l’objection est réelle mais déjà bornée, ou ne
  vaut que sous une condition explicite ;
- **INFONDÉE** : les preuves disponibles réfutent l’attaque telle que formulée.

Les numéros de ligne renvoient à l’état du dépôt audité. Les sources externes
sont exclusivement des documentations officielles ou l’API du dépôt. Une
réponse « minimale » n’est pas un plan produit : c’est le plus petit geste qui
évite de survendre — borner, mesurer ou concéder.

## Verdict d’ensemble

Le noyau défendable est plus étroit que le récit produit : **OxyMake est un
moteur pré-1.0 intéressant pour des DAG statiques, déterministes,
fichier-à-fichier, exécutés par un opérateur qui fait confiance au workflow et
maîtrise entièrement ses entrées**. Le dépôt établit bien la vitesse de
résolution d’un DAG synthétique sur une machine et documente plusieurs limites
avec une franchise inhabituelle. Il n’établit pas encore l’ergonomie en usage,
la portabilité sémantique, l’exploitation sur cluster, la sécurité en contexte
multi-utilisateur, ni l’adéquation à des workflows scientifiques réels.

Les attaques les plus dommageables sont les nos 2, 4, 7, 9, 12, 15 et 16. Deux
angles morts paraissent absents du papier : l’injection par interpolation non
quotée (no 9) et la chaîne de confiance du dashboard (no 10).

## Attaques

### 1. « TOML ne supprime pas la complexité : il la déplace hors du workflow »

**Attaque.** Un format inerte simplifie le parseur, pas nécessairement le travail
de l’utilisateur. Dès que la configuration dépend d’un inventaire, d’une API,
d’un contenu produit en amont ou d’une logique métier, l’utilisateur doit
générer du TOML, maintenir un script auxiliaire, puis synchroniser deux surfaces.
La promesse de simplicité devient une architecture à deux langages dont OxyMake
ne suit pas automatiquement la dépendance.

**Verdict : FONDÉE.** ADR-002 concède que la configuration complexe doit être
générée hors de l’Oxymakefile
([`docs/adr/002-toml-not-dsl.md`, l. 26–31](../../docs/adr/002-toml-not-dsl.md)).
Le papier propose explicitement `python gen_config.py > config.toml`
([`docs/paper/oxymake-paper.tex`, l. 2386–2401](../../docs/paper/oxymake-paper.tex)).
Or le modèle de menace précise qu’un helper lu sans déclaration n’entre pas dans
la clé ([même fichier, l. 844–867](../../docs/paper/oxymake-paper.tex)). La sortie
générée peut être déclarée ; le générateur et ses dépendances ne le sont pas par
construction.

**Réponse honnête minimale.** Remplacer « TOML réduit la complexité » par « TOML
rend statiquement inspectable la partie déclarative ». Documenter un patron où
le générateur et ses sources sont des entrées explicites, puis mesurer le nombre
de fichiers et de lignes nécessaires sur trois workflows réels.

### 2. « “Inspectable sans exécuter” confond structure du DAG et comportement »

**Attaque.** Un lecteur ne comprend pas ce que fait un workflow en inspectant un
TOML qui contient du shell opaque, des scripts externes et des fonctions Python.
Il comprend au mieux les arêtes déclarées. L’affirmation du README — « never have
to execute it to understand what it does » — est donc trop forte.

**Verdict : FONDÉE.** Le README porte cette affirmation
([`README.md`, l. 19–21](../../README.md)), tandis que le papier classe lui-même
`shell` comme « opaque » ([`docs/paper/oxymake-paper.tex`, l. 1226–1242](../../docs/paper/oxymake-paper.tex))
et admet que le moteur n’impose ni absence d’effets de bord ni complétude des
entrées ([même fichier, l. 1246–1254](../../docs/paper/oxymake-paper.tex)). TOML
rend le graphe déclaré lisible ; il ne rend pas les commandes, les dépendances
dynamiques ou les effets externes compréhensibles.

**Réponse honnête minimale.** Dire : « tout outil peut inspecter le graphe et les
commandes déclarés sans exécuter la définition ». Ne pas revendiquer la
compréhension du comportement.

### 3. « Le gain d’analyse statique se paie par la perte des DAG dépendants des données »

**Attaque.** Les entrées fonctions, checkpoints, branchements sur le contenu et
sorties de cardinalité inconnue sont des besoins, pas des abus de Python. Un DAG
entièrement fixé avant exécution ne peut pas les représenter fidèlement.

**Verdict : FONDÉE.** La documentation officielle Snakemake décrit les input
functions et les checkpoints qui réévaluent le DAG après production d’une
sortie ([Snakefiles and Rules](https://snakemake.readthedocs.io/en/stable/snakefiles/rules.html)).
Le parseur OxyMake classe `checkpoint` et `module` parmi les constructions
inacceptées et teste l’erreur correspondante
([`crates/ox-translate/src/snakemake/parser.rs`, l. 1031–1057 et 2075–2112](../../crates/ox-translate/src/snakemake/parser.rs)).
Le papier concède la moindre expressivité mais avance sans preuve que la
spécification « rarely needs computation »
([`docs/paper/oxymake-paper.tex`, l. 2386–2401](../../docs/paper/oxymake-paper.tex)).

**Réponse honnête minimale.** Définir le domaine comme « DAG statique connu avant
exécution ». Retirer l’assertion de rareté ou la soutenir par un corpus. Mesurer,
sur un échantillon public de Snakefiles, la proportion rejetée ou escaladée et
les causes.

### 4. « Le traducteur démontre une compatibilité syntaxique de jouets, pas une migration »

**Attaque.** « Bidirectionnel » suggère une conservation sémantique que les tests
n’établissent pas. Un parseur ligne-à-ligne fondé sur regex ne couvre pas le
langage Python/Snakemake ; neuf petites fixtures internes ne représentent ni les
modules, wrappers, checkpoints, fonctions d’entrée, profils, stockage distant,
notebooks ou plugins d’un projet réel.

**Verdict : FONDÉE.** Le parseur détecte les règles avec une regex et reconnaît
un ensemble fini de formes
([`crates/ox-translate/src/snakemake/parser.rs`, l. 67–77](../../crates/ox-translate/src/snakemake/parser.rs)).
Les fixtures présentes sont neuf Snakefiles minimaux
([`crates/ox-translate/tests/fixtures/`](../../crates/ox-translate/tests/fixtures/)).
Le test de round-trip vérifie surtout que le TOML généré se parse et que des
champs survivent ([`crates/ox-translate/tests/round_trip.rs`, l. 1–45](../../crates/ox-translate/tests/round_trip.rs)).
Le papier ne revendique que quatre fixtures et prend l’arbre des chemins de
sortie comme oracle, pas les octets ni le comportement
([`docs/paper/oxymake-paper.tex`, l. 2040–2054](../../docs/paper/oxymake-paper.tex)).

L’objection ne nie pas l’utilité d’un assistant de migration. Elle nie que
« bidirectionnel » ou « votre vrai DAG » suffise à établir l’équivalence. Le
README invite pourtant à pointer le traducteur vers le Snakefile existant pour
voir « your real DAG » ([`README.md`, l. 36–49](../../README.md)).

**Réponse honnête minimale.** Nommer la surface « traducteur partiel avec
escalades ». Publier une matrice de constructions et un taux de succès sur des
workflows publics versionnés ; exécuter source et traduction sur les mêmes
entrées déterministes et comparer sorties et provenance.

### 5. « Le format maison est un verrou sémantique même si TOML est ouvert »

**Attaque.** Dire « TOML, no vendor lock-in » confond sérialisation et standard.
Un autre parseur TOML peut lire les tables, mais aucun autre moteur n’est tenu
d’en comprendre les règles, wildcards, clés, gates ou classes de
reproductibilité. La pérennité du workflow dépend donc d’OxyMake ou d’une
traduction partielle.

**Verdict : FONDÉE.** La propre auto-évaluation reconnaît un langage spécifique
au moteur, l’absence de vocabulaire de métadonnées standard et de packaging
communautaire ([`docs/paper/oxymake-paper.tex`, l. 2191–2246](../../docs/paper/oxymake-paper.tex)).
Elle attribue néanmoins A2 « Native — TOML, no vendor lock-in », formulation que
ces limites contredisent. `translate`, `export` et l’essentiel des payloads JSON
sont encore instables ([`STATUS.md`, l. 20–34 et 73–81](../../STATUS.md)).

**Réponse honnête minimale.** Remplacer « no vendor lock-in » par « syntaxe
ouverte, sémantique spécifique à OxyMake ». Ne compter l’interopérabilité que
sur un profil publiquement spécifié et testé par round-trip contre un moteur
indépendant.

### 6. « Face à CWL, OxyMake échange une gouvernance neutre contre le contrôle d’un seul projet »

**Attaque.** CWL sépare volontairement le standard des moteurs ; OxyMake lie
format, sémantique et implémentation à un mainteneur. Même si CWL est plus verbeux,
il offre plusieurs moteurs, fournisseurs, éditeurs et cibles de déploiement. Le
coût de migration d’un format neutre peut être inférieur au risque de pérennité
d’un format plus court.

**Verdict : FONDÉE.** CWL se définit comme un standard ouvert, portable et
vendor-neutral ([site officiel](https://www.commonwl.org/)). Sa liste officielle
recense notamment cwltool, Arvados, Toil, StreamFlow et Calrissian sur local,
cloud, Kubernetes et plusieurs ordonnanceurs HPC
([implémentations CWL](https://www.commonwl.org/implementations/)); son écosystème
comprend éditeurs, bibliothèques et WES
([outils CWL](https://www.commonwl.org/tools/)). OxyMake n’a pas de lecteur ou
writer CWL aujourd’hui ; il est repoussé à v1.1
([`docs/FAIR-ALIGNMENT.md`, l. 74–93](../../docs/FAIR-ALIGNMENT.md)).

L’ERRATUM a correctement retiré les caricatures de CWL
([`docs/paper/ERRATUM.md`, l. 73–108](../../docs/paper/ERRATUM.md)); il ne répond
pas au risque institutionnel de créer un nouveau format non neutre.

**Réponse honnête minimale.** Concéder que TOML optimise l’expérience OxyMake,
pas l’interopérabilité. Faire de CWL import/export conformant un prérequis à
toute affirmation de portabilité inter-moteurs, avec tests de conformité et
au moins un moteur indépendant.

### 7. « Le positionnement HPC est une inférence, pas un résultat d’exploitation »

**Attaque.** Le cas HPC est précisément celui où l’état local, les systèmes de
fichiers partagés, les pannes de nœud, les quotas, les reprises, les arrays, les
logs et les différences d’environnement deviennent difficiles. OxyMake se place
sur ce terrain sans avoir mesuré un seul run Slurm ou Ray.

**Verdict : FONDÉE.** Le papier dit qu’aucun run Slurm/Ray n’est mesuré et que
`.oxymake/` doit être sur disque local ; plusieurs nœuds de soumission exigent
une couche future ([`docs/paper/oxymake-paper.tex`, l. 931–937](../../docs/paper/oxymake-paper.tex)).
SQLite WAL ne fonctionne pas sur NFS/Lustre/GPFS, les FS mêmes du terrain visé
([même fichier, l. 2413–2421](../../docs/paper/oxymake-paper.tex)). Le README
ajoute que `-j N` reste séquentiel dans un batch prêt, que la gestion
d’environnements est une délégation, et que S3/GCS sont des stubs
([`README.md`, l. 64–65 et 437–445](../../README.md)).

**Réponse honnête minimale.** Présenter Slurm/Ray comme « backends implémentés,
non validés en exploitation ». Avant de viser HPC : pilote réel, FS partagé,
préemption, annulation, reprise après perte du submit node, deux soumissions
concurrentes, 20–100 échantillons et métriques de coût/échec.

### 8. « “Un seul binaire, sans daemon” déplace l’exploitation ; il ne la supprime pas »

**Attaque.** Pour un batch local, l’absence de control plane est un avantage.
Pour des exécutions récurrentes ou d’équipe, il manque calendrier, file durable,
HA, RBAC, secrets, quotas, politiques de rétention, sauvegarde/restauration de
l’état, multi-tenancy et une API réseau authentifiée. Ces besoins retombent sur
cron/CI, le scheduler HPC et des conventions maison.

**Verdict : PARTIELLEMENT FONDÉE.** Le projet n’affirme pas remplacer Airflow
pour tout usage : son étude de cas interne exclut explicitement calendrier,
RBAC, multi-tenant et orchestration de services
([`ops/research/use-cases-2026-07.md`, l. 22–34](../research/use-cases-2026-07.md)).
Mais le tableau de positionnement réduit Airflow/Prefect/Dagster à « service »
sans rendre visibles les capacités opérationnelles achetées par ce coût
([`docs/paper/oxymake-paper.tex`, l. 2286–2300](../../docs/paper/oxymake-paper.tex)).
L’architecture Airflow officielle exige effectivement scheduler, processeur de
DAG et base de métadonnées, mais apporte aussi supervision, logs et exécuteurs
([Architecture Overview](https://airflow.apache.org/docs/apache-airflow/stable/core-concepts/overview.html)).

**Réponse honnête minimale.** Ajouter une colonne « capacités opérationnelles
hors périmètre ». Borner le segment aux batchs déclenchés de l’extérieur et à un
seul domaine de confiance.

### 9. « L’interpolation transforme des noms de fichiers en fragments de shell »

**Attaque.** Le danger ne vient pas seulement d’un Oxymakefile malveillant. Un
workflow de confiance peut traiter un nom de fichier, wildcard ou valeur de
configuration provenant d’un tiers. OxyMake concatène ces valeurs sans quoting
dans une commande passée à `shell -c`; espaces, substitutions ou métacaractères
peuvent changer la commande. C’est une frontière d’injection de commande.

**Verdict : FONDÉE, sous condition que la valeur interpolée soit non fiable.**
`interpolate_full` remplace directement inputs, outputs, wildcards, params et
config, et joint les chemins avec des espaces, sans échappement shell
([`crates/ox-core/src/resolver.rs`, l. 1166–1267](../../crates/ox-core/src/resolver.rs)).
L’exécuteur passe ensuite la chaîne à l’interpréteur avec `-c`
([`crates/ox-exec-local/src/process.rs`, l. 135–168](../../crates/ox-exec-local/src/process.rs)).
Le modèle de menace traite les entrées non déclarées et l’Oxymakefile non fiable,
mais pas cette confusion donnée/code
([`SECURITY.md`, l. 24–31](../../SECURITY.md)).

Le fait qu’un auteur de workflow puisse déjà écrire n’importe quelle commande
ne réfute pas l’attaque : ici, l’auteur peut être de confiance et la donnée ne
pas l’être. Le comportement est analogue à une template shell, pas à un passage
d’arguments typés.

**Réponse honnête minimale.** Documenter immédiatement que toute interpolation
dans `shell` est du code shell non échappé. Ajouter des tests avec espaces,
quotes, `$()`, `;` et retours ligne. À terme, offrir une forme argv typée ; ne pas
« auto-quoter » silencieusement une syntaxe existante sans migration.

### 10. « Le dashboard introduit une chaîne d’approvisionnement web et une API sans authentification »

**Attaque.** Un binaire présenté comme autonome charge cinq scripts depuis
unpkg au runtime. Ces scripts ont l’origine du dashboard et peuvent lire ses API.
Le serveur n’a ni authentification ni autorisation ; `--bind 0.0.0.0` expose donc
l’état du workflow à tout pair réseau pouvant joindre le port.

**Verdict : PARTIELLEMENT FONDÉE.** Le bind par défaut est sûr au sens réseau :
`127.0.0.1` ([`crates/ox-cli/src/commands/dashboard.rs`, l. 9–30](../../crates/ox-cli/src/commands/dashboard.rs)).
Mais le routeur expose statut, DAG, jobs, runs, gates et détails sans middleware
d’authentification
([`crates/ox-dashboard/src/server.rs`, l. 24–50](../../crates/ox-dashboard/src/server.rs)),
et le HTML charge cinq scripts CDN sans Subresource Integrity
([`crates/ox-dashboard/templates/index.html`, l. 1–11](../../crates/ox-dashboard/templates/index.html)).
La présence de versions dans les URL réduit la dérive mais ne vendore pas les
octets et ne les authentifie pas côté navigateur.

**Réponse honnête minimale.** Avertir ou refuser un bind non-loopback sans option
explicite de risque. Vendoriser les assets ou fournir SRI+CSP. Dire clairement
« aucune authentification, usage local uniquement » tant qu’un modèle d’accès
n’existe pas.

### 11. « Le cache “content-addressed” ne vérifie pas toujours le contenu »

**Attaque.** Le nom suggère que chaque hit est validé par contenu. Le défaut
`mtime+hash` fait confiance à taille+mtime inchangées ; un attaquant ou un outil
capable de préserver ces métadonnées peut donc fournir un contenu altéré comme
hit. Le mode `mtime` est encore plus faible.

**Verdict : PARTIELLEMENT FONDÉE.** La clé est bien dérivée du contenu à sa
création ; l’ambiguïté concerne la validation au lookup. La politique de sécurité
dit explicitement que `mtime+hash` fait confiance aux métadonnées inchangées et
que seul `hash` relit toujours les octets
([`SECURITY.md`, l. 33–50](../../SECURITY.md)). Le papier distingue également les
modes et admet que le défaut n’a pas été mesuré séparément
([`docs/paper/oxymake-paper.tex`, l. 1810–1824](../../docs/paper/oxymake-paper.tex)).

**Réponse honnête minimale.** Employer « clé dérivée du contenu avec validation
metadata-fast-path par défaut ». Recommander `hash` pour CI, caches partagés et
adversaires, et ajouter un test same-size/same-mtime.

### 12. « La reproductibilité repose sur une discipline que le moteur ne peut pas vérifier »

**Attaque.** « Same inputs always produce the same result, on any machine, at
any time » n’est pas une propriété du système. Une commande peut lire l’heure,
le réseau, `$PATH`, la locale, une variable, un GPU, un helper non déclaré, une
image à tag mutable, ou être non déterministe. OxyMake peut mémoriser le résultat
d’une omission avec une grande assurance apparente.

**Verdict : FONDÉE.** Le README formule la garantie sans qualification à la
ligne 15 puis la borne aux entrées déclarées aux lignes 22–26
([`README.md`, l. 10–26](../../README.md)). Le papier appelle le faux hit silencieux
« the more dangerous failure » et reconnaît l’absence de sandbox
([`docs/paper/oxymake-paper.tex`, l. 844–867](../../docs/paper/oxymake-paper.tex)).
Il documente aussi deux exclusions résiduelles : corps de fonction `call` et
tags d’image mutables ([même fichier, l. 2423–2437](../../docs/paper/oxymake-paper.tex)).
La FAIR Alignment va plus loin encore en écrivant « déterminisme / réexécution »
et « bit-for-bit reproducibility » sans ces préconditions
([`docs/FAIR-ALIGNMENT.md`, l. 17–30](../../docs/FAIR-ALIGNMENT.md)).

**Réponse honnête minimale.** Remplacer partout par : « à commande déterministe,
plateforme compatible et entrées exhaustivement déclarées, la même clé sélectionne
le même artefact mis en cache ». Ne pas appeler cela une garantie bit-for-bit du
calcul sans sandbox et test de reproductibilité.

### 13. « L’écosystème de plugins est une architecture interne, pas un écosystème »

**Attaque.** Une liste de traits Rust ne constitue pas un système de plugins. Un
utilisateur doit forker ou compiler un binaire personnalisé contre des traits
instables ; il n’y a ni chargement runtime, registre, compatibilité SemVer,
distribution, ni implémentation externe éprouvée.

**Verdict : FONDÉE.** `STATUS.md` dit exactement que tous les traits sont
instables, qu’aucun plugin externe n’est chargé au runtime et qu’aucune
implémentation hors arbre n’a été « used in anger »
([`STATUS.md`, l. 238–267](../../STATUS.md)). Le papier titre pourtant la section
« Plugin Architecture » et présente cinq axes
([`docs/paper/oxymake-paper.tex`, l. 1304–1317](../../docs/paper/oxymake-paper.tex)).

**Réponse honnête minimale.** Renommer « extension traits internes ». Réserver
« plugin » à une extension hors arbre effectivement construite, distribuée,
versionnée et testée contre deux versions du moteur.

### 14. « Le bus factor est un risque de produit, de sécurité et de standardisation »

**Attaque.** Un moteur d’orchestration devient une dépendance durable au cœur des
pipelines. Un mainteneur unique implique délais, absence de revue indépendante,
risque de compromission de compte, faible redondance de release et aucune
garantie de succession. Une CI abondante ne remplace pas une seconde personne.

**Verdict : FONDÉE.** Le README annonce maintenance best-effort, réponses en
semaines et mainteneur unique ([`README.md`, l. 394–410](../../README.md)).
`CODEOWNERS` affirme qu’il existe exactement un membre/admin, qu’il n’y a pas de
second arbitre exogène et que cette exigence est non satisfaite
([`.github/CODEOWNERS`, l. 3–25](../../.github/CODEOWNERS)). Au 25 juillet 2026,
l’[API GitHub du dépôt](https://api.github.com/repos/noogram/oxymake) rapporte
1 étoile, 0 fork et une seule release ; l’endpoint
[contributors](https://api.github.com/repos/noogram/oxymake/contributors)
retourne un seul compte contributeur. Ces nombres sont un instantané, pas une
mesure de qualité, mais ils réfutent l’existence actuelle d’une communauté
redondante.

Les mitigations supply-chain sont réelles : actions et toolchain épinglés,
checksums et `cargo deny` avant release
([`.github/workflows/release.yml`, l. 8–16 et 29–55](../../.github/workflows/release.yml)).
Elles réduisent le risque technique, pas le bus factor.

**Réponse honnête minimale.** Concéder le risque sans transformer une roadmap
communautaire en promesse. Publier un processus de succession/release d’urgence ;
ne revendiquer une gouvernance partagée qu’après droits et revue réellement
indépendants.

### 15. « Le benchmark principal optimise une phase qui n’est pas le coût utilisateur »

**Attaque.** Un speedup de 33,3× sur 2,31 secondes de planification peut être
spectaculaire et économiquement négligeable dans un pipeline scientifique de
plusieurs heures. Sur le seul résultat end-to-end froid, OxyMake est 1,25–2,3×
plus lent. Le benchmark favorise en plus un DAG régulier, des règles triviales,
un nombre fixe de patterns et aucune contention d’I/O réelle.

**Verdict : FONDÉE.** Le papier borne correctement le 33,3× à la résolution
([`docs/paper/oxymake-paper.tex`, l. 1784–1856](../../docs/paper/oxymake-paper.tex))
et publie le désavantage froid
([même fichier, l. 1858–1895](../../docs/paper/oxymake-paper.tex)). Il reconnaît
que l’attribution du surcoût au hashing/store n’est pas issue d’un profil. La
scale study garde le nombre de règles faible et fixe ; l’algorithme demeure
linéaire dans les patterns par cible, et 50k/100k ne sont que des projections
([même fichier, l. 2001–2028](../../docs/paper/oxymake-paper.tex)).

Autre faiblesse : le comparateur est Snakemake 7.32.4 alors que la documentation
stable consultée le 25 juillet 2026 est 9.23.1
([documentation officielle](https://snakemake.readthedocs.io/en/stable/)). Le
papier justifie 7.x par la sémantique de provenance, mais cela ne justifie pas
son emploi pour une comparaison de performance de parsing/exécution contre la
version courante.

**Réponse honnête minimale.** Faire du résultat warm/no-op le résultat ciblé et
du 33,3× une micro-mesure. Ajouter latest Snakemake, CPU/Linux, graphes avec
nombre de règles croissant, commandes réalistes et profils phase-par-phase.

### 16. « Sept axes d’évaluation ne couvrent presque aucun axe non fonctionnel »

**Attaque.** L’évaluation mesure temps, mémoire, taille, quelques traductions et
une auto-évaluation FAIR. Elle ne mesure ni temps d’apprentissage, erreurs
d’auteur, maintenabilité des workflows, diagnostic, reprise après panne,
robustesse réseau/FS, contention multi-session, sécurité, observabilité en
incident, consommation énergétique, coût cluster, portabilité Linux/Windows,
compatibilité de versions, ni adoption.

**Verdict : FONDÉE.** Les sept axes annoncés sont précisément ceux-là et tous les
benchmarks viennent d’un Apple M4 Max
([`docs/paper/oxymake-paper.tex`, l. 1758–1782](../../docs/paper/oxymake-paper.tex)).
Il n’y a ni dispersion publiée ni réplication Linux/x86-64 ; trois runs seulement
pour l’end-to-end
([même fichier, l. 2462–2470](../../docs/paper/oxymake-paper.tex)). Slurm, Ray,
MCP, dashboard et cache distant sont implémentés mais non mesurés ; K8s, S3/GCS
et cinq passes ne sont pas implémentés
([même fichier, l. 2138–2189](../../docs/paper/oxymake-paper.tex)).

L’ERRATUM améliore fortement l’honnêteté factuelle, mais révèle aussi que des
erreurs structurantes ont traversé v2 : comptage de jobs, ratios inversés,
table de crates périmée et FAIR surévalué
([`docs/paper/ERRATUM.md`, l. 231–304](../../docs/paper/ERRATUM.md)). Cela justifie
une réplication externe, pas seulement une nouvelle auto-correction.

**Réponse honnête minimale.** Renommer la section « évaluation micro-performance
et validation fonctionnelle préliminaire ». Ajouter un tableau explicite
« non mesuré ». Priorité à une réplication indépendante Linux et à une campagne
de panne/cluster avant d’élargir les claims.

### 17. « L’adéquation au domaine est postulée, pas observée »

**Attaque.** Le produit vise à la fois science/HPC, petites équipes data, ML et
agents, soit quatre marchés aux contraintes incompatibles. Aucun utilisateur,
déploiement longitudinal ou workflow de production n’établit que le compromis
TOML/cache/daemon-free résout mieux leur problème global.

**Verdict : PARTIELLEMENT FONDÉE.** Le papier est prudent : il nomme deux cibles
« credible », qualifie ML et agents de plausibles et précise qu’il s’agit
d’inférences de design, pas de résultats d’adoption
([`docs/paper/oxymake-paper.tex`, l. 2363–2381](../../docs/paper/oxymake-paper.tex)).
L’enquête interne conclut elle aussi « plausible » et demande des pilotes
([`ops/research/use-cases-2026-07.md`, l. 1–20 et 82–86](../research/use-cases-2026-07.md)).
L’attaque est donc juste contre le positionnement produit large, mais le papier
reconnaît déjà le manque de preuve.

**Réponse honnête minimale.** Choisir un seul beachhead et publier un pilote
longitudinal avec coûts de migration, incidents, hits de cache utiles, temps de
debug et abandon. Garder les trois autres comme hypothèses.

### 18. « L’étendue du produit excède sa capacité de maintenance »

**Attaque.** Vingt-cinq sous-commandes, vingt-quatre crates du workspace, TUI,
dashboard, MCP, deux traducteurs, trois exécuteurs, caches, lockfiles et
spécifications formelles créent une surface de bugs, documentation et compatibilité
disproportionnée pour un mainteneur et une release. La largeur peut masquer le
durcissement insuffisant du chemin critique.

**Verdict : FONDÉE.** `STATUS.md` compte 25 sous-commandes et déclare une grande
partie instable ([`STATUS.md`, l. 38–81](../../STATUS.md)); le workspace liste 24
crates et en exclut une vingt-cinquième
([`Cargo.toml`, l. 1–32](../../Cargo.toml)). Le binaire par défaut embarque même
TUI, web dashboard et traducteurs
([`docs/paper/oxymake-paper.tex`, l. 2114–2136](../../docs/paper/oxymake-paper.tex)).
Parallèlement, environnements, objet stores, K8s, plugins runtime, politique de
rollover du lockfile et plusieurs optimisations restent incomplets ou instables.

**Réponse honnête minimale.** Publier un « supported core » beaucoup plus petit
et classer le reste experimental. Mesurer maintenance et incidents par surface ;
geler ou extraire les surfaces qui n’ont pas d’utilisateur externe.

### 19. « L’exécution convergente n’est pas une durable execution pour effets externes »

**Attaque.** Un job qui envoie un message, débite un compte, publie un artefact ou
modifie une base ne devient pas idempotent parce que le DAG converge sur des
fichiers. Un crash entre l’effet externe et l’écriture d’état peut le rejouer.
L’interface MCP rend ce malentendu particulièrement dangereux pour un agent.

**Verdict : PARTIELLEMENT FONDÉE.** Le domaine déclaré du papier est celui de
commandes déterministes avec fichiers comme frontières
([`docs/paper/oxymake-paper.tex`, l. 2363–2367](../../docs/paper/oxymake-paper.tex));
il ne promet pas de transactions distribuées. Mais l’étiquette « idempotent
convergent execution » et le positionnement agent peuvent être lus plus largement.
La propre étude interne avertit d’éviter les effets externes non idempotents ou
d’ajouter token et gate
([`ops/research/use-cases-2026-07.md`, l. 185–197](../research/use-cases-2026-07.md)).

**Réponse honnête minimale.** Dire que la convergence porte sur les artefacts
fichier déclarés, pas sur les effets externes. Exclure explicitement ces derniers
du modèle de reprise tant qu’il n’existe ni outbox, compensation, ni token
d’idempotence persisté avant effet.

### 20. « “Binaire autonome” et installation simple restent des affirmations de distribution fragiles »

**Attaque.** Le projet présente plusieurs canaux comme une expérience
d’installation unifiée, mais le livre ne documente que la compilation source,
le package Cargo du workspace n’est pas publiable, Windows n’est pas produit, et
les règles Python/R/Julia, Docker, conda, Slurm et Ray réintroduisent des runtimes
externes.

**Verdict : PARTIELLEMENT FONDÉE.** Une release v0.1.0 existe et le pipeline
construit trois cibles Linux/macOS avec checksums
([`.github/workflows/release.yml`, l. 57–125](../../.github/workflows/release.yml)).
Le README distingue correctement le binaire du runtime des règles et énumère les
outils à fournir ([`README.md`, l. 87–104 et 427–430](../../README.md)). Mais le
guide d’installation ne présente encore que le build source et affirme « no
runtime dependencies » sans cette nuance
([`docs/book/src/getting-started/installation.md`, l. 1–15 et 66–81](../../docs/book/src/getting-started/installation.md)).
Le workspace est `publish = false` ([`Cargo.toml`, l. 34–43](../../Cargo.toml)).

**Réponse honnête minimale.** Séparer « aucune dépendance runtime pour le moteur
local » de « dépendances des jobs/backends ». Tester les canaux d’installation
en CI sur chaque cible publiée et déclarer Windows non supporté.

## Attaques rejetées

### A. « Le papier cache que le cold run est plus lent » — **INFONDÉE**

Le résultat est dans le README et dans le papier, avec ratios et hypothèse
d’attribution explicitement non profilée
([`README.md`, l. 51–57](../../README.md) ;
[`docs/paper/oxymake-paper.tex`, l. 1858–1895](../../docs/paper/oxymake-paper.tex)).
La critique pertinente est le poids rhétorique du 33×, pas une dissimulation.

**Réponse minimale.** Aucune correction factuelle ; conserver cold et warm au
même niveau visuel.

### B. « Le projet prétend que CWL est un moteur mtime inférieur » — **INFONDÉE pour v3**

Cette caractérisation appartenait à v2. L’ERRATUM la retire, distingue standard
et implémentations, et documente le cache content-derived de cwltool
([`docs/paper/ERRATUM.md`, l. 73–108](../../docs/paper/ERRATUM.md)). La v3 décrit
CWL comme couche complémentaire et standard vendor-neutral.

**Réponse minimale.** Ne pas réintroduire la comparaison dans les supports plus
courts ; faire vérifier la table finale par un expert CWL indépendant.

### C. « Il n’y a aucune discipline supply-chain » — **INFONDÉE**

Les actions et le toolchain de release sont épinglés, les artefacts ont des
checksums, `cargo-deny` bloque la release et une analyse advisory planifiée existe
([`.github/workflows/release.yml`, l. 8–16 et 29–55](../../.github/workflows/release.yml) ;
[`.github/workflows/deny.yml`](../../.github/workflows/deny.yml)). Cela ne donne
ni signatures/SLSA, ni revue humaine indépendante, mais « aucune discipline »
serait faux.

**Réponse minimale.** Décrire exactement ces contrôles sans employer
« reproductible » tant qu’une reconstruction indépendante bit-à-bit n’est pas
publiée.

## Priorité de réponse

1. **Avant toute nouvelle claim :** corriger les formulations sur
   inspectabilité, reproductibilité, verrouillage et plugins (attaques 2, 5, 12,
   13).
2. **Avant recommandation à des tiers :** documenter/traiter l’interpolation
   shell et le dashboard (9, 10, 11).
3. **Avant claim HPC :** campagne réelle Slurm/FS partagé/pannes (7, 16).
4. **Avant claim d’interopérabilité :** corpus externe, latest Snakemake et
   moteur standard indépendant (3, 4, 6, 15).
5. **Avant élargissement du produit :** un pilote utilisateur et une réduction
   explicite du supported core (14, 17, 18).

Le plus petit récit entièrement soutenu aujourd’hui tient en une phrase :
**OxyMake v0.1 est un moteur expérimental, rapide à planifier, pour DAG statiques
fichier-à-fichier sur un domaine de confiance unique ; sa valeur en production,
sur cluster et entre moteurs reste à établir.**
