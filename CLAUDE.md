# CLAUDE.md — Contexte pour l'IA

Ce fichier est destiné à Claude (IA) pour reprendre le développement de Vendetta Chess Motor
sans avoir à relire l'intégralité du code source. Il doit être lu en priorité au début
de chaque nouvelle session de travail.

---

## État du projet

- **Version** : 1.1.2
- **Licence** : GPL-3.0-or-later
- **Langage** : Rust (edition 2021), zéro dépendance externe
- **Protocole** : UCI (Universal Chess Interface)
- **Plateforme principale** : Apple Mac Mini M2 Pro (10 cœurs, aarch64-apple-darwin)

### Validations effectuées

- **Perft 6/6 PASS** — génération de coups validée sur les 6 positions de référence
  de la Chess Programming Wiki à profondeur 4-5. Zéro bug de génération.
- **Benchmark** — Lazy SMP mesuré : ×7.75 sur 10 cœurs (NPS moyen : ~25M nps à 10 threads)
- **Compilation** — `cargo build --release --target aarch64-apple-darwin` sans warning
- **Niveau de jeu ~2 600 Elo** — confirmé par victoires réelles contre Stockfish
  à Elo limité 2100, 2300, 2500, après application du Texel Tuning v3

---

## ✅ Réactivation correcte des deux features (2026-06-23, Opus 4.8)

**Historique** : le 2026-06-22, deux features avaient été DÉSACTIVÉES après
une partie perdue contre Stockfish (analyse `python-chess` : `14.Bf4` faute
matérielle nette, évaluée à tort +1,60). Le 2026-06-23, elles ont été
**réimplémentées correctement** — le bug de fond de chacune est corrigé,
pas seulement contourné. **À VALIDER sur Mac** (build/tests non exécutables
dans l'environnement d'édition) : `cargo build --release && cargo test`,
puis la position FEN
`rn2nrk1/2q3pp/b1pbpp1B/p2pN3/1P1P4/2NBP1Q1/1PPK1PPP/R6R w - - 0 14`
(`go movetime 3000`) — vérifier que `Bf4` n'est plus choisi / plus évalué
+1,60, et idéalement un match self-play ou vs Stockfish pour confirmer
l'absence de régression Elo.

1. **Drapeau "improving" (RFP/LMP/NMP) — RÉACTIVÉ et corrigé.**
   Cause racine du bug : `eval_history[ply]` n'était écrit QUE quand une éval
   statique existait (donc PAS en échec). Un nœud en échec laissait à cet
   index une valeur d'une AUTRE branche explorée plus tôt au même ply ; un
   descendant à ply+2 la lisait comme celle de son grand-parent.
   **Correctif** (`alpha_beta()`) : écrire `eval_history[ply]` à CHAQUE visite
   réelle, INCONDITIONNELLEMENT (vraie éval, ou sentinelle `EVAL_HISTORY_NONE`
   en échec/racine), SAUF pendant une recherche Singular Extension
   (`excluded_move` non nul — même ply, ne doit pas écraser le nœud
   englobant). Invariant alors garanti : `eval_history[ply-2]` renvoie
   toujours l'éval de l'ancêtre 2 plies plus haut sur LE chemin courant (les
   descendants n'écrivent qu'aux index ≥ ply+1). C'est la technique standard
   (pile `ss->staticEval` de Stockfish). `improving` redevient fiable ;
   RFP/NMP consomment leurs formules d'origine, `lmp_threshold()` réutilise
   son paramètre (coefficient 2 si improving, 1 sinon).

2. **Gestion de l'échec en quiescence — RÉACTIVÉE sous forme sûre.**
   L'ancienne tentative générait `generate_legal_moves()` à CHAQUE feuille
   (qs_ply==0) pour chercher des coups silencieux DONNANT échec : prohibitif,
   d'où la régression. La nouvelle version implémente la partie réellement
   importante et bien moins coûteuse : si le camp au trait SUBIT un échec à
   l'entrée de `quiescence()`, pas de stand-pat (illégal), génération de
   TOUTES les évasions légales (roi, interpositions, capture du donneur) et
   détection du mat. Ce bloc ne se déclenche QUE sur les nœuds réellement en
   échec (faible fraction), pas à chaque feuille — coût maîtrisé. La
   génération de contre-échecs silencieux reste volontairement NON faite
   (réserver à une itération mesurée). Borne `MAX_QUIESCENCE_PLY` placée en
   tête de fonction pour garantir la terminaison du cas échec.

---

## ⚡ Optimisations NPS (2026-06-23) — pure vitesse, zéro Elo perdu

Quatre optimisations appliquées pour augmenter le NPS **sans changer un seul
résultat de recherche** (donc zéro Elo perdu, et même un gain à temps fixe).
**À VALIDER sur Mac** : `cargo build --release` (zéro warning attendu) +
`cargo test` + perft **6/6** (la génération de coups est inchangée — perft est
le garde-fou de correction du mailbox et des MoveList).

1. **Profil release** (`Cargo.toml`) : `lto = true` + `codegen-units = 1`.
   Optimisation inter-modules, ~+5-15 % NPS. (`panic = "abort"` volontairement
   omis : interfère avec `cargo test` et le `panic!` de magic.rs.)
2. **`target-cpu=native`** (`.cargo/config.toml`) : instructions spécifiques au
   M2 Pro. ⚠️ Binaire non portable → retirer ce fichier pour une release GitHub.
