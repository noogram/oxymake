# Interpolation non échappée dans `shell` — triage (2026-08-10)

## Verdict

**Confirmé.** Un Oxymakefile de confiance qui interpole une donnée contrôlée
par un tiers dans un champ `shell` peut faire interpréter cette donnée comme du
code par le shell. Ce n'est pas l'exécution attendue d'un Oxymakefile hostile :
c'est une frontière donnée/code distincte dans un workflow par ailleurs digne
de confiance.

La réponse livrée est documentaire uniquement. Elle ne change ni la
sémantique d'interpolation ni le découpage en mots des commandes existantes.

## Vérification du chemin d'exécution

| Étape | Preuve | Constation |
|---|---|---|
| Construction des valeurs | `crates/ox-core/src/resolver.rs:528-546` | Les entrées et sorties concrètes deviennent des chaînes à interpoler. |
| Interpolation de `shell` | `crates/ox-core/src/resolver.rs:1033-1055` | Un `ExecutionBlock::Shell` reçoit le résultat de `interpolate_full`. |
| Substitution non échappée | `crates/ox-core/src/resolver.rs:1170-1267` | `replace` insère wildcards, params, config, log et ressources tels quels; `{input}` et `{output}` sont joints par `" "` aux lignes 1247-1248. |
| Exécution | `crates/ox-exec-local/src/process.rs:135-168` | L'exécuteur construit `Command::new(shell).arg("-c").arg(command)`. |

L'audit adversarial initial était donc exact sur le mécanisme. Les plages de
lignes peuvent naturellement évoluer; les références ci-dessus sont relevées
sur le commit de ce livrable.

## Reproduction exécutée

Environnement : `ox 0.1.0`, macOS, exécuteur local, cache désactivé. Le
répertoire temporaire contenait un fichier reçu d'un tiers, nommé
`incoming/input; touch INJECTED_FROM_INPUT; #.txt`. La liste `samples` simule
une configuration générée depuis des métadonnées tierces; elle sélectionne ce
nom de fichier dans le workflow de confiance suivant :

```toml
ox_version = "0.1"

[config]
samples = ["input; touch INJECTED_FROM_INPUT; #"]

[rule.copy]
input = ["incoming/{sample}.txt"]
output = ["out/result.done"]
shell = "printf '%s\\n' {input} > {output}"
```

Commande lancée :

```text
/bin/bash -c 'printf '\''%s\n'\'' incoming/input; touch INJECTED_FROM_INPUT; #.txt > out/result.done'
```

Résultat observé : le shell a affiché `incoming/input`, le fichier
`INJECTED_FROM_INPUT` a été créé, puis OxyMake a signalé l'échec parce que la
redirection avait été commentée et que `out/result.done` n'existait pas.
L'exécution a donc terminé avec le code 1, mais le code injecté avait déjà été
exécuté. Une variante avec la même valeur interpolée dans `{output}` a produit
le même effet (`INJECTED` créé).

## Délimitation réaliste

La provenance est une propriété du déploiement, pas du type de placeholder.

| Valeur interpolée | Peut venir d'un tiers dans un usage réaliste ? | Limite |
|---|---|---|
| Chemins `{input}` | Oui, lorsqu'un nom dans un checkout, une archive extraite, un upload, un partage ou une sortie amont est sélectionné par un target/wildcard non fiable. | Les entrées littérales d'un Oxymakefile de confiance sont contrôlées par son auteur. La découverte des fichiers existants ne déduit pas à elle seule de nouvelles valeurs de wildcard. |
| Chemins `{output}` et `{log}` | Oui, s'ils incorporent un wildcard ou une valeur de configuration d'origine externe. | Un chemin de sortie/log entièrement littéral est contrôlé par l'auteur. |
| Wildcards (`{name}`, `{wildcards.name}`) | Oui. Ils proviennent d'un target demandé qui matche une sortie, ou de listes de configuration utilisées pour l'expansion des entrées. | La découverte des fichiers existants reconnaît des sources mais ne les énumère pas pour créer des wildcards; ils sont contrôlés uniquement si l'auteur contraint leur domaine à des constantes fiables. |
| `{config.name}` | Oui. La documentation propose elle-même de générer la configuration hors de l'Oxymakefile; un import, une génération ou un `--set` peut transporter des données tierces. | Une constante écrite dans l'Oxymakefile de confiance est contrôlée par son auteur. |
| `{params.name}` | En général non : les paramètres sont déclarés dans la règle, donc contrôlés par l'auteur. | Ils deviennent non fiables si l'auteur y injecte explicitement une donnée externe avant l'exécution. |
| `{resources.name}`, `{threads}` | En général contrôlés par l'auteur ou l'opérateur. | Même réserve : une valeur routée depuis une entrée externe reste non fiable. |

La même interpolation sert aussi `run`, `script` et `call`, mais le fait établi
ici est l'interprétation comme langage de commande à la frontière `shell -c`.
Les autres modes ont leurs propres contextes d'injection et ne doivent pas être
présumés sûrs par analogie.

## Documentation corrigée

`SECURITY.md` nomme désormais explicitement cette frontière et la responsabilité
de l'utilisateur. La référence des expressions et la page des modes d'exécution
disent également que `shell` reçoit du texte brut, plutôt que de laisser entendre
que les chemins sont transmis comme arguments. La référence du format affirmait
que les wildcards pouvaient être « inferred from existing files » : le
résolveur ne le fait pas; il reconnaît ces fichiers comme sources et lie les
wildcards depuis un target ou les listes de configuration. Cette affirmation a
été corrigée. Aucun énoncé du papier ne promet un passage argv typé ou un
échappement de l'interpolation; ses passages sur les processus ordinaires et
l'absence de sandbox ne sont donc pas en contradiction factuelle avec ce
résultat. Aucune correction du papier n'était justifiée.

## Recommandation de conception ultérieure (non implémentée)

1. **Échappement automatique dans `shell`.** Réduit le risque pour les chemins
   et valeurs simples, mais change le langage : les workflows qui attendent de
   l'expansion shell, des listes space-joined, des redirections ou du code dans
   les placeholders changeraient de comportement. Il faudrait une migration et
   probablement une syntaxe distinguant données et fragments de shell.
2. **Mode de commande à arguments typés.** Ajouter une forme distincte, par
   exemple programme + tableau `argv`, permet de transmettre les données sans
   `-c`. C'est la frontière la plus robuste, mais certains pipelines, pipes,
   redirections et substitutions doivent rester du shell explicite; les règles
   existantes ne peuvent pas être converties automatiquement en général.
3. **Mode strict opt-in.** Conserver `shell` historique et proposer un mode qui
   refuse les placeholders non sûrs, impose un alphabet/une API argv, ou exige
   un marquage explicite pour le texte shell. Compatibilité initiale meilleure,
   mais deux modèles à documenter et une migration volontaire restent
   nécessaires.

La recommandation est de concevoir une nouvelle surface argv typée ou un mode
strict opt-in, plutôt que de modifier silencieusement l'interpolation actuelle.
