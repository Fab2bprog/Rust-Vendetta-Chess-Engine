# Changelog — Vendetta Chess Motor

Toutes les évolutions notables du projet sont consignées ici.
Format inspiré de [Keep a Changelog](https://keepachangelog.com/fr/),
versionnage sémantique [SemVer](https://semver.org/lang/fr/).

---

## [1.1.2] — 2026-06-26

Renommage du projet en **Vendetta Chess Motor** (ancien nom : « VendettaChess » ;
crate `vendetta_chess` → `vendetta_chess_motor`, binaire principal idem). Mise en
place d'un cadre de mesure d'Elo et durcissement de l'outillage de test.

### Ajouté
- **Outil de test SPRT en self-play** (binaire `selfplay`) — mesure objective du
  gain d'Elo d'une modification, en faisant jouer le moteur contre lui-même (deux
  variantes A/B), parallélisé, **zéro dépendance** (parsing/PRNG/SPRT maison).
  Voir `COMMENT_TESTER_SPRT.md`.
- **Interrupteurs de recherche à l'exécution** regroupés dans `FeatureToggles`
  (`SearchInfo.toggles`) — permettent d'isoler une heuristique pour un test A/B
  sans recompiler ni impacter le jeu normal (coût NPS nul) : `improving`,
  `futility`, LMR enrichie, Correction History, king attack.
- **Sécurité du roi par l'attaque** (king attack) dans l'évaluation — danger
  non-linéaire pondéré par type de pièce sur la zone du roi adverse, fusionné
  dans la passe mobilité (coût NPS minime). Réglage volontairement conservateur
  (testé SPRT, ~+3 Elo).
- **Handler UCI `register`** (no-op : moteur sans protection anti-copie) →
  couverture de l'**intégralité** des commandes de la spec UCI.

### Modifié
- **Correction History reworkée** — apprentissage pondéré par la profondeur (en
  point fixe) et **plusieurs tables combinées** (structure de pions, pièces
  non-pion par couleur, continuation), au lieu d'une seule table à pas fixe.

### Corrigé / Robustesse
- `selfplay` : `concurrency` borné (`clamp(1, 64)`) — évite tout risque d'OOM
  avec une valeur de config absurde.
- `selfplay` : anti-spam d'affichage — la progression n'est ré-imprimée que
  lorsque le compteur de parties avance réellement.
- `selfplay` : commentaire d'en-tête obsolète corrigé (pointe sur
  `COMMENT_TESTER_SPRT.md`).
- Audit robustesse/crash/propreté du 2026-06-26 — aucun bug critique trouvé ;
  voir `AUDIT_STABILITE_2026-06-26.md`.

---

## [1.1.0] — 2026-06-24

Version d'enrichissement et de durcissement : deux fonctionnalités de recherche
réactivées correctement, un bug de crash corrigé, sept optimisations de vitesse
(NPS) à comportement strictement identique, et une passe de propreté du code.
Rétrocompatible avec 1.0.0 (aucun changement d'interface UCI).

> ⚠️ **À re-valider après cette version** (le code de génération et de recherche
> a évolué) : `cargo test -- --include-ignored`, perft 6/6, et un match A/B
> avant/après pour chiffrer le gain Elo réel.

### Corrigé
- **Crash sur coup UCI non-ASCII** — `parse_move_uci` découpait les tokens par
  octets ; un token contenant un caractère multi-octets (ex. un emoji) pouvait
  paniquer. Rejet propre désormais (`!mv_str.is_ascii()`), avec test de
  non-régression.
- **Drapeau "improving" (RFP/LMP/NMP)** — réactivé après correction de la cause
  racine : `eval_history[ply]` est désormais écrit à chaque visite réelle du
  nœud (sentinelle en échec), sauf en recherche Singular Extension. L'invariant
  garantit que `eval_history[ply-2]` reflète toujours l'ancêtre du chemin
  courant (technique standard `ss->staticEval`).

### Ajouté
- **Gestion de l'échec en quiescence** — quand le camp au trait subit un échec,
  plus de stand-pat (illégal) : génération de toutes les évasions et détection
  du mat. Coût maîtrisé (déclenché uniquement sur les nœuds réellement en échec).
  La génération de contre-échecs silencieux reste volontairement non faite.
- **Préchargement de la table de transposition** (`prefetch`) — la ligne de
  cache du slot TT est préchargée juste après `make_move` (`prfm` sur aarch64,
  `_mm_prefetch` sur x86-64).
- **Cache de structure de pions** (pawn hash) thread-local — l'évaluation des
  pions (doublés/isolés/passés) est mémorisée par paire de bitboards de pions.
- **Futility Pruning par coup** (`alpha_beta`) — élague un coup silencieux ne
  donnant pas échec, à faible profondeur (≤ 6), si `static_eval + 100×depth ≤
  alpha`. Complémentaire du Razoring (niveau nœud) et de la LMP (nombre de
  coups). ⚠️ **Heuristique à valider par match SPRT** avant d'être considérée
  comme acquise (marges conservatrices par défaut ; désactivable via
  `FUTILITY_MAX_DEPTH = 0`).
- **LMR enrichie** (`alpha_beta`) — la réduction Late Move Reduction, jusque-là
  fonction de la seule profondeur × rang du coup, est ajustée de ±1 selon des
  signaux supplémentaires : +1 si la position ne s'améliore pas, −1 aux nœuds PV
  (fenêtre large), −1 pour un killer/countermove (bornée à r ≥ 1). ⚠️ **Ajustements
  à valider par match SPRT** (petits et conservateurs par défaut).
- **Correction History** (`SearchInfo` + `alpha_beta`) — l'évaluation statique
  d'un nœud est corrigée selon l'écart historique éval↔recherche observé pour des
  positions de MÊME structure de pions (table par thread, indexée [couleur][clé
  de pions], correction bornée à ±64 cp). Tous les élagages en aval (RFP, NMP,
  futility, improving) profitent d'une éval mieux calibrée. ⚠️ **Heuristique à
  valider par match SPRT** ; désactivable via `CORRHIST_MAX = 0`. Version
  simplifiée (les nœuds à coupure bêta ne sont pas encore appris).
- **`.gitignore`** et **`CHANGELOG.md`**.

### Performance (NPS — sans aucun changement de résultat de recherche)
- **LTO + `codegen-units = 1`** dans le profil release.
- **`target-cpu=native`** via `.cargo/config.toml` (binaire non portable — à
  retirer pour une release distribuable).
- **Mailbox `piece_on[64]`** — `piece_at()` passe d'un scan de 12 bitboards à
  une lecture O(1), maintenu incrémentalement.
- **Listes de coups sur la pile (`MoveList`)** — suppression de l'allocation tas
  par nœud et de la contention de l'allocateur entre threads Lazy SMP.
- **Génération légale par détection des clouages** — chemin rapide sans
  make/unmake pour le cas courant, repli sûr pour les cas délicats (roi, roque,
  prise en passant, pièce clouée, échec), filet de vérification en build debug.

### Modifié
- **Taille de hash par défaut : 16 Mo → 32 Mo**, et **plafond max relevé de
  512 Mo à 32 Go** (option UCI `Hash`). Le défaut prudent protège les configs
  modestes ; le plafond élevé permet aux grosses machines d'allouer beaucoup de
  TT en analyse.
- **Repli gracieux à l'allocation de la table de transposition** (robustesse) —
  l'allocation est désormais faillible (`try_reserve_exact` au lieu d'un
  abandon du processus) : si un réglage `Hash` excède la mémoire disponible, la
  taille est réduite de moitié jusqu'à réussir (et en tout dernier recours la
  table actuelle est conservée), avec un message `info string`. Un réglage trop
  ambitieux ne peut plus faire planter le moteur.