3. **Mailbox `piece_on[64]`** (`board/state.rs`) : `piece_at()` passe d'un scan
   linéaire de 12 bitboards à une lecture O(1). Maintenu incrémentalement dans
   `place_piece`/`remove_piece` (seuls points de mutation vérifiés ;
   `piece_bb_mut()` inutilisé). `debug_assert` de cohérence mailbox↔bitboards
   dans `piece_at` (revérifié à chaque perft/test, zéro coût en release).
4. **`MoveList` sur la pile** (`moves/mod.rs`) : type à capacité fixe
   (`[Move; 256]`, zéro dépendance) remplaçant `Vec<Move>` sur le chemin chaud.
   `generate_legal_moves_into` / `generate_legal_captures_into` (zéro alloc)
   pour alpha_beta/quiescence ; wrappers `Vec` conservés pour binaires/tests/UCI
   (signatures publiques inchangées). Supprime l'allocation tas par nœud — et
   surtout la **contention de l'allocateur** entre threads Lazy SMP. Les threads
   secondaires passent à une **pile de 8 Mio** (`search/mod.rs`,
   `Builder::stack_size`) pour absorber sans risque les tableaux de coups sur
   pile en récursion profonde (repli gracieux si la création du thread échoue).
   Note : `MoveList::new()` initialise le tampon (sûr) ; si un profilage montrait
   que ce coût domine, le passage à `MaybeUninit` (encapsulé) est le pas suivant.
5. **Préchargement TT** (`transposition.rs::prefetch` + appel dans `alpha_beta`) :
   juste après `make_move`, `board.hash` est le hash de l'enfant ; on précharge
   la ligne de cache du slot TT correspondant (`prfm pldl1keep` sur aarch64,
   `_mm_prefetch` sur x86-64, no-op ailleurs) pour qu'elle soit chaude au probe()
   de l'enfant. La latence mémoire (TT 64 Mio, souvent hors cache) est masquée
   par le calcul d'extension/LMP qui suit. Le `unsafe` est sûr par construction :
   les instructions de préchargement sont des indications qui ne fautent jamais
   et ne modifient aucun état (cohérent avec les `unsafe impl Send/Sync` déjà
   présents dans ce fichier). La quiescence ne sonde pas la TT → pas concernée.
6. **Pawn hash table** (`eval/pawns.rs`) : cache thread-local de l'éval de
   structure de pions (doublés/isolés/passés), qui ne dépend QUE des bitboards
   de pions — or les pions bougent rarement, donc taux de hit très élevé. CLÉ =
   la paire de bitboards (blanc, noir) vérifiée par comparaison EXACTE → aucune
   fausse correspondance possible (au pire un remplacement, jamais une valeur
   fausse). Valeur cachée = score blanc-relatif, orientation par le trait
   appliquée APRÈS → résultat strictement identique (zéro Elo). AUCUNE modif de
   make/unmake/Board (volontairement, pour ne pas toucher la machinerie du hash
   incrémental) : tout est contenu dans `pawns.rs`. Cache thread-local (un par
   thread Lazy SMP, sans synchronisation), jamais besoin d'être vidé (une
   structure de pions donnée a toujours le même score). 8192 entrées ≈ 192 Kio
   par thread.
7. **Génération légale par clouages** (`moves/mod.rs`, audit NPS #8 — le plus
   gros gain de génération, et le plus délicat). `filter_legal_into()` remplace
   le make/unmake systématique par coup. `pinned_pieces()` calcule UNE fois par
   nœud le bitboard des pièces clouées (méthode par retrait de bloqueur, EXACTE
   quand le roi n'est pas en échec). Un pseudo-coup est alors LÉGAL sans
   vérification s'il n'est ni en échec, ni un coup de roi, ni un roque, ni une
   prise en passant, ni le coup d'une pièce clouée — le cas courant (>90 % des
   coups). Les cas délicats gardent le chemin sûr make/unmake (roque toujours
   validé par is_castling_legal, en passant pour l'échec à la découverte
   horizontal). **Seule l'erreur « clouage manqué » serait dangereuse (coup
   illégal), donc FILET DE SÉCURITÉ en build DEBUG** : chaque décision du chemin
   rapide est revérifiée contre make/unmake → toute divergence fait échouer
   `cargo test` / perft AVANT toute partie. **À VALIDER impérativement sur Mac :
   `cargo test -- --include-ignored`** (perft profond, des millions de positions,
   avec le filet actif) AVANT de jouer. En release le chemin rapide ne fait
   aucun make/unmake (le gain). Résultat de génération identique → zéro Elo, juste
   plus rapide.

---

## Binaires disponibles

```
cargo run --release --bin vendetta_chess_motor        # moteur UCI
cargo run --release --bin perft                 # validation génération de coups
cargo run --release --bin benchmark             # mesure des performances
cargo run --release --bin extract_positions     # Texel Tuning étape 1 : PGN → positions
cargo run --release --bin tuner                 # Texel Tuning étape 2 : coordinate descent
cargo run --release --bin selfplay              # test SPRT (mesure d'Elo, voir section dédiée)
cargo test                                      # tests rapides (< 10 s)
cargo test -- --include-ignored                 # suite complète avec tests lents
```

Outil annexe **hors du dépôt vendetta_chess_motor** (racine du dossier projet,
compilé directement via `rustc -O filter_pgn.rs -o filter_pgn`) :
`filter_pgn.rs` — filtre un dump mensuel Lichess (.pgn.zst) sur Elo et
cadence, en amont de `extract_positions`.

---

## Tests SPRT — mesurer l'Elo d'une modif (outil maison)

Binaire `selfplay` (`src/bin/selfplay.rs`) : fait jouer le moteur contre
lui-même, deux variantes A (référence) et B (candidat), et conclut par un test
SPRT si la modif de B ajoute de l'Elo. **Self-play interne** (pas d'UCI, pas de
sous-processus), **zéro dépendance** (parsing, PRNG et SPRT faits main). Mode
d'emploi complet : `COMMENT_TESTER_SPRT.md`.

