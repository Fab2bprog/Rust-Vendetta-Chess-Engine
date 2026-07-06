# Vendetta Chess Motor

**Vendetta Chess Motor** est un moteur d'échecs professionnel écrit entièrement en Rust, compatible avec le protocole UCI (*Universal Chess Interface*). Il peut être utilisé avec n'importe quelle interface graphique UCI : Arena, CuteChess, Scid, Lichess BOT, etc.

> **Version 1.1.2** · Licence GPL-3.0 · Rust pur, aucune dépendance externe

---

## À propos

Vendetta Chess Motor est né de la philosophie **stabilité et correction avant performance**. Son nom rend hommage à la Corse (*la vendetta*), au langage Rust (*la robustesse*), et aux échecs (*Chess*).

Le moteur est écrit en Rust standard, sans aucune dépendance externe. Le compilateur Rust élimine à la compilation des catégories entières de bugs qui font crasher les moteurs C++ en production : déréférencement nul, *use-after-free*, *data races* entre threads. La robustesse n'est pas une option — elle est garantie par le langage.

**Auteur :** Fabrice Garcia

---

## Remerciements

Vendetta Chess Motor a été développé **en coworking avec Claude** (Anthropic) — l'ensemble du code, des audits de robustesse, du Texel Tuning et de la documentation de ce projet est le fruit d'une collaboration directe entre un humain et une IA, du tout premier prototype jusqu'à la version actuelle. C'est dit ici explicitement par souci d'honnêteté envers quiconque consulte ce dépôt.

---

## Niveau de jeu estimé

**~2 600 Elo** — niveau Grand Maître fort, capable de battre la quasi-totalité des joueurs humains.

Estimation confirmée empiriquement par une série de parties contre Stockfish
à Elo limité (victoires obtenues successivement aux paliers 2100, 2300 puis
2500), après l'application du Texel Tuning v3 (voir section "Texel Tuning"
plus bas). Ce chiffre reflète une évaluation par l'usage plutôt qu'un
classement officiel sur un pool de parties noté — à affiner avec davantage
de parties et, à terme, un véritable outil de classification Elo (ex :
Bayeselo, ordo) sur un grand nombre de parties.

| Composant | Gain Elo estimé |
|---|---|
| Alpha-bêta + LMR + Null Move | base |
| Table de transposition Zobrist | +100 – 150 |
| Killer moves + History heuristic | +50 – 80 |
| Countermove heuristic | +20 – 40 |
| Continuation History | +15 – 25 |
| Aspiration Windows | +30 – 50 |
| Razoring | +5 – 15 |
| Reverse Futility Pruning (Static Null Move) | +10 – 20 |
| Late Move Pruning (LMP) | +10 – 20 |
| Mate Distance Pruning | +0 – 5 (gratuit, sans risque) |
| Internal Iterative Reduction (IIR) | +10 – 20 |
| Drapeau "improving" (RFP/LMP/NMP) | réactivé et corrigé (v1.1.0) — +5 à +15 |
| Gestion de l'échec en quiescence | réactivée sous forme sûre (v1.1.0) — +10 à +20 |
| Tempo Bonus | +5 – 15 |
| Check Extension | +20 – 40 |
| Singular Extension | +40 – 70 |
| SEE (Static Exchange Evaluation) | +60 – 80 |
| Lazy SMP (multi-threading) | +50 – 80 |
| Évaluation étendue (mobilité, centre, finales…) | +100 – 150 |
| Détection de menaces / pièces en prise | +10 – 25 |
| Magic Bitboards (attaques O(1)) | +30 – 50 |
| Texel Tuning v3 (matériel, pions, mobilité, roi, centre) | confirmé par parties réelles |

---

## Fonctionnalités

