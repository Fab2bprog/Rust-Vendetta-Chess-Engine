# Vendetta Chess Motor

**Vendetta Chess Motor** is a professional chess engine written entirely in Rust, compatible with the UCI (*Universal Chess Interface*) protocol. It can be used with any UCI-compatible graphical interface: Arena, CuteChess, Scid, Lichess BOT, etc.

> **Version 1.1.2** · GPL-3.0 License · Pure Rust, no external dependencies

---

## About

Vendetta Chess Motor was born from the philosophy of **stability and correctness before performance**. Its name pays tribute to Corsica (*the vendetta*), the Rust language (*robustness*), and chess (*Chess*).

The engine is written in standard Rust, with no external dependencies. The Rust compiler eliminates entire categories of bugs at compile time that crash C++ engines in production: null dereferences, *use-after-free*, and *data races* between threads. Robustness isn't an option — it's guaranteed by the language.

**Author:** Fabrice Garcia

---

## Estimated playing strength

**~2,600 Elo** — strong Grandmaster level, capable of beating nearly all human players.

Estimate empirically confirmed through a series of games against Stockfish
at limited Elo (wins achieved successively at the 2100, 2300, and then
2500 tiers), after applying Texel Tuning v3 (see the "Texel Tuning" section
below). This figure reflects an evaluation based on usage rather than an
official rating from a scored pool of games — to be refined with more
games and, eventually, a proper Elo rating tool (e.g.
Bayeselo, ordo) over a large number of games.

| Component | Estimated Elo gain |
|---|---|
| Alpha-beta + LMR + Null Move | base |
| Zobrist transposition table | +100 – 150 |
| Killer moves + History heuristic | +50 – 80 |
| Countermove heuristic | +20 – 40 |
| Continuation History | +15 – 25 |
| Aspiration Windows | +30 – 50 |
| Razoring | +5 – 15 |
| Reverse Futility Pruning (Static Null Move) | +10 – 20 |
| Late Move Pruning (LMP) | +10 – 20 |
| Mate Distance Pruning | +0 – 5 (free, risk-free) |
| Internal Iterative Reduction (IIR) | +10 – 20 |
| "Improving" flag (RFP/LMP/NMP) | re-enabled and fixed (v1.1.0) — +5 to +15 |
| Check handling in quiescence | re-enabled in a safe form (v1.1.0) — +10 to +20 |
| Tempo Bonus | +5 – 15 |
| Check Extension | +20 – 40 |
| Singular Extension | +40 – 70 |
| SEE (Static Exchange Evaluation) | +60 – 80 |
| Lazy SMP (multi-threading) | +50 – 80 |
| Extended evaluation (mobility, center, endgames…) | +100 – 150 |
| Threat detection / hanging pieces | +10 – 25 |
| Magic Bitboards (O(1) attacks) | +30 – 50 |
| Texel Tuning v3 (material, pawns, mobility, king, center) | confirmed by real games |

---

## Features

### Board representation
- **64-bit Bitboards** — ultra-fast representation of game state
- **Magic Bitboards** — O(1) computation of sliding piece attacks (rook, bishop, queen)
- **Zobrist Hashing** — unique fingerprint of each position for the transposition table
- **Symmetric Make / Unmake** — move application and undo without copying the board
- **Strict FEN validation** — castling rights checked against actual piece placement

### Move generation
- Full legal move generation: normal moves, captures, castling, en passant, promotions
- Zero illegal moves guaranteed — legal filtering accelerated by pin detection (v1.1.0): a fast path with no make/unmake for the common case, falling back to make/is_in_check/unmake for tricky cases (king, castling, en passant, pinned piece, check), with a safety net in debug builds
- **Perft 6/6 PASS** — generation validated against the 6 reference positions from the Chess Programming Wiki (to be re-validated after the v1.1.0 changes via `cargo test -- --include-ignored`)