**Commandes** (depuis `vendetta_chess_motor/`) :
```
cargo build --release
cargo run --release --bin selfplay        # lit selfplay_config.txt
touch STOP                                # (autre terminal) arrêt propre + rapport final
cat rapport_selfplay.txt                  # lire le verdict
```
- Config : `selfplay_config.txt` (format `clé = valeur` : nodes_a/b, improving_a/b,
  games_max, elo0/elo1, alpha/beta, **concurrency**, report). PASS = garder la modif, FAIL = la retirer.
- **Parallélisme** : clé `concurrency` (défaut 4) = nb de parties jouées
  simultanément (chaque partie = 2 recherches mono-thread, donc 4 ≈ 8 cœurs sur
  le M2 Pro). N'affecte PAS le verdict, seulement la vitesse (~4× plus rapide).
  Implémenté dans `main()` de `selfplay.rs` : N threads ouvriers tirent des
  parties via un `AtomicU64` (game_counter), agrègent W/D/L dans des `AtomicU64`,
  le thread principal scrute toutes les 500 ms (SPRT / plafond / fichier STOP).
- Sortie : `rapport_selfplay.txt` (autosauvegardé en cours). Statut possible :
  SPRT_CONCLU_PASS / _FAIL / INTERROMPU / PLAFOND_ATTEINT.
- Fiabilité À VÉRIFIER d'abord : A-contre-A (Elo ~0, FAIL) et A-contre-affaibli
  (`nodes_b` bas → Elo très négatif, FAIL).