- **Pile des threads de recherche portée à 8 Mio** (threads Lazy SMP **et**
  thread de recherche principal) — marge anti-débordement face aux tableaux de
  coups désormais alloués sur la pile. Aucun impact sur les performances.
- **Propreté du code** — retrait de code mort (`piece_bb`, `piece_bb_mut`,
  paramètre `qs_ply` vestigial), commentaires périmés rafraîchis, et passe
  `cargo clippy` (de 41 à 6 avertissements lib, les 6 restants étant des choix
  délibérés documentés).

---

## [1.0.0] — 2026-06

Première version stable et complète.

### Ajouté
- Moteur d'échecs complet en Rust pur, **zéro dépendance externe**, 100 %
  compatible **UCI**.
- Représentation par **bitboards** + **Magic Bitboards** (attaques O(1)),
  hachage **Zobrist** incrémental, make/unmake symétrique, validation FEN stricte.
- **Génération de coups légaux** validée **Perft 6/6** sur les positions de
  référence de la Chess Programming Wiki.
- Recherche **alpha-bêta** : Iterative Deepening, Aspiration Windows, PVS, LMR,
  Null Move Pruning, Razoring, Reverse Futility Pruning, Late Move Pruning,
  Mate Distance Pruning, Internal Iterative Reduction, Check & Singular
  Extensions, SEE, quiescence avec Delta Pruning.
- Heuristiques d'ordonnancement : TT, killers, history, countermove,
  continuation history.
- **Lazy SMP** (multi-threading, jusqu'à 768 threads), table de transposition
  lock-free.
- Évaluation : matériel, PST tapered, mobilité, centre, structure de pions,
  sécurité du roi, finales, menaces, tempo.
- **Texel Tuning v3** appliqué (calibrage K + 22 paramètres) — niveau de jeu
  estimé **~2 600 Elo** (validé par victoires contre Stockfish à Elo limité).
- 64 niveaux de difficulté, options UCI étendues.