### Search algorithms
- **Iterative Deepening** with precise time management
- **Alpha-Beta** with [alpha, beta] window
- **Aspiration Windows** — narrow window around the previous score, widened on failure
- **Null Move Pruning** (R=3) with anti-zugzwang protection
- **Late Move Reduction (LMR)** — dynamic logarithmic reduction via a precomputed table
- **Reverse Futility Pruning (Static Null Move)** — beta-side cutoff: if the static evaluation already exceeds beta by a comfortable margin at low depth, the opponent would never let the game reach this position
- **Razoring** — alpha-side cutoff: if the static evaluation is far below alpha at low depth, dive straight into quiescence (formerly named "Futility Pruning" in the code — renamed to match standard terminology, which reserves that term for per-move pruning, not implemented here)
- **Late Move Pruning (LMP)** — a quiet move that comes very late in the ordering, at low depth, is not searched at all (unlike LMR, which only reduces the probe depth). Implemented after the Countermove Heuristic to make move ordering reliable before depending on it this aggressively. Killer moves and countermoves are explicitly exempt from this pruning
- **Mate Distance Pruning** — directly tightens [alpha, beta] to the mate scores reachable from the current node; an exact technique (not a heuristic), free and with zero tactical risk
- **Internal Iterative Reduction (IIR)** — when the transposition table has no move for a node (depth ≥ 4), reduces the depth by 1 before continuing rather than treating this poorly-documented node with the same confidence as a well-informed one. Replaces the old Internal Iterative Deepening (IID) — no extra recursive call, just a conditional subtraction
- **"Improving" flag (RFP/LMP/NMP)** — **re-enabled and fixed in v1.1.0**. The cause of the original bug (`eval_history` written conditionally, leaving stale data from other branches) has been resolved: the static eval stack is now written on every real visit (sentinel value when in check), except during Singular Extension search — so `eval_history[ply-2]` always reflects the ancestor of the current path (standard technique, similar to `ss->staticEval`)
- **Check Extension** — +1 depth when a move gives check (bounded to avoid infinite recursion)
- **Singular Extension** — +1 depth for the TT move when it is the only good move
- **SEE** (*Static Exchange Evaluation*) — static evaluation of capture sequences
- **Quiescence search** — resolves pending captures at the end of the search, with **correct check handling** (v1.1.0): when the side to move is in check, no stand-pat, all evasions are searched and mate is detected (generating quiet counter-check moves remains intentionally unimplemented — too costly)
- **Transposition table** — *lock-free* (AtomicU64), shared across all threads, configurable size (default 32 MB, `Hash` UCI option)
- **Lazy SMP** — multi-core parallelism, up to 768 threads

### Move ordering
1. Transposition table move
2. Winning captures (SEE ≥ 0), sorted by SEE score
3. Queen promotions
4. Killer moves (2 per depth)
5. Countermove (recorded refutation of the last move played by the opponent)
6. Quiet moves ordered by History + Continuation History (additive bonus, not a separate tier)
7. Losing captures (SEE < 0)

### Evaluation function
- **Material** — piece values calibrated in centipawns
- **Bishop pair bonus**
- **Piece-Square Tables (PST)** — interpolated between middlegame and endgame (*tapered eval*)
- **Mobility** — bonus per accessible square (knight, bishop, rook, queen)
- **Center control** (d4/d5/e4/e5)
- **Pawn structure** — doubled, isolated, passed pawns
- **King safety** — pawn shield, open files, king in the center
- **Endgame specifics** — mop-up, rook on the 7th, Tarrasch rule, opposite-colored bishops
- **Threats / hanging pieces** — penalty for a piece attacked by a cheaper enemy piece, and for a piece attacked with no defense at all ("hanging"). Cheap square control check (not a full SEE), active at any point in the game
- **Tempo Bonus** — fixed bonus for the player to move (initiative advantage)

---

## Performance

Measurements taken on an **Apple Mac Mini M2 Pro (10 cores)**, `--release` build.

### Perft validation — move generation correctness

| Position | Description | Depth | Nodes | Result |
|---|---|---|---|---|
| Initial position | Base case | 5 | 4,865,609 | ✓ PASS |
| Kiwipete | Castling, en passant, promotions | 4 | 4,085,603 | ✓ PASS |
| Passed pawn endgame | Multiple promotions | 5 | 674,624 | ✓ PASS |
| Promotions and castling | Limited rights | 4 | 422,333 | ✓ PASS |
| En passant edge cases | Complex en passant captures | 4 | 2,103,487 | ✓ PASS |
| Middlegame | Balanced open position | 4 | 3,894,594 | ✓ PASS |