### Représentation du plateau
- **Bitboards 64 bits** — représentation ultra-rapide de l'état du jeu
- **Magic Bitboards** — calcul en O(1) des attaques des pièces glissantes (tour, fou, dame)
- **Zobrist Hashing** — empreinte unique de chaque position pour la table de transposition
- **Make / Unmake symétrique** — modification et annulation de coup sans copie du plateau
- **Validation FEN stricte** — droits de roque vérifiés contre la présence réelle des pièces

### Génération des coups
- Génération légale complète : coups normaux, captures, roques, prises en passant, promotions
- Zéro coup illégal garanti — filtrage légal accéléré par détection des clouages (v1.1.0) : chemin rapide sans make/unmake pour le cas courant, repli make/is_in_check/unmake pour les cas délicats (roi, roque, prise en passant, pièce clouée, échec), avec filet de sécurité en build debug
- **Perft 6/6 PASS** — génération validée sur les 6 positions de référence de la Chess Programming Wiki (à re-valider après les changements v1.1.0 via `cargo test -- --include-ignored`)

### Algorithmes de recherche
- **Iterative Deepening** avec gestion précise du temps
- **Alpha-Bêta** avec fenêtre [alpha, beta]
- **Aspiration Windows** — fenêtre étroite autour du score précédent, élargie en cas d'échec
- **Null Move Pruning** (R=3) avec protection anti-zugzwang
- **Late Move Reduction (LMR)** — réduction dynamique logarithmique via table précalculée
- **Reverse Futility Pruning (Static Null Move)** — coupe côté bêta : si l'évaluation statique dépasse déjà bêta d'une marge confortable à faible profondeur, l'adversaire ne laisserait jamais la partie atteindre cette position
- **Razoring** — coupe côté alpha : si l'évaluation statique est loin sous alpha à faible profondeur, plonge directement en quiescence (anciennement nommé "Futility Pruning" dans le code — renommé pour correspondre à la terminologie standard, qui réserve ce terme à un élagage par coup, non implémenté ici)
- **Late Move Pruning (LMP)** — un coup silencieux qui arrive très tard dans l'ordre, à faible profondeur, n'est pas recherché du tout (contrairement au LMR qui réduit seulement la profondeur de la sonde). Implémenté après le Countermove Heuristic pour fiabiliser l'ordre des coups avant d'en dépendre aussi agressivement. Les killer moves et countermoves sont explicitement exemptés de cet élagage
- **Mate Distance Pruning** — resserre directement [alpha, beta] aux scores de mat atteignables depuis le nœud courant ; technique exacte (pas une heuristique), gratuite et sans aucun risque tactique
- **Internal Iterative Reduction (IIR)** — quand la table de transposition n'a aucun coup pour un nœud (depth ≥ 4), réduit la profondeur d'1 avant de continuer plutôt que de traiter ce nœud peu documenté avec la même confiance qu'un nœud bien renseigné. Remplace l'ancienne Internal Iterative Deepening (IID) — aucun appel récursif supplémentaire, juste une soustraction conditionnelle
- **Drapeau "improving" (RFP/LMP/NMP)** — **réactivé et corrigé en v1.1.0**. La cause du bug d'origine (`eval_history` écrit conditionnellement, laissant des données d'autres branches) est résolue : la pile d'évals statiques est désormais écrite à chaque visite réelle (sentinelle en échec), sauf en recherche Singular Extension — `eval_history[ply-2]` reflète donc toujours l'ancêtre du chemin courant (technique standard, type `ss->staticEval`). Voir CLAUDE.md
- **Check Extension** — +1 profondeur quand un coup donne échec (borné pour éviter toute récursion infinie)
- **Singular Extension** — +1 profondeur pour le coup TT quand il est le seul bon coup
- **SEE** (*Static Exchange Evaluation*) — évaluation statique des séquences de captures
- **Recherche de quiescence** — résolution des captures pendantes en fin de recherche, avec **gestion correcte de l'échec** (v1.1.0) : quand le camp au trait subit un échec, pas de stand-pat, recherche de toutes les évasions et détection du mat (la génération de contre-échecs silencieux reste, elle, volontairement non faite — coût trop élevé)
- **Table de transposition** — *lock-free* (AtomicU64), partagée entre tous les threads, taille configurable (défaut 32 Mo, option UCI `Hash`)
- **Lazy SMP** — parallélisme multi-cœurs, jusqu'à 768 threads

