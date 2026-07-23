# Cas d'usage réels pour OxyMake — enquête 2026-07-22

## Conclusion courte

OxyMake est le plus crédible là où une équipe possède déjà des commandes de
calcul déterministes et des fichiers comme frontières d'artefacts, mais veut
éliminer la recomputation et la colle d'orchestration sans déployer un service.
Les meilleurs premiers terrains sont (1) les pipelines scientifiques/HPC
portables et (2) les petites équipes data qui veulent un DAG local, observable
et partageable. Le cas des agents IA est prometteur et documenté comme besoin
du marché, mais reste à prouver par des utilisateurs d'OxyMake.

Cette note sépare volontairement :

- **documenté** : la douleur est attestée par une source primaire ou une
  discussion technique identifiable ;
- **plausible** : l'adéquation avec OxyMake est une inférence, à valider par un
  entretien ou un pilote ;
- **limite produit** : une contrainte actuelle qui interdit de présenter le
  produit comme une réponse générale.

## Contrat OxyMake examiné

Les correspondances ci-dessous reposent sur des capacités présentes dans le
code et la documentation du dépôt, pas sur une feuille de route :

| Élément du contrat | Ce que le dépôt établit | Garde-fou |
| --- | --- | --- |
| Convergence / idempotence | `ox run` calcule les sorties manquantes ou obsolètes jusqu'à la cible ; les jobs déclarés à sorties déterministes sont réutilisés. | Ce n'est pas une garantie sur une commande avec effets externes, temps, hasard ou entrées non déclarées. |
| Clé de contenu | Les clés incluent le contenu des entrées, la source de règle/script et l'environnement ; les tests couvrent les changements de script et d'environnement. | Une lecture non déclarée est invisible à la clé ([README](../../README.md)). |
| Portabilité laptop → cluster | Même workflow avec exécuteurs locaux, Slurm ou Ray ; un cache distant de répertoire est disponible. | Le cluster, `sbatch`/Ray et les environnements scientifiques restent à provisionner ; les caches S3/GCS sont encore des stubs ([README](../../README.md)). |
| Surface agent | `ox run --json` émet du NDJSON et `ox serve --mcp` sert MCP via stdio. | MCP ne transforme pas, à lui seul, une action externe en action sûre ou durable. |
| Opérations | Binaire CLI Rust unique ; pas de serveur de planification ou de base de métadonnées requis pour un flux local. | Ce n'est pas un remplacement complet d'Airflow/Prefect pour calendriers, API multi-tenant, RBAC ou orchestration de services. |

## Tableau de synthèse

| Cas | Workflow concret | Douleur documentée | Pourquoi la pile courante laisse un espace | Apport OxyMake | Niveau |
| --- | --- | --- | --- | --- | --- |
| Bioinformatique d'équipe | FASTQ → QC → alignement → BAM/VCF → rapport ; développement laptop, exécution Slurm | Cache et `$HOME` indisponibles sur nœuds de calcul ; problèmes de métadonnées sur FS partagé ; invalidation de cache délicate | Les moteurs sont riches mais exposent des réglages FS/cache/environnement dont l'équipe doit maîtriser les interactions | DAG convergent, clés par contenu, exécuteur Slurm sans réécrire le flux, cache de répertoire partagé | Douleur **documentée** ; fit **plausible fort** |
| ML expérimental | Ingestion/versionnement → features → entraînement de N configurations → évaluation/registre | Cache partagé entre machine et GPU exige permissions ; les outils couvrent souvent tracking, données et DAG séparément | DVC et MLflow répondent à des parties différentes ; leur composition demande conventions et synchronisation d'artefacts | Recalcul minimal des features/évaluations, clé explicable, sorties NDJSON pour relier un tracker | Douleur **documentée** ; remplacement global **non démontré** |
| Recherche computationnelle | Prétraitement → lots de simulations/économétrie/climat → agrégation → figures/publication | Reproductibilité HPC menacée par dépendances matérielles, contexte d'infrastructure et provenance ; calculs/caches surprenants | Notebooks/scripts et ordonnanceurs distribuent le travail sans toujours conserver une frontière d'artefacts ou une reprise simple | Exécution déclarative de commandes, réemploi par contenu, même fichier local/Slurm/Ray | Douleur **documentée** ; fit **plausible**, à pilote par discipline |
| Agents IA opérant sur des artefacts | Agent planifie ; moteur lance analyse/build/test/export ; agent lit les événements et corrige | Les appels longs nécessitent polling/état ; le protocole MCP discute justement un construct asynchrone déterministe | Un agent qui boucle lui-même sur shell/API mélange raisonnement, retries et état ; les moteurs durables existants sont souvent orientés service | MCP stdio + NDJSON, DAG rerunnable et sorties mises en cache : l'agent pilote, le moteur exécute | Besoin **documenté** ; avantage OxyMake **plausible** |
| Petite/moyenne équipe data | Extraction quotidienne → validation → transformation → export parquet/CSV ; d'abord sur laptop puis VM/cluster | Airflow HA impose DB compatible, verrous et scheduler ; Prefect déploie des flows côté serveur avec work pools | La puissance d'une plateforme de contrôle est un coût d'exploitation disproportionné pour un DAG batch modeste | Un binaire, fichier de workflow, cache et événements structurés ; migration d'exécuteur graduelle | Complexité des alternatives **documentée** ; segment **plausible fort** |