### Alpha-beta search benchmark — 3 seconds per position

| Threads | Average NPS | Lazy SMP gain |
|---|---|---|
| 1 | ~3,200,000 nps | ×1.00 |
| 2 | ~6,400,000 nps | ×2.01 |
| 4 | ~12,700,000 nps | ×4.02 |
| 8 | ~22,200,000 nps | ×6.89 |
| 10 | ~24,900,000 nps | ×7.75 |

Lazy SMP scales nearly linearly up to 4 threads and reaches ×7.75 on 10 cores (77.5% efficiency).

---

## Building

### Prerequisites
- [Rust](https://rustup.rs/) 1.70 or higher
- Cargo (included with Rust)

### Standard build

```bash
git clone https://github.com/<your-account>/vendetta_chess_motor.git
cd vendetta_chess_motor
cargo build --release
```

The binary is produced at `target/release/vendetta_chess_motor`.

### Native Apple Silicon build

```bash
cargo build --release --target aarch64-apple-darwin
```

---

## Usage

Vendetta Chess Motor is a pure UCI engine — it has no graphical interface of its own. It is used with UCI-compatible software:

1. Build the engine in release mode
2. In your graphical interface (Arena, CuteChess, Scid, etc.), add a new engine pointing to the `vendetta_chess_motor` binary
3. The engine identifies itself as: `id name Vendetta Chess Motor 1.1.2`

### UCI options

| Option | Type | Default | Range | Description |
|---|---|---|---|---|
| `Hash` | spin | 32 MB | 1 MB – 32 GB | Transposition table size (conservative default; increase according to available RAM, especially for analysis) |
| `Threads` | spin | auto | 1 – 768 | Number of search threads (Lazy SMP) |
| `Skill Level` | spin | 64 | 1 – 64 | Playing strength, custom option (64 = full strength) |
| `Ponder` | check | true | — | Think during the opponent's time |
| `Debug` | check | false | — | Internal debug messages |
| `UCI_LimitStrength` | check | false | — | Enables strength limiting via `UCI_Elo` (standard UCI, takes priority over `Skill Level`) |
| `UCI_Elo` | spin | 2600 | 600 – 2600 | Target Elo strength when `UCI_LimitStrength` is active |
| `MultiPV` | spin | 1 | 1 – 218 | Number of best lines shown |
| `Move Overhead` | spin | 50 ms | 0 – 5000 | Safety margin subtracted from thinking time (network/GUI latency) |
| `Clear Hash` | button | — | — | Immediately clears the transposition table |
| `UCI_AnalyseMode` | check | false | — | Always forces the best move (takes priority over any strength limiting) |
| `Contempt` | spin | 0 | -100 – 100 | Slightly penalizes draws (centipawns) — 0 = unchanged behavior. Useful against a weaker opponent |
| `UCI_EngineAbout` | string | — | — | Information about the engine (read-only, no effect) |

**Additional `go` commands**: in addition to the standard parameters (`wtime`, `btime`, `depth`, `movetime`, `infinite`, `searchmoves`...), Vendetta Chess Motor accepts `go nodes <x>` (limit by node count) and `go mate <x>` (search for a forced mate in x moves).

### Difficulty levels

64 levels available via `Skill Level`. The scaling combines two mechanisms:

**Maximum depth** (quadratic interpolation):
- Level 1 → depth 1 (absolute beginner)
- Level 16 → depth 4 (amateur)
- Level 32 → depth 7 (intermediate)
- Level 48 → depth 11 (advanced)
- Level 64 → no limit (full strength)

**Error probability** (quadratic decay):
- Level 1 → 90% chance of playing a random move
- Level 32 → ~10%
- Level 57+ → 0% (always the best move)

---

## Development tools

### Perft — move generation validation

```bash
# Full suite over the 6 reference positions
cargo run --release --bin perft

# Specific position
cargo run --release --bin perft -- "<fen>" <depth>

# Divide mode — move-by-move breakdown (to isolate a bug)
cargo run --release --bin perft -- divide "<fen>" <depth>
```

### Benchmark — performance measurement

```bash
# Full suite (3 seconds per position)
cargo run --release --bin benchmark

# Custom duration (in milliseconds)
cargo run --release --bin benchmark -- --time 5000

# Maximum number of threads
cargo run --release --bin benchmark -- --threads 4
```

### Unit tests

```bash
# Quick tests (< 10 seconds)
cargo test

# Full suite including slow tests (several minutes)
cargo test -- --include-ignored
```

### Texel Tuning — automatic evaluation calibration

Two-stage pipeline to calibrate a subset of the evaluation constants
(material, pawn structure, mobility, king safety, center) on a
base of real games, rather than hand-picked values. See
`ARCHITECTURE.md` for the full details of the algorithm, including the
preliminary K-scale calibration (a required step — see the version
history in `ARCHITECTURE.md`).

```bash
# 1. Extract positions (FEN + result) from a PGN file
#    (itself prepared beforehand by filter_pgn.rs, outside the repo — see
#    ARCHITECTURE.md, a standalone tool for filtering a Lichess dump)
cargo run --release --bin extract_positions -- positions.pgn positions.txt

# 2. Run the tuning (K calibration, then coordinate descent over 22 parameters)
cargo run --release --bin tuner -- positions.txt
```

**Current status: v3 values applied to the production code and validated
by real games** (successive wins against Stockfish at limited Elo
2100, 2300, then 2500 — playing strength now estimated at ~2,600 Elo,
see the "Estimated playing strength" section above). Calibrated on 2,464,785
positions from 302,864 Lichess Rapid/Classical games, Elo ≥ 2100
(May 2026 dump). See `ARCHITECTURE.md` for the full table of values
before/after.

---

## Project structure

```
src/
├── main.rs              # Entry point
├── lib.rs               # Public exports
├── board/
│   ├── state.rs         # Board state, make/unmake, FEN
│   ├── bitboard.rs      # Bitboard operations, attack tables
│   └── magic.rs         # Magic Bitboards
├── moves/
│   ├── mod.rs           # Legal generation, perft
│   ├── pawn.rs          # Pawns
│   ├── knight.rs        # Knights
│   ├── bishop.rs        # Bishops
│   ├── rook.rs          # Rooks
│   ├── queen.rs         # Queens
│   └── king.rs          # King and castling
├── search/
│   ├── mod.rs           # SearchEngine, Lazy SMP, time management
│   ├── alphabeta.rs     # Alpha-beta, LMR, RFP, Razoring, Singular Extension
│   ├── transposition.rs # Lock-free transposition table
│   ├── killers.rs       # Killer moves
│   ├── history.rs       # History heuristic
│   └── see.rs           # Static Exchange Evaluation
├── eval/
│   ├── mod.rs           # Main evaluation with tapering
│   ├── material.rs      # Piece values
│   ├── tables.rs        # Piece-Square Tables
│   ├── position.rs      # Positional evaluation
│   ├── pawns.rs         # Pawn structure
│   ├── king_safety.rs   # King safety
│   ├── mobility.rs      # Mobility
│   ├── center.rs        # Center control
│   ├── endgame.rs       # Endgames
│   └── phase.rs         # Game phase
├── game/
│   ├── mod.rs           # Game logic
│   ├── rules.rs         # Draw, checkmate, stalemate
│   └── history.rs       # Repetition detection
├── uci/
│   ├── mod.rs           # UCI state machine
│   └── parser.rs        # UCI command parser
├── utils/
│   └── types.rs         # Core types
└── bin/
    ├── perft.rs              # Perft validation tool
    ├── benchmark.rs          # Benchmark tool
    ├── extract_positions.rs  # Texel Tuning step 1 — PGN → positions extraction
    └── tuner.rs              # Texel Tuning step 2 — coordinate descent
```

---

## License

Vendetta Chess Motor is distributed under the **GNU General Public License v3.0 (GPL-3.0-or-later)**.

You are free to use, modify, and redistribute it under the terms of this license. Any distribution of derivative software must be done under the same GPL-3.0 license and include the complete source code.

See: https://www.gnu.org/licenses/gpl-3.0.html
