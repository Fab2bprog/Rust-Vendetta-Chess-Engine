# Architecture de Vendetta Chess Motor

> Version 1.1.2 · Licence GPL-3.0

## Vue d'ensemble

Vendetta Chess Motor est organisé en modules indépendants et testables séparément.
La dépendance entre modules suit une hiérarchie stricte pour éviter les cycles.
Aucune dépendance externe — Rust standard uniquement.

## Hiérarchie des modules

```
utils/types     → (aucune dépendance)
board           → utils
moves           → board, utils
eval            → board, utils
search          → moves, eval, board, utils
game            → board, moves, utils
uci             → search, game, board, moves, eval, utils
main            → uci
bin/perft       → moves, board, utils
bin/benchmark   → search, board, utils
```

## Modules

---

### utils

Types communs partagés par tous les modules.

- `types.rs` — Color, Piece, Square, Move, MoveFlags, constantes de score
  (SCORE_INF, SCORE_MATE, SCORE_DRAW)

---

### board

Représentation de l'échiquier par bitboards.

- `bitboard.rs` — type Bitboard (u64), opérations bit (set/clear/get/pop/lsb),
  masques de colonnes et rangs, tables d'attaque précalculées (cavalier, roi),
  fonctions d'attaque des pièces glissantes via Magic Bitboards (bishop_attacks,
  rook_attacks, queen_attacks) — O(1) par délégation à magic.rs
- `magic.rs`   — Magic Bitboards pour tour et fou : masques d'occupancy, nombres
  magiques trouvés au démarrage par essai aléatoire épars (< 10 ms, 128 magiques),
  tables plates heap-allouées (2 Mo tours + 256 Ko fous), API publique O(1)
  rook_attacks_magic() / bishop_attacks_magic(), thread-safe via OnceLock