**Interrupteurs runtime** (pour faire coexister A et B dans le même binaire sans
recompiler — config keys `*_a`/`*_b` du selfplay). Tous REGROUPÉS dans la struct
`FeatureToggles` (`search/mod.rs`), accessibles via `info.toggles.*` :
- `info.toggles.disable_improving` → key `improving_a`/`improving_b`.
- `info.toggles.disable_futility`  → key `futility_a`/`futility_b`.
- `info.toggles.disable_lmr_tweaks` → key `lmr_a`/`lmr_b` (neutralise les ±1, garde la LMR de base).
- `info.toggles.disable_correction` → key `correction_a`/`correction_b` (éval brute, aucune correction lue/apprise).
- `info.toggles.disable_king_attack` → key `king_attack_a`/`king_attack_b` (terme éval : sécurité du roi par l'attaque). Câblé via `evaluate_opt(board, king_attack)` (eval/mod.rs), accumulé dans la passe mobilité.
Tous à false en jeu normal (zéro coût NPS, branches prédites). Pour tester une
AUTRE feature : ajouter un champ `disable_xxx` à `FeatureToggles`, une condition
`!info.toggles.disable_xxx` en tête du bloc, puis le câblage `SideState` +
`Config` + parsing + header/rapport dans `bin/selfplay.rs`.

### ⚠️ Features de recherche AJOUTÉES mais NON ENCORE VALIDÉES par SPRT

À trancher une par une (chacune désactivable, voir le code) — tant qu'elles ne
sont pas mesurées, elles pourraient TIRER L'ELO VERS LE BAS sans qu'on le voie :

1. **`improving`** (RFP/NMP/LMP) — réactivé ; toggle runtime `disable_improving`.
2. **Futility Pruning par coup** — ✅ VALIDÉ SPRT (+49 Elo @10k nœuds, PASS). Gardé.
3. **LMR enrichie** (improving/PV/killer) — testée SPRT : +6.5 ±8.2 @10k nœuds (penche positif, non concluant). Gardée. Toggle `disable_lmr_tweaks`.
4. **Correction History** (`CorrectionHistory` dans `search/mod.rs` + `alpha_beta`) —
   v1 (table pion seule, pas fixe) testée SPRT : -3 Elo, FAIL. **Reworkée** (A: update
   pondéré par la profondeur en point fixe ; B: 4 tables — pion, non-pion blanc/noir,
   continuation — combinées par moyenne pondérée). À RE-tester SPRT, idéalement à
   `nodes = 50000` (la corrhist brille en profondeur). Toggle runtime `disable_correction`.

Rappel méthodo : « code correct » ≠ « gagne de l'Elo ». NE PAS empiler d'autres
heuristiques de recherche sans les avoir validées par match.

---

## Conventions de code

- **Commentaires** : obligatoires et exhaustifs. Chaque fichier a un bloc d'en-tête,
  chaque fonction non triviale est documentée, les choix techniques sont expliqués
  là où ils sont faits.
- **Langue des commentaires** : français
- **Assertions** : `debug_assert!` uniquement (jamais `panic!` ou `unwrap()` en
  production). Les erreurs récupérables retournent `Option` ou `Result`.
- **Nommage** : snake_case Rust standard, noms explicites et complets.
- **Pas de dépendances externes** — Rust standard uniquement.

---

## Décisions techniques importantes

### Représentation

- Cases : `u8`, valeur 0-63, `sq = rang * 8 + colonne`
- Bitboards : `u64`, bit i = case i
- Magic Bitboards pour tour et fou (O(1)), initialisés via `OnceLock` au démarrage
- Zobrist hash incrémental — mis à jour dans `make_move`/`unmake_move`
- `piece_count[2][6]` maintenu incrémentalement dans `place_piece`/`remove_piece`
- `eval_mg` / `eval_eg` maintenus incrémentalement (évaluation O(1) à la racine)

### Génération de coups

- Approche make/is_in_check/unmake — pas de détection explicite des clouages
- `generate_legal_captures()` séparée de `generate_legal_moves()` pour la quiescence
  (évite ~30 make/unmake silencieux par nœud)
- Validation FEN des droits de roque : vérification roi + tour présents AVANT
  le calcul du hash Zobrist

### Évaluation

- **Menaces / pièces en prise** (`src/eval/threats.rs`) ajouté : deux
  signaux cumulables — (1) pièce attaquée par une pièce adverse de moindre
  valeur (pion→mineure/tour, mineure→tour, tout→dame), pénalité fixe
  indépendante de toute défense ; (2) pièce (hors pion/roi) attaquée ET non
  défendue ("en prise"), via `own_attack_bitboard()` comme proxy bon marché
  de défense (cases que je contrôle moi-même, SANS masquer mes propres
  pièces — contrairement à mobility.rs qui exclut `!own_pieces`). PAS de
  SEE complet (trop coûteux à chaque `evaluate()`) — juste des intersections
  de bitboards d'attaque, même niveau de simplicité que mobility/king_safety/
  center. Actif à TOUTE phase de la partie (pas désactivé en finale,
  contrairement à mobility/center/king_safety). Pas encore intégré au tuner
  v4 (`tuner.rs`) — écart connu, à combler dans une v5 éventuelle si ces
  pénalités s'avèrent utiles à calibrer plus précisément.
- **Tempo Bonus** ajouté (`TEMPO_BONUS: i32 = 10` dans `eval/mod.rs`) :
  simple constante ajoutée à la fin de `evaluate()` — la fonction est déjà
  "du point de vue du joueur qui a le trait" par convention, donc aucun
  calcul supplémentaire nécessaire. Pas encore dans le tuner v4, même écart
  connu que threats.rs.

### Recherche

- `MAX_PLY = 192` (128 + 64) — borne absolue anti-récursion infinie
- `MAX_QUIESCENCE_PLY = 256` — borne de la quiescence
- Check Extension bornée par `ply + 1 < MAX_PLY` (sinon boucle infinie à depth=4
  avec échecs perpétuels)
- LMR via table `OnceLock<[[i32; 64]; 64]>` précalculée au premier appel
- Tri des coups : pré-calcul O(N) des scores puis `sort_unstable_by` (pas de
  lazy selection sort — revert décidé pour la clarté)
- Politique TT par génération : `new_search()` incrémente le compteur,
  les entrées périmées sont remplaçables même à profondeur supérieure
- Lazy SMP jusqu'à 768 threads, variation de profondeur `t % 3` entre threads
- **Reverse Futility Pruning (RFP)** ajouté : `depth <= 6`, marge `120 * depth`
  côté bêta. `static_eval` calculé une seule fois (`static_eval_opt`), partagé
  avec Razoring pour éviter un double appel à `evaluate()`. Fail-hard : retourne
  `beta`, pas `static_eval` (cohérence avec le reste du fichier)
- **Razoring** — l'ancien bloc nommé "Futility Pruning" dans ce fichier a été
  renommé : la terminologie standard réserve "Futility Pruning" à un élagage
  PAR COUP (non implémenté ici), et appelle "Razoring" la coupe au NIVEAU DU
  NŒUD basée sur alpha que ce code effectue réellement (`depth <= 2`, marge
  `150 * depth`)
- **Countermove Heuristic** ajouté (`src/search/countermove.rs`,
  `CountermoveTable`) : indexée par (pièce, case d'arrivée) du DERNIER coup
  joué, dérivée via `board.piece_at(prev_move.to)` — pas de paramètre piece
  séparé à propager. Nouveau paramètre `prev_move: Move` ajouté à
  `alpha_beta()` (entre `history` et `info`) ; propagé à TOUS les appels
  récursifs (mv pour les enfants après un coup réel, `Move::NULL` après un
  coup nul ou à la racine, `prev_move` inchangé pour la vérification SE qui
  ne joue aucun coup). Score d'ordonnancement 750_000, entre killer 2
  (800_000) et l'history heuristic. Remis à zéro à chaque "go", comme les
  killers (pas d'`age()` comme l'history — un seul slot par clé).
  **Correctif audit robustesse (point 2)** : pour une promotion,
  `board.piece_at(prev_move.to)` lirait la pièce APRÈS promotion (ex: Dame)
  au lieu du Pion qui a réellement joué le coup. Cas particulier explicite
  ajouté (`prev_move.flags.is_promotion()` → `Piece::Pawn` directement, sans
  relire le plateau).
- **Late Move Pruning (LMP)** ajouté, implémenté APRÈS le Countermove
  Heuristic (dépendance volontaire : LMP n'est sûr que si l'ordre des coups
  est fiable). `depth <= LMP_MAX_DEPTH (8)`, seuil quadratique
  `4 + 2*depth²` coups examinés avant élagage total (pas de réduction comme
  LMR — le coup n'est PAS recherché). Désactivé en échec, si le coup donne
  échec, si le coup a reçu une extension, ou sur capture/promotion.
  **Correctif audit robustesse** : désactivé aussi sur un killer move
  (`killers.is_killer(mv, ply)`) ou un countermove
  (`countermoves.get(p, t) == mv`) — sans cette exemption, un coup déjà
  prouvé efficace ailleurs dans l'arbre pouvait être sauté simplement parce
  que plusieurs captures gagnantes le précédaient dans le tri.
- **Correctif audit robustesse — history.update_bad sur coups élagués** :
  nouveau tableau `lmp_pruned: [bool; MAX_MOVES]` marquant les coups sautés
  par le LMP. La boucle `history.update_bad()` (déclenchée par une coupure
  bêta sur un coup ultérieur) les exclut désormais — sans ce suivi, un coup
  jamais recherché était pénalisé comme s'il avait été essayé et avait échoué.
- **Mate Distance Pruning** ajouté : resserre `[alpha, beta]` aux scores de
  mat atteignables depuis le `ply` courant (`SCORE_MATE - (ply+1)` côté
  bêta, `-SCORE_MATE + ply` côté alpha), placé après la sonde TT et avant le
  dispatch vers la quiescence. Technique gratuite : aucune heuristique, aucune
  marge approximative — juste de l'arithmétique exacte sur les scores de mat.
  A nécessité de rendre `beta` mutable dans la signature (`mut beta: i32` —
  changement purement interne, `mut` sur un paramètre n'affecte jamais les
  sites d'appel en Rust, aucun des ~12 appels n'a eu besoin d'être modifié).
- **Internal Iterative Reduction (IIR)** ajouté : si `tt_move.is_null()` et
  `depth >= 4` et `excluded_move.is_null()`, `depth -= 1` avant la génération
  des coups — placé APRÈS le bloc Singular Extension (qui a besoin de la
  profondeur non réduite pour son propre calcul de `se_depth`) et AVANT la
  génération des coups (pour que la réduction se propage à toute la boucle
  de coups : profondeurs des enfants, extensions, stockage TT final). Même
  technique que pour `beta` : `depth` rendu `mut depth: i32` dans la
  signature, aucun site d'appel modifié. Remplace l'ancienne Internal
  Iterative Deepening (IID, jamais implémentée ici, jugée peu rentable une
  fois aspiration windows + TT en place) — l'IIR n'a besoin d'AUCUN appel
  récursif supplémentaire, contrairement à l'IID classique.
- **Drapeau "improving" (RFP/LMP/NMP)** — RÉACTIVÉ ET CORRIGÉ le 2026-06-23
  (voir l'encart "Réactivation correcte des deux features" en haut de ce
  fichier). Compare l'éval statique du nœud courant à celle d'il y a 2 plies
  (même camp au trait — le trait alterne à chaque ply, donc ply et ply-2 sont
  TOUJOURS le même camp, comparaison directe valide). Stocké dans
  `SearchInfo::eval_history: [i32; MAX_PLY]`, désormais écrit à CHAQUE visite
  réelle du nœud (INCONDITIONNELLEMENT : vraie éval ou sentinelle
  `EVAL_HISTORY_NONE = i32::MIN` en échec/racine), SAUF en recherche Singular
  Extension (`excluded_move` non nul, même ply). C'est ce qui rend
  `eval_history[ply-2]` fiable (toujours l'ancêtre du chemin courant) et
  corrige le bug d'origine. `improving = false` par défaut si donnée
  manquante (réglage le PLUS PRUDENT).
  Effets, chacun raisonné indépendamment (PAS une formule copiée sans
  réflexion) :
    - RFP : marge `120 * (depth - improving as i32)` — plus petite donc
      coupe plus facilement quand la position s'améliore (le score est
      "confirmé" par la tendance) ; marge complète sinon.
    - NMP : réduction `4` si improving, `3` sinon (`.max(0)` pour éviter une
      profondeur négative passée à l'enfant — possible à depth==3 avec R=4).
    - LMP : `lmp_threshold(depth, improving)` — coefficient quadratique
      inchangé (2) si improving (réglage déjà validé), réduit à 1 sinon
      (élague plus tôt, faute de raison de croire qu'un coup tardif sauve
      une position qui ne progresse pas).
    - Razoring délibérément NON touché — l'utilisateur a nommé spécifiquement
      RFP/LMP/NMP, pas Razoring ; cohérent avec la direction inverse que
      prendrait l'ajustement côté alpha (voir discussion, non implémentée).
- **Gestion de l'échec en quiescence** — RÉACTIVÉE sous forme sûre le
  2026-06-23 (voir l'encart "Réactivation correcte des deux features" en haut
  de ce fichier). L'ancienne tentative (coups silencieux DONNANT échec
  recherchés à `qs_ply == 0` via `generate_legal_moves()` à chaque feuille)
  était prohibitive et a été abandonnée. La version actuelle traite le cas
  vraiment important : si le camp au trait SUBIT un échec à l'entrée de
  `quiescence()`, pas de stand-pat (illégal), génération de TOUTES les
  évasions légales et recherche complète, plus détection du mat
  (`-(SCORE_MATE - ply)` si aucune évasion). Ne se déclenche QUE sur les
  nœuds réellement en échec (faible fraction) — coût maîtrisé, contrairement
  à l'ancienne version active à chaque feuille. Borne `MAX_QUIESCENCE_PLY`
  déplacée en tête de fonction pour garantir la terminaison de ce cas (pas de
  détection de répétition en quiescence). Le paramètre `qs_ply: u32` reste
  dans la signature (propagé `qs_ply+1` en récursion). La GÉNÉRATION de
  contre-échecs silencieux reste volontairement NON faite (nécessiterait une
  détection d'échec bon marché + un banc d'essai NPS/Elo).
- **Continuation History** ajoutée (`src/search/continuation_history.rs`,
  `ContinuationHistoryTable`) : généralisation cumulative du Countermove —
  indexée [pièce_adverse][case_adverse][pièce][case_arrivée], 6×64×6×64 =
  147 456 entrées ≈ 576 Kio. Stockée en `Vec<i32>` à plat (PAS en tableau
  imbriqué `[[[[i32; 64]; 6]; 64]; 6]`) : à cette taille, un tableau imbriqué
  construit par valeur risquerait une grosse allocation temporaire sur la
  pile — un `Vec` est alloué sur le tas dès `vec![0; N]`, aucun risque quelle
  que soit la taille. Vieillie comme l'history (`age()`, pas `clear()`) à
  chaque "go" — contrairement au countermove qui n'a qu'un seul slot et est
  remis à zéro. Intégrée comme bonus ADDITIF au score history existant dans
  `move_score()` (pas un palier de priorité séparé) : `history.get(...) +
  cont_history.get(prev_piece, prev_to, piece, mv.to)`. Nouveau paramètre
  `cont_history: &mut ContinuationHistoryTable` ajouté à `alpha_beta()`
  (entre `countermoves` et `prev_move`) et à `move_score()` — propagé à
  TOUS les sites d'appel (6 récursifs internes + 4 dans mod.rs + 2 dans
  benchmark.rs + 1 construction directe de `SearchEngine` dans uci/mod.rs,
  cette dernière déjà oubliée une fois lors de l'ajout du countermove —
  vérifiée en priorité cette fois via `grep "SearchEngine\s*\{"` sur tout
  le projet).
- **Contempt Factor** ajouté : nouvelle fonction `draw_score(contempt, ply)`
  dans `alphabeta.rs`, qui remplace les 4 `return SCORE_DRAW;` (règle des 50
  coups, répétition, matériel insuffisant, pat). 0 par défaut = exactement
  `SCORE_DRAW`, comportement inchangé. Dérivation par parité de `ply` (pas
  besoin de connaître la couleur du moteur explicitement) : `ply` pair → ce
  nœud EST le camp racine → `-contempt` directement ; `ply` impair → camp
  adverse → `+contempt`, qui devient `-contempt` après l'inversion négamax
  impaire. Résultat : la racine perçoit TOUJOURS `-contempt` (nullité
  légèrement défavorable), quel que soit l'endroit de l'arbre où elle est
  détectée.
  **Point de vigilance traité explicitement** : la TT est partagée entre
  threads Lazy SMP — `info.contempt` DOIT être identique partout, sinon des
  scores de nullité incohérents seraient mis en cache selon le thread qui
  les a calculés. `SearchEngine::search()` copie `config.contempt` sur le
  thread principal ET sur chaque thread secondaire (capturé par valeur avant
  le `spawn`, `i32` étant `Copy`).
  Câblage UCI complet : nouveau champ `UciEngine::contempt` (défaut 0),
  option `"Contempt" type spin default 0 min -100 max 100` déclarée dans
  `cmd_uci()`, gérée dans `cmd_setoption()` (`.clamp(-100, 100)`), copiée
  dans `config.contempt` au moment du `Go` — exactement le même schéma que
  "Move Overhead".

### Magic Bitboards

- `find_magic()` borné à `MAX_MAGIC_ATTEMPTS = 100_000_000` avec `panic!` diagnostique
  (seul endroit où `panic!` est autorisé — bug de masque impossible à récupérer)

---

## Ce qui a déjà été corrigé (ne pas ré-auditer)

1. **Validation du roi dans `from_fen()`** — erreur si un roi manque
2. **Thread de recherche UCI** — la recherche tourne dans un thread séparé,
   stdin dans un thread dédié, signal d'arrêt via `Arc<AtomicBool>`
3. **Code mort `pawn.rs`** — supprimé
4. **`is_insufficient_material()`** — complété avec `piece_count`
5. **Validation des coups UCI** — vérification contre la liste légale avant `make_move`
6. **Pondering** — implémenté (ponderhit, stop pendant le ponder)
7. **hashfull, seldepth, lowerbound/upperbound, searchmoves, debug** — tous implémentés UCI
8. **`generate_legal_captures()`** — réécriture complète (captures seules)
9. **Évaluation incrémentale** — `eval_mg`/`eval_eg` dans Board
10. **`piece_count`** — incrémental dans Board
11. **Détection de répétition** — corrigée
12. **`PASSED_PAWN_MASK`** — précalculée via `OnceLock`
13. **Division par zéro `movestogo`** — protégée par `.max(1)`
14. **Rang de la case en passant** — validé dans `from_fen()`
15. **`bestmove (none)`** — au lieu de `bestmove 0000`
16. **Check Extension récursion infinie** — bornée par `ply + 1 < MAX_PLY`
17. **`find_magic()` boucle infinie** — bornée par `MAX_MAGIC_ATTEMPTS`
18. **Droits de roque FEN** — validés contre présence réelle des pièces
19. **PVS (Principal Variation Search)** — implémenté dans `alpha_beta()` : fenêtre
    nulle pour tout coup après le premier, re-recherche progressive (profondeur
    réduite → pleine profondeur → pleine fenêtre) si le coup dépasse alpha.
    **Bug corrigé après coup** : `.max(1)` appliqué à TOUTE la profondeur des
    coups non-PV (y compris sans réduction LMR) au lieu de seulement quand LMR
    s'applique réellement — cassait la parité de profondeur avec le coup n°1 et
    faisait jouer des coups sans intérêt en début de partie. Voir `full_depth`
    vs `probe_depth` dans le code pour la distinction correcte.
20. **Delta Pruning en quiescence** — filtre les captures dont le gain matériel
    maximal ne peut pas dépasser alpha, avant même de calculer le SEE
21. **Killer 1/2 différenciés** — légère préférence pour le killer le plus récent
    (810 000 vs 800 000 dans `move_score()`)
22. **Optimisations eval (calcul plus léger, contenu inchangé)** :
    - `phase.rs` : `piece_count` au lieu de `count_ones()` sur bitboards
    - `mobility.rs`/`center.rs` : fusionnés — un seul calcul d'attaque par
      pièce réutilisé pour mobilité ET centre (au lieu de deux calculs séparés)
    - `endgame.rs` : `passed_pawns_bb()` calculée une fois par couleur au lieu
      de jusqu'à 3 fois
23. **`#[inline]` abandonné** — tenté sur `piece_at()`, `probe()`, `store()` etc.
    (fonctions volumineuses très réutilisées) : a causé une RÉGRESSION de NPS
    par bloat de code/cache d'instructions, mesurée par l'utilisateur. Restauré
    depuis sauvegarde. **Ne pas retenter sans profiling réel** (pas d'intuition
    de taille de fonction).
24. **Singular Extension non bornée** — même défaut que la Check Extension
    avant sa correction (depth ne décroît pas, `ply + 1 < MAX_PLY` manquant).
    Trouvé lors d'un audit post-session systématique. Corrigé par la même garde.
25. **`MAX_PLY` incohérente entre fichiers** — `killers.rs` avait sa propre
    copie figée à 128, vs 192 dans `alphabeta.rs` (killer moves silencieusement
    désactivés au-delà de 128 plies). `killers.rs` importe désormais la
    constante d'`alphabeta.rs` (`pub(crate)`) — une seule source de vérité.
26. **Extensions UCI** (9 ajouts, voir `ARCHITECTURE.md` section uci pour le
    détail technique complet) :
    `UCI_LimitStrength`/`UCI_Elo`, `go nodes`/`go mate`, `MultiPV`
    (`search_multipv()`, réutilise `searchmoves` sans toucher `alpha_beta()`),
    `info currmove`/`currmovenumber`, `Move Overhead`, bouton `Clear Hash`,
    `UCI_AnalyseMode`, `UCI_EngineAbout`.
    **Bug corrigé en cours de route** : `info currmove` imprimait sans
    condition dès `ply == 0` dans `alpha_beta()`, polluant la sortie de
    `src/bin/benchmark.rs` (qui appelle `alpha_beta()` directement, hors
    couche UCI, pour mesurer le NPS brut). Corrigé via `SearchInfo.show_currmove`
    (false par défaut, activé uniquement par `SearchEngine::search()`).

---

## Structure des fichiers clés

```
Cargo.toml                  version = "1.1.2", license = "GPL-3.0-or-later"
src/uci/mod.rs              ENGINE_VERSION = "1.1.2"
src/board/state.rs          Board, make_move, unmake_move, from_fen, to_fen
src/board/magic.rs          find_magic(), init_magic_tables()
src/search/alphabeta.rs     alpha_beta(), quiescence(), MAX_PLY, MAX_QUIESCENCE_PLY
src/search/mod.rs           SearchEngine, Lazy SMP, compute_time_limit
src/search/transposition.rs TranspositionTable, politique par génération
src/moves/mod.rs            generate_legal_moves(), generate_legal_captures(), perft()
src/eval/mod.rs             evaluate(), is_insufficient_material()
src/eval/tables.rs          PST milieu/finale (données pures, zéro import Board)
src/bin/perft.rs            6 positions de référence, modes suite/position/divide
src/bin/benchmark.rs        NPS par palier de threads, scalabilité Lazy SMP
src/bin/extract_positions.rs  Rejoue le PGN (SAN via generate_legal_moves), échantillonne
src/bin/tuner.rs            EvalParams (342 params v4 : 22 scalaires + PST), calibrate_k()
```

---

## Texel Tuning — état d'avancement

**Valeurs v3 appliquées en production ET VALIDÉES par parties réelles.**
Victoires successives contre Stockfish à Elo limité 2100, 2300, puis 2500 —
niveau de jeu de Vendetta Chess Motor désormais estimé à **~2 600 Elo** (estimation
empirique basée sur ces résultats, pas un classement formel — voir README.md
section "Niveau de jeu estimé"). Pipeline complet et fonctionnel :
`filter_pgn.rs` (hors dépôt) → `extract_positions` → `tuner`. Voir
`ARCHITECTURE.md` section "Binaires de développement" pour le détail
technique complet.

- Base de données : dump Lichess mai 2026, filtré Elo≥2100 + Rapid/Classical
  → 302 971 parties → 302 864 rejouées avec succès (0 échec) → 2 464 785 positions
- Deux bugs de parsing PGN corrigés : (1) annotations `{ [%eval ...] [%clk ...] }`
  non retirées avant tokenisation ; (2) vérification du résultat final ("1-0" etc.)
  faite après le retrait du préfixe numérique au lieu d'avant

### Historique des tentatives — IMPORTANT pour ne pas répéter l'erreur

- **v1 (12 params : matériel + structure de pions)** — convergence en ~150
  passes, mais résultat **dégénéré** : toutes les valeurs de pièces divisées
  par ~2 de façon quasi uniforme, ET bonus de pions passés **négatifs** aux
  premiers rangs (impossible aux échecs). Cause initiale supposée : modèle
  trop pauvre.
- **v2 (22 params : + bishop pair, mobilité ×4, sécurité roi ×3, centre ×2)**
  — résultat presque IDENTIQUE à v1 (même effondrement, même signe négatif
  sur les pions passés). Ça a invalidé l'hypothèse "modèle trop pauvre" :
  un changement de signe ne s'explique pas par un simple effondrement
  d'échelle (qui rétrécirait vers zéro sans changer de signe).
- **Cause réelle identifiée** : `SIGMOID_SCALE` (K, l'échelle qui convertit
  l'eval en probabilité de victoire) était figée à 400 (valeur "historique")
  sans être calibrée sur CES données précises. La méthode Texel originale
  calibre K en premier — étape sautée dans v1/v2.
- **v3 (calibrage K + 22 params)** : `calibrate_k()` ajoutée — recherche
  ternaire sur K à paramètres fixes (valeurs de départ), AVANT la boucle de
  coordinate descent. Résultat : **K calibré = 748.22** (vs 400 historique).
  Avec ce K fixé, le tuning des poids converge vers des valeurs cohérentes :
  bonus de pions passés strictement positifs et croissants
  `[7, 8, 33, 75, 138, 218]`, rapports entre pièces sensés (Fou > Cavalier,
  Tour, puis Dame). **Leçon générale : toujours calibrer K avant de tuner
  les poids — un signe incohérent (pas juste une magnitude réduite) est le
  signal qu'il faut chercher pour détecter ce problème.**

### Valeurs appliquées en production (commit du jour)

| Fichier | Constante | Avant | Après (v3) |
|---|---|---|---|
| material.rs | PIECE_VALUE Cavalier/Fou/Tour/Dame | 320/330/500/900 | 216/224/382/817 |
| material.rs | BISHOP_PAIR_BONUS | 30 | 50 |
| pawns.rs | DOUBLED/ISOLATED_PAWN_PENALTY | -20/-20 | -24/-19 |
| pawns.rs | PASSED_PAWN_BONUS[1..6] | 5,10,20,35,60,100 | 7,8,33,75,138,218 |
| mobility.rs | KNIGHT/BISHOP/ROOK/QUEEN_MOBILITY_BONUS | 4/3/2/1 | 11/10/10/5 |
| king_safety.rs | SHIELD_PAWN_BONUS | 10 | 14 |
| king_safety.rs | KING_CENTER_PENALTY | -30 | **-7** (changement le plus marqué — à surveiller) |
| king_safety.rs | OPEN_FILE_NEAR_KING_PENALTY | -15 | -21 |
| center.rs | CENTER_PAWN_BONUS / CENTER_ATTACK_BONUS | 15/5 | 9/6 |

Perft re-vérifié après application : **6/6 PASS** (changement limité à
l'évaluation, génération de coups non affectée). **Validé** par victoires
réelles contre Stockfish à Elo limité 2100, 2300, 2500 — niveau de jeu
estimé à ~2 600 Elo. Point observé à surveiller : le roi blanc s'est montré
nettement plus actif/central en milieu et fin de partie dans les parties
analysées (cohérent avec `KING_CENTER_PENALTY` -30→-7) — sans poser
problème jusqu'ici, mais à garder à l'œil si des défaites inattendues
apparaissent.

### v4 — PST (en cours, code prêt, exécution en attente)

- **Portée** : Pion/Cavalier/Fou/Tour/Dame uniquement (5 × 64 = 320 nouveaux
  paramètres, total 342). **Roi délibérément exclu** : en production il a
  deux tables (MG/EG) sélectionnées par phase de partie ; le tuner n'a pas
  de détection de phase, donc une seule table tunée pour le Roi devrait soit
  être appliquée aux deux tables de production (perd la distinction
  abri/centralisation), soit nécessiter d'ajouter la détection de phase au
  tuner (chantier à part, réservé à une v5 éventuelle). Décidé explicitement
  avec l'utilisateur avant d'écrire le code.
- **Fidélité du modèle** : pour les 5 pièces tunées, AUCUNE approximation —
  en production ces 5 pièces utilisent déjà une seule table pour MG et EG
  (seul le Roi a deux tables, voir `eval/tables.rs::piece_square_values`).
- Valeurs de départ importées directement depuis `eval::tables` (pas de
  copie-collé manuel des constantes — élimine le risque de désynchronisation).
- Coût attendu : 342 paramètres contre 22 en v3 (~15,5× plus d'essais par
  passe) — temps de convergence nettement plus long, à mesurer empiriquement.
- Réutilise le fichier de positions déjà extrait (2 464 785 positions) — pas
  de nouvelle extraction PGN nécessaire.
- **Bug évité pendant l'implémentation** : `print_status()` bouclait sur
  `0..NUM_PARAMS` en indexant `PARAM_NAMES[i]` — avec NUM_PARAMS=342 et
  PARAM_NAMES.len()==22, ça aurait paniqué dès le premier appel. Corrigé en
  bouclant sur `NUM_SCALAR_PARAMS` ; les PST sont affichées séparément via
  `print_pst_table()`, uniquement dans le rapport final (pas périodiquement —
  320 valeurs toutes les 100 passes serait illisible).

---

## Chantiers futurs envisagés

- **Étendre le Texel Tuning aux PST** (384 valeurs) — v3 validée, c'est la suite logique
- **Mesure Elo plus rigoureuse** (Bayeselo, ordo) sur un grand nombre de
  parties — remplacer l'estimation empirique actuelle par un chiffre plus solide
- **NNUE** — réseau de neurones pour l'évaluation (objectif long terme)
- **CI/CD GitHub Actions** — compilation et tests automatiques à chaque push
- **CHANGELOG.md** — historique des versions

---

## Propriétaire du projet

Fabrice Garcia — fabgarcia2b@hotmail.fr
Le projet sera publié sur GitHub sous licence GPL-3.0.

`ENGINE_AUTHOR` (src/uci/mod.rs) et `authors` (Cargo.toml) doivent toujours
refléter ce nom — corrigé une fois après avoir été laissé par erreur à
"Vendetta Chess Motor Contributors" (générique). Le développement s'est fait en
coworking avec Claude (Anthropic) — voir README.md section "Remerciements"
pour la mention publique de cette collaboration.
