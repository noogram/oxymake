# Re-gate final avant push — 2026-07-21

**Périmètre :** vérification ciblée des seuls items `REFUTED` de
`ops/audits/pre-push-review-2026-07-21.md`, après `6985e19`, `0608035` et
`24e0d06`. Le rapport source n'est plus présent à `HEAD`; la base de contrôle
a été lue depuis `c7c3e68:ops/audits/pre-push-review-2026-07-21.md`.

**Verdict global : PUSH-READY — NON.** Les corrections code, papier v3.2,
documentation, artefacts et gates demandées sont confirmées, mais l'item
bloquant d'attribution/confidentialité du rapport source n'est pas éteint : le
papier suivi, le PDF et le TeX du tarball exposent toujours l'identité et
l'affiliation privées, et l'historique conserve l'adresse privée. L'attribution
publique exigée est `Noogram`.

## Verdict binaire par item initialement `REFUTED`

| Item du rapport source | Éteint ? | Preuve ciblée |
|---|---:|---|
| 1.2 — cache partageable entre checkouts | **OUI, comme correction de portée ; capacité autonome toujours absente** | `DirectoryCache`, le papier, le changelog, `STATUS.md` et la référence CLI disent désormais explicitement « blob transport » et indiquent que l'index SQLite local doit voyager. Le test deux-checkouts copie intentionnellement `cache.db` (et WAL/SHM s'ils existent), puis obtient un hit. Il ne prétend donc plus tester une restauration depuis les seuls blobs. |
| 1.3 — frontière des chemins dans `key.rs` | **OUI** | Le format de clé passe à v4. Les chemins relatifs sont résolus depuis la racine d'invocation, `.`/`..` sont normalisés, les chemins existants sont canonicalisés et les sorties de racine restent absolues. Tests exacts présents pour chemin interne, `../`, préfixe homonyme et échappement par symlink. |
| T2 — content-addressing / formulations absolues | **OUI** | La discussion dit « same cache key » et qualifie machine, OS/architecture, disposition interne et politique de re-vérification. La conclusion dit « same declared inputs = same cache key on the same platform » et rappelle la politique explicite. |
| T5 — cache partagé | **OUI, comme correction de portée** | Toutes les surfaces vérifiées qualifient le backend répertoire de transport de blobs non autonome ; le manifeste distant reste explicitement du travail futur. |
| T6 — portabilité des décisions | **OUI** | La revendication porte désormais sur l'identité de clé sous les limites documentées, et les tests de frontière plus le test deux-checkouts couvrent la sémantique réellement livrée. |
| 2.3 — over-claims survivants | **OUI** | La conclusion ne contient plus « same caching decision, always » ; « on any machine » est limité à une machine de même OS/architecture et à la même disposition interne ; « source of truth for change detection is file content » est remplacé par la distinction clé dérivée du contenu / validation `mtime+hash`. Les autres occurrences de `always` trouvées concernent des mécanismes sans rapport (Makefile, stratégie `hash`, TOML, politique de matérialisation). |
| 2.4 — `ERRATUM.md` incomplet/inexact | **OUI** | Une entrée v3.2 est ajoutée en append et supersède explicitement les deux entrées v3.1 sur la portabilité et le cache partagé. Elle ajoute des sources primaires pour Snakemake, Bazel/Buck et Nix/Guix. |
| 3.2 — ADR-018 désaligné | **OUI** | L'amendement du 2026-07-21 conserve le tableau antérieur comme historique, constate le flag livré, marque G3 partiel faute de manifeste distant, précise la frontière v4 et remplace la revendication de binaire statique par la formulation attestée. |
| 3.3 — discipline de surface publique | **OUI** | `CHANGELOG.md` décrit la limite de l'index local ; `STATUS.md` catalogue `--cache-remote` comme livré mais instable ; `docs/book/src/reference/commands.md` documente le flag, la promotion vers `hash` et la limite du transport de blobs. |
| 4 — attribution/confidentialité | **NON — BLOQUANT** | `docs/paper/oxymake-paper.tex:90-92` contient encore « Emmanuel Sérié », CMAP, CNRS, École Polytechnique et Institut Polytechnique de Paris. Le texte extrait du PDF contient les mêmes lignes. Le tarball embarque un TeX byte-identical au TeX suivi, donc les contient aussi. Les commits concernés, dont `6985e19`, `0608035` et `24e0d06`, portent toujours `Emmanuel Sérié <emmanuel@serie.dev>`. `Noogram` n'est pas l'auteur public du papier. |
| 4 — propreté du diff / trailing spaces | **OUI** | `git diff --check 192e1a8^..HEAD` et `git diff --check` ne produisent aucune sortie ; les deux espaces finaux de `ops/audits/falsify-crusoe-v3.md` ont disparu. |
| 4 — CHANGELOG surévaluant l'inter-checkout | **OUI** | L'entrée dit désormais que le backend transporte seulement les blobs, que l'index ne voyage pas et qu'un checkout neuf exige aussi cet index. |

## Contrôles ciblés et artefacts papier

- `cargo test -p ox-cache` : **PASS**, 99 tests.
- `cargo test -p ox-cli --test cli directory_remote_cache_hits_from_a_second_identical_checkout -- --exact` : **PASS**, 1 test.
- TeX du tarball contre TeX suivi : **PASS**, byte-identical.
- `metrics.tex` du tarball contre fichier suivi : **PASS**, byte-identical.
- Reconstruction isolée du tarball par trois passes `pdflatex` : **PASS**.
- Texte du PDF reconstruit contre texte du PDF suivi : **PASS**, byte-identical.
- Grep du texte PDF : les formulations v3.2 qualifiées et « blob transport » sont présentes ; l'identité et l'affiliation privées le sont aussi.
- `git ls-files -ci --exclude-standard` : **PASS**, aucune sortie.

## Gates workspace

- `timeout 600 just check` (`cargo check --workspace`) : **PASS**.
- `timeout 600 just test` (`cargo test --workspace`) : **PASS** ; tests ignorés inchangés.
- `timeout 600 just lint` (`cargo clippy --workspace -- -D warnings`) : **PASS**.
- `timeout 600 just fmt-check` (`cargo fmt --all -- --check`) : **PASS**.

## Décision

**PUSH-READY : NON.** Le seul item `REFUTED` encore actif est bloquant :
l'attribution/confidentialité. Avant push, remplacer l'auteur et l'affiliation
privés des artefacts publics par `Noogram`, reconstruire PDF et tarball, puis
réécrire localement l'identité des commits concernés avec l'identité publique
autorisée. Aucun push n'a été effectué pendant cette re-gate.