- `state.rs` — struct Board (12 bitboards pièces + 2 bitboards d'occupation),
  CastlingRights, make_move / unmake_move, make_null_move / unmake_null_move,
  lecture/écriture FEN, hachage Zobrist incrémental, king_square(), piece_at(),
  `piece_count[2][6]` maintenu incrémentalement (détection matériel insuffisant),
  `eval_mg` / `eval_eg` maintenus incrémentalement (évaluation O(1)),
  `piece_on[64]` (mailbox `Option<(Piece,Color)>`) maintenu incrémentalement dans
  place_piece/remove_piece → piece_at() en O(1) au lieu d'un scan de 12 bitboards
  (debug_assert de cohérence avec un scan en build debug),
  validation FEN des droits de roque contre la présence réelle des pièces,
  hash Zobrist calculé après correction des droits (cohérence TT garantie)

---

### moves

Génération complète des coups légaux.

- `pawn.rs`   — poussées simples/doubles, captures, en passant, promotions
- `knight.rs` — coups des cavaliers (table précalculée)
- `bishop.rs` — coups des fous (pièce glissante)
- `rook.rs`   — coups des tours (pièce glissante)
- `queen.rs`  — coups des dames (fou + tour)
- `king.rs`   — coups du roi (table précalculée) + roques (légalité vérifiée)
- `mod.rs`    — generate_legal_moves(), generate_legal_captures()
                (captures seules, évite ~30 make/unmake silencieux par nœud quiescence),
                is_in_check(), is_square_attacked(), perft(), perft_divide()
                • `MoveList` : liste de coups à capacité fixe (`[Move; 256]`)
                  allouée sur la PILE (Deref vers `[Move]`), zéro allocation tas
                  par nœud — remplace `Vec<Move>` sur le chemin chaud
                • `generate_legal_moves_into()` / `generate_legal_captures_into()` :
                  variantes zéro-allocation (remplissent une MoveList fournie),
                  utilisées par la recherche ; les versions renvoyant `Vec` sont
                  conservées comme wrappers pour binaires/tests/UCI
                • `filter_legal_into()` + `pinned_pieces()` : filtrage légal
                  accéléré — chemin rapide SANS make/unmake pour le cas courant
                  (pas en échec, pièce non clouée, ni roi/roque/en passant), chemin
                  sûr make/unmake pour les cas délicats. Filet de sécurité en build
                  debug (revérification de chaque décision rapide contre make/unmake)

---

### eval

Évaluation statique d'une position. Score en centipions, positif = bon pour
le joueur actif.

- `material.rs`   — valeurs des pièces (P=100, N=320, B=330, R=500, Q=900, K=20000),
                    bonus paire de fous (+30), piece_value()
- `position.rs`   — tables de positions (PST) pour milieu de partie et finale,
                    interpolées par la phase (tapered eval)
- `pawns.rs`      — pions doublés (-24), pions isolés (-19),
                    pions passés (valeurs Texel v3 selon avancement).
                    Pawn hash table THREAD-LOCAL : cache de l'éval de structure
                    de pions, clé = paire de bitboards de pions (vérif exacte,
                    zéro fausse corrélation), valeur blanc-relative. L'éval ne
                    dépend que des pions → taux de hit élevé, jamais besoin de
                    vider le cache
- `king_safety.rs`— bouclier de pions (+10/pion), pénalité roi au centre (-30),
                    colonnes ouvertes près du roi (-15/colonne)
- `phase.rs`      — GamePhase, compute_phase() basé sur matériel restant,
                    is_endgame(), middlegame_factor(), taper()
- `mobility.rs`   — bonus de mobilité par case accessible (N=4, B=3, R=2, Q=1)
- `center.rs`     — contrôle du centre (d4/d5/e4/e5) : présence (+15),
                    attaques (+5) — désactivé en finale
- `endgame.rs`    — 6 critères de finale :
                    (1) Mop-up : pousser le roi adverse dans un coin (+500 cp requis)
                    (2) Tour sur la 7ème rangée (+25 cp)
                    (3) Tour derrière pion passé — règle de Tarrasch (+20 cp)
                    (4) Proximité du roi aux pions passés (dist_ennemi - dist_ami) × 5
                    (5) Fous de couleurs opposées : avantage réduit de 50 %
                    (6) Bonus d'avancement des pions passés (distance roi ennemi 1/2/3)
- `threats.rs`    — Menaces / pièces en prise : (1) pièce attaquée par une
                    pièce adverse moins chère (pénalité fixe) ; (2) pièce
                    attaquée et non défendue ("en prise", via bitboard
                    d'attaque propre comme proxy de défense). Pas de SEE
                    complet — contrôle de case bon marché. Actif à toute
                    phase (pas désactivé en finale)
- `mod.rs`        — evaluate() : somme pondérée de tous les critères,
                    is_insufficient_material()

---

### search

Algorithme de recherche avec toutes les heuristiques.

- `transposition.rs` — Table de transposition 64 Mo, lock-free via AtomicU64 (paires),
                        hachage Zobrist, TTFlag (Exact/LowerBound/UpperBound),
                        ajustement des scores de mat (store/probe),
                        politique de remplacement par génération (new_search() incrémente
                        le compteur de génération — les entrées périmées sont remplaçables
                        même à profondeur supérieure),
                        prefetch(hash) : préchargement de la ligne de cache du slot
                        (prfm aarch64 / _mm_prefetch x86-64 / no-op ailleurs), appelé
                        après make_move pour masquer la latence avant le probe enfant ;
                        allocation faillible (try_new via try_reserve_exact) avec repli
                        gracieux côté UCI — un réglage Hash trop grand réduit la taille
                        au lieu de planter (taille réglable jusqu'à 32 Go, défaut 32 Mo)
- `killers.rs`       — Killer moves : 2 coups silencieux par profondeur (max 64 niveaux)
- `history.rs`       — History heuristic : score [pièce][case_arrivée],
                        update_good() / update_bad() avec gravity
- `countermove.rs`   — Countermove heuristic : un coup de réfutation par
                        [pièce_adverse][case_arrivée_adverse], dérivé via
                        board.piece_at(prev_move.to) — un seul slot par clé
- `continuation_history.rs` — Continuation History : généralisation
                        cumulative du countermove, score [pièce_adverse]
                        [case_adverse][pièce][case_arrivée] (147 456 entrées,
                        Vec<i32> à plat — pas de tableau imbriqué, pour éviter
                        tout risque de pile à cette taille). Vieillie comme
                        l'history, pas remise à zéro comme le countermove
- `see.rs`           — Static Exchange Evaluation (SEE) :
                        évalue la séquence complète de captures sur une case,
                        LVA (Least Valuable Attacker) récursif avec option d'arrêt,
                        gestion des rayons X via occupancy dynamique,
                        gestion des promotions (pion → dame)
- `alphabeta.rs`     — Algorithme de recherche principal :
                        • move_score() : ordonnancement TT → SEE captures → promotions
                          → killers → history → captures perdantes
                        • order_moves() : pré-calcul des scores en O(N) puis tri —
                          évite les appels redondants à see() dans sort_unstable_by
                        • lmr_reduction() : réduction dynamique logarithmique
                          `1 + ln(depth) × ln(move_index) / 2`, via table OnceLock
                          précalculée au premier appel (évite les calculs float répétés)
                        • quiescence() : stand-pat, captures triées et filtrées par SEE ;
                          si le camp au trait SUBIT un échec → pas de stand-pat,
                          recherche de TOUTES les évasions légales + détection
                          du mat (réactivation sûre, voir CLAUDE.md ; la
                          génération de contre-échecs silencieux reste, elle,
                          non faite — coût). Profondeur bornée à
                          MAX_QUIESCENCE_PLY (= MAX_PLY + 64), garde en tête de fonction
                        • alpha_beta() avec :
                          - Détection de nulle (50 coups, répétition, matériel insuffisant)
                          - Sonde TT avec cutoffs Exact/LowerBound/UpperBound
                          - Reverse Futility Pruning / Static Null Move
                            (depth ≤ 6, marge 120×depth, coupe côté bêta)
                          - Razoring (depth ≤ 2, marge 150×depth, coupe côté
                            alpha — anciennement nommé "Futility Pruning"
                            dans ce fichier, renommé pour correspondre à la
                            terminologie standard)
                          - Null Move Pruning (R=3, anti-zugzwang)
                          - Singular Extension (depth ≥ 6, vérification à depth/2)
                          - Multi-cut (si SE-score ≥ beta sans le coup TT)
                          - Late Move Reduction dynamique (depth ≥ 3, move_index ≥ 3)
                            ENRICHIE (⚠️ ajustements à valider SPRT) : réduction
                            de base logarithmique, +1 si la position ne s'améliore
                            pas, −1 si nœud PV (fenêtre large), −1 si killer ou
                            countermove ; bornée à r ≥ 1
                          - Late Move Pruning (depth ≤ 8, seuil 4+2×depth²
                            coups — coup non recherché du tout, pas réduit ;
                            implémenté après le Countermove Heuristic dont
                            il dépend pour la fiabilité de l'ordre des coups ;
                            killers et countermoves explicitement exemptés)
                          - Futility Pruning par coup (depth ≤ 6, coupe un coup
                            silencieux ne donnant pas échec si static_eval +
                            100×depth ≤ alpha — ⚠️ marges À VALIDER PAR SPRT,
                            désactivable via FUTILITY_MAX_DEPTH = 0)
                          - Mate Distance Pruning (resserre alpha/beta aux
                            scores de mat atteignables depuis ce ply — exact,
                            pas une heuristique, placé avant le dispatch
                            quiescence)
                          - Internal Iterative Reduction (depth -= 1 si pas de
                            coup TT, depth ≥ 4 ; placé après Singular Extension,
                            avant la génération des coups ; remplace l'IID,
                            aucun appel récursif supplémentaire)
                          - Drapeau "improving" (RFP/NMP/LMP) — RÉACTIVÉ et
                            corrigé : eval_history[ply] écrit à chaque visite
                            réelle (inconditionnellement, sentinelle en échec),
                            sauf en recherche SE — l'invariant rend
                            eval_history[ply-2] fiable (ancêtre du chemin
                            courant). Voir CLAUDE.md, encart "Réactivation
                            correcte des deux features"
                          - Correction History (⚠️ à valider SPRT) : l'éval
                            statique est corrigée par la table SearchInfo
                            .correction_history (par thread, indexée [couleur]
                            [clé de structure de pions], bornée ±64 cp) ; apprise
                            en fin de nœud via (score − éval corrigée).
                            Désactivable par CORRHIST_MAX = 0
                          - Check Extension (+1 si le coup donne échec, depth ≤ 4,
                            borné par ply + 1 < MAX_PLY pour éviter toute récursion infinie)
                          - Mise à jour killers/history/countermove/continuation
                            history sur coupures bêta (history et continuation
                            history excluent les coups élagués par LMP via
                            lmp_pruned[])
                        • Constantes de sécurité :
                          MAX_PLY = 192 (128 + 64) — borne absolue de récursion
                          MAX_QUIESCENCE_PLY = 256 — borne de la quiescence
- `mod.rs`           — SearchEngine, Iterative Deepening, Aspiration Windows
                        (delta initial 50 cp, doublement sur fail),
                        Lazy SMP (jusqu'à 768 threads, pile de 8 Mio par thread
                        secondaire via Builder::stack_size — récursion profonde +
                        listes de coups sur la pile ; repli gracieux si la création
                        du thread échoue, Arc<TT> partagée,
                        Arc<AtomicBool> signal d'arrêt partagé avec le thread UCI,
                        variation de profondeur entre threads secondaires : t % 3),
                        gestion du temps (compute_time_limit, movestogo protégé
                        contre la division par zéro),
                        ponder : recherche infinie jusqu'à ponderhit ou stop,
                        64 niveaux de difficulté :
                          • skill_level_max_depth() : interpolation quadratique
                            de la profondeur max (1→1, 16→4, 32→7, 48→11, 64→∞)
                          • apply_skill_level() : probabilité d'erreur décroissante
                            (niveau 1 = 90 % aléatoire, niveau 57+ = 0 % d'erreur)

---

### game

Gestion de la partie en cours.

- `mod.rs`      — struct Game, historique des positions, coordination make/unmake
- `rules.rs`    — détection de nulle (50 coups, répétition, matériel insuffisant),
                  mat, pat
- `history.rs`  — historique des positions pour la détection de répétition à 3 fois

---

### uci

Protocole de communication UCI (Universal Chess Interface).

- `parser.rs` — analyse des commandes UCI :
                uci, isready, ucinewgame, position (startpos/fen + moves),
                go (movetime, wtime/btime/winc/binc, movestogo, depth,
                    infinite, ponder, searchmoves, nodes, mate), stop, ponderhit,
                setoption (Hash, Threads, Skill Level, Ponder, Debug,
                    UCI_LimitStrength, UCI_Elo, MultiPV, Move Overhead,
                    Clear Hash, UCI_AnalyseMode, Contempt, UCI_EngineAbout), quit
- `mod.rs`    — machine d'état UciEngine, thread stdin séparé (canal mpsc),
                thread de recherche (spawn_search), gestion du ponder,
                émission de info (depth, seldepth, score, nodes, nps, time,
                hashfull, lowerbound/upperbound, pv, currmove/currmovenumber,
                multipv) et bestmove

#### Extensions UCI (au-delà du strict minimum requis)

Ajoutées après audit de conformité — Vendetta Chess Motor respectait déjà toutes les
commandes obligatoires ; ces ajouts couvrent des cas d'usage et conventions
standards utiles à l'interopérabilité avec un plus grand nombre de GUIs/plateformes :

- **`UCI_LimitStrength` + `UCI_Elo`** (`search::elo_to_skill_level()`) —
  interpolation linéaire 600-2600 Elo → niveaux 1-64 (l'échelle de Skill
  Level existante). Permet aux GUIs/plateformes standards (ex : hébergement
  de bot Lichess) de brider Vendetta Chess Motor sans connaître l'option maison
  "Skill Level". Priorité dans la commande Go : `UCI_AnalyseMode` >
  `UCI_LimitStrength` > `Skill Level`.
- **`go nodes <x>`** — `SearchInfo.max_nodes`, vérifié dans `check_time()`
  aux côtés de la limite de temps (même fréquence : toutes les 4096 nœuds).
- **`go mate <x>`** — traduit en profondeur de recherche (2×x plies),
  réutilise le système de score de mat existant (`format_score()`).
- **`MultiPV`** (`SearchEngine::search_multipv()`) — réutilise le mécanisme
  `searchmoves` déjà existant pour exclure progressivement les meilleures
  lignes déjà trouvées, **sans modifier** `search()` ni `alpha_beta()`.
  Comportement par défaut (`multipv=1`) strictement inchangé. Limitation
  connue : les lignes "info depth..." intermédiaires de chaque ligne ne
  portent pas de champ "multipv" (seul le récapitulatif final par ligne
  l'inclut) — sans incidence sur le résultat affiché par la GUI.
- **`info currmove`/`currmovenumber`** — émis dans `alpha_beta()` à la
  racine (`ply == 0`) uniquement. Protégé par `SearchInfo.show_currmove`
  (false par défaut) : **bug corrigé en cours de route** — la première
  version imprimait sans condition dès `ply == 0`, polluant la sortie de
  `src/bin/benchmark.rs` qui appelle `alpha_beta()` directement (hors
  couche UCI) pour mesurer le NPS brut. Seul `SearchEngine::search()` (la
  vraie recherche pilotée par l'UCI) active ce drapeau.
- **`Move Overhead`** — remplace l'ancienne marge fixe de 50 ms codée en
  dur dans `compute_time_limit()` ; même valeur par défaut, désormais
  réglable (`SearchConfig.move_overhead`). Important en ligne/tournoi pour
  éviter une perte au temps due à la latence de communication.
- **`Clear Hash`** (bouton) — vide la TT immédiatement sans passer par
  `ucinewgame` (qui réinitialise aussi killers/history, inutile en cours
  de réflexion).
- **`UCI_AnalyseMode`** — force `skill_level = 64`, prioritaire sur tout
  autre bridage de force.
- **`Contempt`** (`alphabeta.rs::draw_score()`) — pénalise légèrement les
  positions nulles (50 coups, répétition, matériel insuffisant, pat) du
  point de vue du camp à la racine de la recherche. 0 par défaut =
  `SCORE_DRAW` exact, comportement inchangé. Dérivation par parité de `ply`
  (pair → racine directement, impair → adversaire, inversé par le négamax) —
  pas besoin de connaître la couleur du moteur. **Point de vigilance** :
  `info.contempt` est copié IDENTIQUE sur tous les threads Lazy SMP, la TT
  partagée stockerait sinon des scores de nullité incohérents selon le
  thread qui les a calculés.
- **`UCI_EngineAbout`** — chaîne d'information cosmétique (nom, version,
  auteur, licence), déclarée pour la conformité UCI mais sans effet
  fonctionnel.

---

## Représentation des cases

```
Square : u8, valeur 0 à 63
sq = rang * 8 + colonne
Colonne : 0=a, 1=b, ..., 7=h
Rang    : 0=rang1, 1=rang2, ..., 7=rang8
Exemple : e4 = rang 3, colonne 4 → sq = 28
```

## Représentation des coups

```rust
pub struct Move {
    pub from:      u8,         // case de départ (0–63)
    pub to:        u8,         // case d'arrivée (0–63)
    pub flags:     MoveFlags,  // Quiet | DoublePush | Castle* | Capture |
                               // EnPassant | Promotion | PromotionCapture
    pub promotion: u8,         // 0=aucune, 1=N, 2=B, 3=R, 4=Q
}
```

## Multi-threading : Lazy SMP

```
Thread principal
  ├── Board (clone)
  ├── SearchInfo (signal stop partagé)
  ├── KillerMoves (privé)
  ├── HistoryTable (privée)
  └── Arc<TranspositionTable>  ←──┐
                                   │ (partagée)
Thread secondaire ×N               │
  ├── Board (clone indépendant)    │
  ├── SearchInfo (signal stop)     │
  ├── KillerMoves (privé)          │
  ├── HistoryTable (privée)        │
  └── Arc<TranspositionTable>  ────┘
```

Les threads secondaires peuplent la TT à diverses profondeurs.
Le thread principal en bénéficie via les TT hits (meilleur ordonnancement,
coupures plus tôt).

## Flux de la recherche (simplifié)

```
go wtime ... btime ...
  └── SearchEngine::search()
        └── Iterative Deepening (depth 1, 2, 3, ...)
              └── Aspiration Windows [prev_score ± 50]
                    └── alpha_beta(depth, alpha, beta, ply=0)
                          ├── Détection nulle / TT probe
                          ├── Mate Distance Pruning (resserre alpha/beta, exact)
                          ├── Drapeau "improving" (éval vs ply-2, RFP/NMP/LMP)
                          ├── Reverse Futility Pruning (depth ≤ 6, côté bêta)
                          ├── Razoring (depth ≤ 2, côté alpha)
                          ├── Null Move (depth ≥ 3)
                          ├── Singular Extension (depth ≥ 6)
                          ├── Internal Iterative Reduction (depth -= 1 si pas de coup TT)
                          └── Pour chaque coup (ordonné par SEE/killers/countermove/history)
                                ├── Check Extension
                                ├── Late Move Pruning (coup tardif → pas recherché)
                                ├── LMR (move tardif → depth réduite)
                                └── alpha_beta(depth-1+ext, ...)
                                      └── quiescence() si depth ≤ 0
                                            └── captures filtrées SEE ≥ 0
```

---

## Binaires de développement

Quatre binaires supplémentaires sont inclus dans `src/bin/`, destinés au
développement uniquement — ils ne font pas partie du moteur UCI livré à
l'utilisateur final.

### perft — Validation de la génération de coups

```
cargo run --release --bin perft
cargo run --release --bin perft -- "<fen>" <depth>
cargo run --release --bin perft -- divide "<fen>" <depth>
```

Compte les nœuds feuilles à profondeur N et compare aux valeurs de référence
de la Chess Programming Wiki. Un écart de 1 nœud révèle un bug précis dans la
génération (roque illégal, prise en passant manquée, clouage ignoré, etc.).
Le mode divide décompose le résultat par coup racine pour isoler la divergence.

**Résultat v1.0.0 : 6/6 PASS** sur l'ensemble des positions de référence.

### benchmark — Mesure des performances

```
cargo run --release --bin benchmark
cargo run --release --bin benchmark -- --time 5000 --threads 8
```

Lance une recherche alpha-bêta réelle pendant N secondes sur 5 positions types,
en faisant varier le nombre de threads (1, 2, 4, 8, N). Affiche le NPS et
le gain de scalabilité Lazy SMP à chaque palier.

**Résultat v1.0.0 sur Apple M2 Pro (10 cœurs) : ×7.75 sur 10 threads.**

### extract_positions — Texel Tuning étape 1

```
cargo run --release --bin extract_positions -- <entrée.pgn> <sortie.txt>
```

Rejoue chaque partie d'un fichier PGN avec le moteur lui-même (résolution
SAN via `generate_legal_moves` — pas de réimplémentation des règles),
échantillonne une position tous les 8 demi-coups après les 10 premiers
(théorie d'ouverture ignorée), et écrit `<FEN>;<résultat>` dans un fichier
texte intermédiaire. Sépare le coût de parsing PGN (fait une seule fois) du
coût de tuning (répété à chaque passe de coordinate descent).

Le fichier PGN d'entrée est lui-même préparé en amont par `filter_pgn.rs` —
un outil **autonome, hors du dépôt vendetta_chess_motor** (dans le dossier racine
du projet), qui filtre un dump mensuel Lichess (.pgn.zst) sur l'Elo des deux
joueurs et la cadence (Rapid/Classical), en décompressant en flux via le
sous-processus `zstd` — zéro dépendance Cargo ajoutée. Compilation directe :
`rustc -O filter_pgn.rs -o filter_pgn`.

### tuner — Texel Tuning étape 2 (calibrage K + coordinate descent)

```
cargo run --release --bin tuner -- <positions.txt>
```

Calibre automatiquement un sous-ensemble des constantes d'évaluation, en
deux phases :

**Phase 1 — calibrage de K (`calibrate_k()`)**
L'erreur Texel compare `sigmoïde(eval / K)` au résultat réel de la partie.
K doit être calibré sur les données AVANT de toucher aux poids — sinon
l'optimiseur compense un mauvais calibrage d'échelle en rétrécissant tous
les poids au lieu de les calibrer correctement (voir "Historique des
versions" ci-dessous, ce piège s'est concrètement produit). Recherche
ternaire sur K dans [50, 1000], à paramètres fixes (valeurs de départ du
moteur). K est ensuite **fixé** pour toute la phase 2.

**Phase 2 — coordinate descent sur les poids**
  1. Charge toutes les positions (FEN + résultat réel) en mémoire.
  2. Calcule l'erreur quadratique moyenne entre `sigmoïde(eval, K)` et le
     résultat réel, sur l'ensemble du jeu de données — en parallèle sur
     tous les cœurs disponibles (`std::thread::scope`, somme indépendante
     par tranche de positions, aucune coordination pendant le calcul).
  3. Pour chaque paramètre, essaie ±1 ; garde le changement s'il réduit
     l'erreur globale, sinon le rejette.
  4. Répète jusqu'à ce qu'une passe complète n'améliore plus aucun paramètre.
  5. Affiche le détail des 22 paramètres toutes les `PRINT_EVERY` passes
     (défaut 100), pour suivre la progression sans noyer la sortie.

**Portée v3 : 22 paramètres** — matériel (Cavalier/Fou/Tour/Dame, Pion fixé
à 100 comme ancre), pénalités pions doublés/isolés, bonus pions passés (6
paliers), paire de fous, mobilité (4 pièces), sécurité du roi (3 critères),
contrôle du centre (2 critères). Volontairement sans PST (384 valeurs)
pour l'instant — extension possible en v4.

Important : l'évaluation utilisée par le tuner (`tunable_eval_white_pov`)
est une réimplémentation simplifiée, **séparée** de `eval::evaluate()`. Le
moteur réel maintient le matériel et les PST de façon incrémentale
(`board.eval_mg`/`eval_eg`, mis à jour à chaque `place_piece`/`remove_piece`)
pour la performance en recherche — un choix incompatible avec le besoin du
tuner de recalculer le score pour des milliers de jeux de paramètres
candidats. Le tuner recalcule donc le matériel, la structure de pions, la
mobilité, la sécurité du roi et le centre directement depuis les bitboards
à chaque appel : plus lent, mais sans incidence (calcul hors-ligne, pas une
recherche en temps limité). Le moteur de production n'est pas touché par
ce fichier — seules les VALEURS finales en sont extraites, manuellement.

#### Historique des versions du tuner — pourquoi le calibrage K était nécessaire

- **v1 (12 paramètres, sans calibrage K)** : convergence en ~150 passes vers
  un résultat **dégénéré** — toutes les valeurs de pièces divisées par ~2 de
  façon quasi uniforme, et bonus de pions passés **négatifs** aux premiers
  rangs (impossible aux échecs : un pion passé n'est jamais une faiblesse).
- **v2 (22 paramètres, sans calibrage K)** : résultat presque identique à
  v1, ce qui a invalidé l'hypothèse initiale ("modèle trop pauvre") —
  ajouter des critères n'a rien changé au problème.
- **Diagnostic** : un effondrement d'échelle pur rétrécit les valeurs vers
  zéro SANS changer leur signe. Le fait que les bonus de pions passés
  passent du positif au négatif aux premiers rangs, de façon quasi
  identique entre v1 et v2, pointait vers une cause structurelle commune
  aux deux modèles plutôt qu'un manque d'expressivité : `SIGMOID_SCALE`
  était figée à 400 (valeur "historique") sans être calibrée sur ces
  données précises.
- **v3 (calibrage K + 22 paramètres)** : K calibré = **748.22** (vs 400).
  Avec ce K fixe, le tuning converge vers des bonus de pions passés
  strictement positifs et croissants, et des rapports de valeurs entre
  pièces cohérents avec la théorie classique.

**Valeurs v3 appliquées en production** (mesurées sur 2 464 785 positions,
302 864 parties Lichess Rapid/Classical Elo ≥ 2100, dump mai 2026) :

| Paramètre | Avant | Après v3 |
|---|---|---|
| Cavalier / Fou / Tour / Dame | 320 / 330 / 500 / 900 | 216 / 224 / 382 / 817 |
| Bonus paire de fous | 30 | 50 |
| Pions doublés / isolés | -20 / -20 | -24 / -19 |
| Pions passés (rangs 2-7) | 5,10,20,35,60,100 | 7,8,33,75,138,218 |
| Mobilité Cavalier/Fou/Tour/Dame | 4/3/2/1 | 11/10/10/5 |
| Bouclier de pions | 10 | 14 |
| Roi au centre | -30 | **-7** (changement le plus marqué) |
| Colonne ouverte près du roi | -15 | -21 |
| Centre — pion / attaque | 15 / 5 | 9 / 6 |

Perft re-vérifié après application : 6/6 PASS (changement limité à
l'évaluation).

**Validation confirmée par parties réelles** : après application, Vendetta Chess Motor
a battu Stockfish successivement réglé à 2100, 2300 puis 2500 Elo limité.
Niveau de jeu désormais estimé à ~2 600 Elo (voir README.md). La chute de
la pénalité "roi au centre" (-30 → -7) s'est traduite concrètement par un
roi blanc nettement plus actif en milieu/fin de partie dans les parties
observées — comportement risqué en théorie, mais qui n'a pas posé de
problème dans les parties jouées jusqu'ici (positions où Vendetta Chess Motor avait
déjà l'avantage). Point à surveiller si des défaites inattendues apparaissent.

**Mesures de performance sur Apple M2 Pro (10 cœurs) :** calibrage K ≈ 3.4s,
puis ~0.86s par passe en parallèle sur 2,46 millions de positions
(contre ~6s par passe en séquentiel — gain ×7, cohérent avec le nombre de
cœurs) ; convergence atteinte après 156 passes pour la v3.

---

## Chantiers futurs envisagés

- **Étendre le Texel Tuning aux PST** (384 valeurs) — le Texel Tuning v3
  étant désormais validé par parties réelles, c'est la suite logique
- **Mesure Elo plus rigoureuse** — outil de classification (Bayeselo, ordo)
  sur un grand nombre de parties, pour remplacer l'estimation empirique
  actuelle (~2 600 Elo sur la base de victoires ponctuelles)
- **NNUE** : réseau de neurones pour l'évaluation, à très long terme