### Ordonnancement des coups
1. Coup de la table de transposition
2. Captures gagnantes (SEE ≥ 0), triées par score SEE
3. Promotions dame
4. Killer moves (2 par profondeur)
5. Countermove (réfutation enregistrée du dernier coup adverse joué)
6. Coups silencieux ordonnés par History + Continuation History (bonus additif, pas un palier séparé)
7. Captures perdantes (SEE < 0)

### Fonction d'évaluation
- **Matériel** — valeurs de pièces calibrées en centipions
- **Bonus paire de fous**
- **Piece-Square Tables (PST)** — interpolées entre milieu de partie et finale (*tapered eval*)
- **Mobilité** — bonus par case accessible (cavalier, fou, tour, dame)
- **Contrôle du centre** (d4/d5/e4/e5)
- **Structure de pions** — pions doublés, isolés, passés
- **Sécurité du roi** — bouclier de pions, colonnes ouvertes, roi au centre
- **Spécificités de finale** — mop-up, tour sur la 7ème, règle de Tarrasch, fous de couleurs opposées
- **Menaces / pièces en prise** — pénalité pour une pièce attaquée par une pièce adverse moins chère, et pour une pièce attaquée sans aucune défense ("en prise"). Contrôle de case bon marché (pas de SEE complet), actif à tout moment de la partie
- **Tempo Bonus** — bonus fixe pour le joueur qui a le trait (avantage de l'initiative)

---

## Performances

Mesures effectuées sur **Apple Mac Mini M2 Pro (10 cœurs)**, compilation `--release`.

### Validation Perft — correction de la génération de coups

| Position | Description | Profondeur | Nœuds | Résultat |
|---|---|---|---|---|
| Position initiale | Cas de base | 5 | 4 865 609 | ✓ PASS |
| Kiwipete | Roques, en passant, promotions | 4 | 4 085 603 | ✓ PASS |
| Finale pions passés | Promotions multiples | 5 | 674 624 | ✓ PASS |
| Promotions et roques | Droits limités | 4 | 422 333 | ✓ PASS |
| En passant edge-cases | Prises en passant complexes | 4 | 2 103 487 | ✓ PASS |
| Milieu de partie | Position ouverte équilibrée | 4 | 3 894 594 | ✓ PASS |

### Benchmark de recherche alpha-bêta — 3 secondes par position

| Threads | NPS moyen | Gain Lazy SMP |
|---|---|---|
| 1 | ~3 200 000 nps | ×1.00 |
| 2 | ~6 400 000 nps | ×2.01 |
| 4 | ~12 700 000 nps | ×4.02 |
| 8 | ~22 200 000 nps | ×6.89 |
| 10 | ~24 900 000 nps | ×7.75 |

Le Lazy SMP scale quasi-linéairement jusqu'à 4 threads et atteint ×7.75 sur 10 cœurs (efficacité 77.5%).

---

## Compilation

### Prérequis
- [Rust](https://rustup.rs/) 1.70 ou supérieur
- Cargo (inclus avec Rust)

### Compilation standard

```bash
git clone https://github.com/<votre-compte>/vendetta_chess_motor.git
cd vendetta_chess_motor
cargo build --release
```

Le binaire est produit dans `target/release/vendetta_chess_motor`.

### Compilation native Apple Silicon

```bash
cargo build --release --target aarch64-apple-darwin
```

---

## Utilisation

Vendetta Chess Motor est un moteur UCI pur — il ne dispose pas d'interface graphique propre. Il s'utilise avec un logiciel compatible UCI :

1. Compiler le moteur en mode release
2. Dans votre interface graphique (Arena, CuteChess, Scid, etc.), ajouter un nouveau moteur en pointant vers le binaire `vendetta_chess_motor`
3. Le moteur s'identifie : `id name Vendetta Chess Motor 1.1.2`

### Options UCI

| Option | Type | Défaut | Plage | Description |
|---|---|---|---|---|
| `Hash` | spin | 32 Mo | 1 Mo – 32 Go | Taille de la table de transposition (défaut prudent ; à augmenter selon la RAM, surtout en analyse) |
| `Threads` | spin | auto | 1 – 768 | Nombre de threads de recherche (Lazy SMP) |
| `Skill Level` | spin | 64 | 1 – 64 | Niveau de jeu, option maison (64 = pleine puissance) |
| `Ponder` | check | true | — | Réflexion pendant le temps de l'adversaire |
| `Debug` | check | false | — | Messages de débogage internes |
| `UCI_LimitStrength` | check | false | — | Active le bridage de force via `UCI_Elo` (standard UCI, prioritaire sur `Skill Level`) |
| `UCI_Elo` | spin | 2600 | 600 – 2600 | Force cible en Elo quand `UCI_LimitStrength` est actif |
| `MultiPV` | spin | 1 | 1 – 218 | Nombre de meilleures variantes affichées |
| `Move Overhead` | spin | 50 ms | 0 – 5000 | Marge de sécurité retirée du temps de réflexion (latence réseau/GUI) |
| `Clear Hash` | button | — | — | Vide la table de transposition immédiatement |
| `UCI_AnalyseMode` | check | false | — | Force toujours le meilleur coup (prioritaire sur tout bridage de force) |
| `Contempt` | spin | 0 | -100 – 100 | Pénalise légèrement la nullité (centipions) — 0 = comportement inchangé. Utile contre un adversaire plus faible |
| `UCI_EngineAbout` | string | — | — | Information sur le moteur (lecture, sans effet) |

**Commandes `go` supplémentaires** : en plus des paramètres standards (`wtime`, `btime`, `depth`, `movetime`, `infinite`, `searchmoves`...), Vendetta Chess Motor accepte `go nodes <x>` (limite par nombre de nœuds) et `go mate <x>` (recherche d'un mat forcé en x coups).

### Niveaux de difficulté

64 niveaux disponibles via `Skill Level`. La graduation combine deux mécanismes :

**Profondeur maximale** (interpolation quadratique) :
- Niveau 1 → profondeur 1 (débutant absolu)
- Niveau 16 → profondeur 4 (amateur)
- Niveau 32 → profondeur 7 (intermédiaire)
- Niveau 48 → profondeur 11 (avancé)
- Niveau 64 → sans limite (pleine puissance)

**Probabilité d'erreur** (décroissance quadratique) :
- Niveau 1 → 90% de chance de jouer un coup aléatoire
- Niveau 32 → ~10%
- Niveau 57+ → 0% (toujours le meilleur coup)

---

## Outils de développement

### Perft — validation de la génération de coups

```bash
# Suite complète sur les 6 positions de référence
cargo run --release --bin perft

# Position spécifique
cargo run --release --bin perft -- "<fen>" <profondeur>

# Mode divide — décomposition coup par coup (pour isoler un bug)
cargo run --release --bin perft -- divide "<fen>" <profondeur>
```

### Benchmark — mesure des performances

```bash
# Suite complète (3 secondes par position)
cargo run --release --bin benchmark

# Durée personnalisée (en millisecondes)
cargo run --release --bin benchmark -- --time 5000

# Nombre de threads maximum
cargo run --release --bin benchmark -- --threads 4
```

### Tests unitaires

```bash
# Tests rapides (< 10 secondes)
cargo test

# Suite complète incluant les tests lents (plusieurs minutes)
cargo test -- --include-ignored
```

### Texel Tuning — calibration automatique de l'évaluation

Pipeline en deux étapes pour calibrer une partie des constantes d'évaluation
(matériel, structure de pions, mobilité, sécurité du roi, centre) sur une
base de parties réelles, plutôt que des valeurs choisies à la main. Voir
`ARCHITECTURE.md` pour le détail complet de l'algorithme, y compris le
calibrage préalable de l'échelle K (étape indispensable — voir l'historique
des versions dans `ARCHITECTURE.md`).

```bash
# 1. Extraire les positions (FEN + résultat) d'un fichier PGN
#    (lui-même préparé en amont par filter_pgn.rs, hors du dépôt — voir
#    ARCHITECTURE.md, outil autonome de filtrage d'un dump Lichess)
cargo run --release --bin extract_positions -- positions.pgn positions.txt

# 2. Lancer le tuning (calibrage K puis coordinate descent sur 22 paramètres)
cargo run --release --bin tuner -- positions.txt
```

**Statut actuel : valeurs v3 appliquées au code de production et validées
par des parties réelles** (victoires successives contre Stockfish à Elo
limité 2100, 2300, puis 2500 — niveau de jeu désormais estimé à ~2 600 Elo,
voir section "Niveau de jeu estimé" plus haut). Calibré sur 2 464 785
positions issues de 302 864 parties Lichess Rapid/Classical, Elo ≥ 2100
(dump mai 2026). Voir `ARCHITECTURE.md` pour le tableau complet des valeurs
avant/après.

---

## Structure du projet

```
src/
├── main.rs              # Point d'entrée
├── lib.rs               # Exports publics
├── board/
│   ├── state.rs         # État du plateau, make/unmake, FEN
│   ├── bitboard.rs      # Opérations bitboard, tables d'attaque
│   └── magic.rs         # Magic Bitboards
├── moves/
│   ├── mod.rs           # Génération légale, perft
│   ├── pawn.rs          # Pions
│   ├── knight.rs        # Cavaliers
│   ├── bishop.rs        # Fous
│   ├── rook.rs          # Tours
│   ├── queen.rs         # Dames
│   └── king.rs          # Roi et roques
├── search/
│   ├── mod.rs           # SearchEngine, Lazy SMP, gestion du temps
│   ├── alphabeta.rs     # Alpha-bêta, LMR, RFP, Razoring, Singular Extension
│   ├── transposition.rs # Table de transposition lock-free
│   ├── killers.rs       # Killer moves
│   ├── history.rs       # History heuristic
│   └── see.rs           # Static Exchange Evaluation
├── eval/
│   ├── mod.rs           # Évaluation principale avec tapering
│   ├── material.rs      # Valeurs des pièces
│   ├── tables.rs        # Piece-Square Tables
│   ├── position.rs      # Évaluation positionnelle
│   ├── pawns.rs         # Structure de pions
│   ├── king_safety.rs   # Sécurité du roi
│   ├── mobility.rs      # Mobilité
│   ├── center.rs        # Contrôle du centre
│   ├── endgame.rs       # Finales
│   └── phase.rs         # Phase de jeu
├── game/
│   ├── mod.rs           # Logique de partie
│   ├── rules.rs         # Nulle, mat, pat
│   └── history.rs       # Détection de répétition
├── uci/
│   ├── mod.rs           # Machine d'état UCI
│   └── parser.rs        # Parseur des commandes UCI
├── utils/
│   └── types.rs         # Types fondamentaux
└── bin/
    ├── perft.rs              # Outil de validation perft
    ├── benchmark.rs          # Outil de benchmark
    ├── extract_positions.rs  # Texel Tuning étape 1 — extraction PGN → positions
    └── tuner.rs              # Texel Tuning étape 2 — coordinate descent
```

---

## Licence

Vendetta Chess Motor est distribué sous licence **GNU General Public License v3.0 (GPL-3.0-or-later)**.

Vous êtes libre de l'utiliser, le modifier et le redistribuer selon les termes de cette licence. Toute distribution d'un logiciel dérivé doit être faite sous la même licence GPL-3.0 et inclure le code source complet.

Voir : https://www.gnu.org/licenses/gpl-3.0.html