## Fiches

### 1. Bioinformatique : pipeline de cohorte, workstation vers Slurm

**Workflow.** Une plate-forme reçoit des FASTQ, lance FastQC/trim, aligne les
lectures, produit BAM puis VCF et un rapport QC. Le bioinformaticien change une
règle localement ; la cohorte complète part ensuite sur Slurm. Les artefacts
intermédiaires coûteux doivent être réemployables entre checkouts compatibles
de l'équipe.

**Douleur observée.** Dans un ticket Snakemake, des utilisateurs de conteneurs
HTCondor/Slurm rapportent que le cache tombe sur un `$HOME`/`/.cache` non
inscriptible ; une solution proposée consiste à modifier le conteneur ou le
code pour rediriger le cache vers scratch. La même discussion relie le problème
aux contraintes de nœuds HPC et de mémoire
([Snakemake #1593](https://github.com/snakemake/snakemake/issues/1593)). Côté
Nextflow, la documentation reconnaît que les timestamps incohérents des
systèmes de fichiers partagés imposent un mode de cache spécial ; la clé par
défaut n'est pas entièrement fondée sur le contenu
([référence Nextflow `cache`](https://www.nextflow.io/docs/latest/reference/process.html#cache)).
Une demande de fonctionnalité décrit aussi des recalculs inutiles quand une
métadonnée non pertinente change, avec des `subMap` manuels faciles à mal joindre
([Nextflow #5308](https://github.com/nextflow-io/nextflow/issues/5308)).

**Pourquoi l'outillage ne suffit pas toujours.** Snakemake et Nextflow sont des
solutions matures, et ce cas n'est pas un argument pour les remplacer en bloc.
La friction est le couplage concret cache/FS/conteneur/profil d'ordonnanceur,
ainsi que la sémantique exacte de la clé. Les réglages existent, mais deviennent
une responsabilité du laboratoire.

**Où OxyMake apporte de la valeur.** Si chaque commande déclare intégralement
ses entrées et sorties, une clé de contenu évite de dépendre des mtimes. Le même
workflow peut conserver les commandes et changer seulement d'exécuteur (local,
Slurm ou Ray), tandis qu'un cache distant de **répertoire** permet un partage
contrôlé. La convergence signifie qu'une relance vise les artefacts manquants ou
invalides plutôt qu'une nouvelle cohorte entière.

**Limites / preuve.** Fit **plausible fort**, pas encore une preuve de migration
en bioinformatique. Tester d'abord un pipeline de 20–100 échantillons avec
montage scratch réel, fichiers de référence, conteneur et deux clones ; mesurer
les hits, invalidations et reprises. Ne pas promettre S3/GCS ni cacher les
lectures non déclarées.

### 2. ML : feature pipeline et sweep d'expériences

**Workflow.** Une petite équipe extrait des données, fabrique des features,
entraîne un ensemble de configurations, compare métriques et publie le meilleur
modèle. Les features et évaluations doivent être partagées entre laptop et
serveur GPU sans refaire les étapes inchangées.

**Douleur observée.** La documentation DVC présente précisément le scénario
machine de travail ↔ serveur GPU et demande de créer un cache partagé avec des
permissions communes ; elle avertit que le GC sur un cache partagé peut
supprimer des données nécessaires à un autre projet
([partage du cache DVC](https://dvc.org/doc/user-guide/how-to/share-a-dvc-cache)).
DVC se présente par ailleurs comme outil de versionnement, pipeline incrémental
et expériences ; MLflow Projects comme format de packaging/exécution
([DVC](https://github.com/iterative/dvc),
[MLflow Projects](https://mlflow.org/docs/latest/ml/projects/)). Cette séparation
documente le besoin, mais laisse à l'équipe la couture entre données, DAG et
tracking.

**Pourquoi l'outillage ne suffit pas toujours.** DVC est très adapté à la
version des données et au pipeline ; MLflow est très adapté au tracking. Ils ne
sont pas un échec : l'espace est celui d'une équipe qui ne veut pas maintenir
plusieurs conventions pour savoir quelle transformation a produit quel
artefact, surtout pendant un sweep.

**Où OxyMake apporte de la valeur.** OxyMake peut être le plan d'exécution des
étapes fichier-à-fichier : features et évaluations identiques convergent vers
le cache au lieu de se relancer. Les événements NDJSON donnent à un wrapper
MLflow/DVC une interface machine plutôt que du parsing de logs. Conserver DVC
pour les gros objets/versionnement et MLflow pour l'UI/registre est souvent le
positionnement le plus crédible.

**Limites / preuve.** Douleur **documentée**, bénéfice d'intégration
**plausible**. Il faut un adaptateur ou une convention de métadonnées pour que
la clé OxyMake, le run MLflow et le hash DVC se recoupent. Un pilote doit
comparer un sweep avec et sans réemploi des features et vérifier qu'aucune
entrée notebook, seed, image ou dataset n'est omise.

### 3. Recherche computationnelle : simulations, économétrie et climat

**Workflow.** Préparer jeux de paramètres et données, exécuter des centaines
de simulations ou régressions par lots, agréger, puis générer tableaux et
figures. Le même projet passe d'un portable à un nœud de calcul, parfois à un
cluster Ray.

**Douleur observée.** La littérature récente sur HPC relève les dépendances
matérielles, le contexte d'infrastructure et la documentation insuffisante
comme menaces à la reproductibilité
([Rethinking Reproducibility in the Classical (HPC)-Quantum Era](https://arxiv.org/abs/2603.04924)).
La revue scientifique sur les workflows/provenance lie explicitement réemploi,
exécution scalable et science reproductible à la traçabilité des données
([Scientific Workflows and Provenance](https://arxiv.org/abs/1311.4610)). Dans
un retour Dask, des mainteneurs décrivent performance et moment de calcul comme
opaques/surprenants et souhaitent éviter les recomputations inutiles
([dask/community #301](https://github.com/dask/community/issues/301)).

**Pourquoi l'outillage ne suffit pas toujours.** Un ordonnanceur HPC place les
jobs, Dask exécute un graphe Python, et un notebook raconte l'analyse ; aucun
de ces éléments ne force seul une déclaration complète des fichiers, une clé
portable ou une reprise de bout en bout. Les workflows scientifiques dédiés
peuvent le faire, mais imposent un langage ou une plateforme supplémentaire.

**Où OxyMake apporte de la valeur.** Pour des étapes déjà exprimables comme
commandes et fichiers, un Oxymakefile fournit une frontière légère : entrée,
sortie, clé de contenu et réexécution convergente. Slurm/Ray donnent une montée
en capacité sans réécrire la structure du flux ; NDJSON rend la provenance de
run exploitable par un rapport.

**Limites / preuve.** Fit **plausible**. La reproductibilité scientifique
nécessite aussi seeds, versions de compilateur/librairies, image/containers,
architecture CPU/GPU et archivage des données. Les inclure comme entrées ou
provenance est indispensable ; OxyMake ne les infère pas magiquement.

### 4. Agents IA : un agent décide, un moteur converge

**Workflow.** Un agent reçoit « régénère l'analyse et publie le rapport » ; il
inspecte le graphe, lance le workflow, suit les événements, puis ne raisonne à
nouveau qu'en cas d'échec ou de décision humaine. Les étapes font par exemple
collecte → validation → synthèse → tests → export, avec artefacts nommés.

**Douleur observée.** La proposition MCP SEP-1391 décrit les opérations longues
comme un problème de polling, de contexte et de comportement modèle ; elle
oppose ce comportement à un construct asynchrone déterministe défini par le
protocole ([MCP SEP-1391](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1391)).
Des moteurs de durable execution présentent explicitement retries, persistance,
compensation et exposition MCP comme réponse aux appels d'agents peu fiables
([Conductor AI Cookbook](https://conductor-oss.github.io/conductor/devguide/ai/)).
Une publication MCP-native de 2026 rapporte elle aussi l'intérêt d'un plan
déclaratif, idempotent, exécuté sans agent au runtime
([MCP Workflow Engine](https://arxiv.org/abs/2605.00827)).

**Pourquoi l'outillage ne suffit pas toujours.** MCP standardise l'accès aux
outils, pas la sémantique d'un DAG ni la sûreté des retries. Les plateformes de
durable execution répondent au besoin, mais supposent fréquemment un service,
un SDK ou une infrastructure de contrôle plus large que le flux local d'un
projet.

**Où OxyMake apporte de la valeur.** `ox serve --mcp` donne à l'agent un point
d'entrée structuré ; `ox run --json` offre un flux NDJSON stable. Le moteur peut
converger vers les artefacts voulus et réutiliser des résultats dont la clé de
contenu correspond, ce qui réduit les appels outil et la recomputation. C'est
une séparation nette : l'agent planifie/interprète, OxyMake exécute des étapes
bornées.

**Limites / preuve.** Besoin de marché **documenté**, valeur OxyMake
**plausible**. Le MCP actuel est stdio et les garanties de durabilité complètes
(files d'attente, compensation, reprise inter-hôte, autorisation) ne doivent
pas être revendiquées. Éviter les effets externes non idempotents (envoi,
paiement, suppression) ou les encapsuler derrière une clé/idempotency token et
une approbation.

### 5. Petite/moyenne équipe data : batch sans control plane permanent

**Workflow.** Tous les jours : télécharger une exportation fournisseur,
valider le schéma, joindre/rendre propre, produire Parquet et un CSV pour la
finance. Aujourd'hui le flux tourne dans un script CI ou manuellement ; demain
il peut nécessiter plus de CPU, sans multi-tenant ni dizaines de DAGs.

**Douleur observée.** Airflow documente qu'un scheduler HA dépend d'une base de
métadonnées et de verrous de lignes ; PostgreSQL 12+ ou MySQL 8 sont les
expériences pleinement supportées, et d'autres choix peuvent mener à des
deadlocks ([Airflow Scheduler](https://airflow.apache.org/docs/apache-airflow/stable/administration-and-deployment/scheduler.html)).
Prefect décrit les deployments comme représentations server-side et les lie à
une infrastructure via work pools
([Prefect Deployments](https://docs.prefect.io/v3/concepts/deployments)). Ces
architectures sont rationnelles à grande échelle ; elles matérialisent aussi un
coût de plateforme réel pour un unique batch.

**Pourquoi l'outillage ne suffit pas toujours.** Airflow/Prefect apportent
planification, UI, politiques et opérations distribuées ; les écarter est une
décision de périmètre, non un jugement de qualité. Une équipe de 2–10 personnes
peut surtout vouloir une relance fiable, un cache et un log lisible, sans DB ni
service à maintenir.

**Où OxyMake apporte de la valeur.** Le binaire et le fichier de workflow
peuvent vivre dans le dépôt du projet ; une invocation externe (cron, CI ou
scheduler existant) déclenche `ox run`. La convergence évite de retraiter les
sorties valides après un échec et le NDJSON permet logs/alertes sans scraper la
sortie humaine. Si le besoin monte, l'exécuteur peut changer sans redessiner
les dépendances de données.

**Limites / preuve.** Segment **plausible fort**. OxyMake n'apporte pas à lui
seul une planification calendaire, un UI de gouvernance, des permissions
centralisées ou une API de plateforme. C'est une excellente frontière de
positionnement : recommander Airflow/Prefect dès que ces besoins deviennent le
problème principal.

## Priorisation et tests de marché

1. **Bioinformatique/HPC** — recruter un laboratoire qui a déjà Slurm et deux
   clones du même pipeline. Critère : taux de cache hit et temps de reprise,
   pas « nombre de règles converties ».
2. **Petite équipe data** — convertir un batch quotidien à 5–20 étapes.
   Critère : aucune base/service ajouté, reprise après interruption et
   observabilité NDJSON consommée par CI.
3. **ML feature pipeline** — intégrer, ne pas remplacer, DVC/MLflow. Critère :
   réemploi vérifié d'une feature coûteuse et lien explicite entre run/clé.
4. **Agents IA** — démo avec étapes sans effets externes. Critère : après un
   second ordre identique, l'agent observe des artefacts convergés plutôt que
   relancer le calcul ; journal NDJSON interprétable sans parsing libre.
5. **Recherche générale** — un pilote par domaine avant message marketing.
   Critère : manifest d'environnement et entrées complets, puis reproduction
   sur une seconde machine/partition.

## Méthode et sources

Recherche web effectuée le 2026-07-22. Les sources privilégiées sont les
issues/docs des outils concernés et les publications directement accessibles.
Une issue prouve l'existence et la nature d'une douleur chez son auteur, pas sa
fréquence statistique ; les conclusions de segment sont donc annotées
« plausible » tant qu'un pilote OxyMake ne les confirme pas.
