# Vendetta Chess Motor Architecture

> Version 1.1.2 · GPL-3.0 License

## Overview

Vendetta Chess Motor is organized into independent modules that can be tested separately.
The dependency between modules follows a strict hierarchy to avoid cycles.
No external dependencies — standard Rust only.

## Module hierarchy

```
utils/types     → (no dependency)
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

Common types shared by all modules.

- `types.rs` — Color, Piece, Square, Move, MoveFlags, score constants
  (SCORE_INF, SCORE_MATE, SCORE_DRAW)

---

### board

Board representation via bitboards.

- `bitboard.rs` — Bitboard type (u64), bit operations (set/clear/get/pop/lsb),
  file and rank masks, precomputed attack tables (knight, king),
  sliding piece attack functions via Magic Bitboards (bishop_attacks,
  rook_attacks, queen_attacks) — O(1) by delegating to magic.rs
- `magic.rs`   — Magic Bitboards for rook and bishop: occupancy masks, magic
  numbers found at startup via sparse random trial (< 10 ms, 128 magics),
  heap-allocated flat tables (2 MB rooks + 256 KB bishops), public O(1) API
  rook_attacks_magic() / bishop_attacks_magic(), thread-safe via OnceLock
- `state.rs` — Board struct (12 piece bitboards + 2 occupancy bitboards),
  CastlingRights, make_move / unmake_move, make_null_move / unmake_null_move,
  FEN read/write, incremental Zobrist hashing, king_square(), piece_at(),
  `piece_count[2][6]` maintained incrementally (insufficient material detection),
  `eval_mg` / `eval_eg` maintained incrementally (O(1) evaluation),
  `piece_on[64]` (mailbox `Option<(Piece,Color)>`) maintained incrementally in
  place_piece/remove_piece → piece_at() in O(1) instead of scanning 12 bitboards
  (consistency debug_assert against a scan in debug builds),
  FEN validation of castling rights against actual piece placement,
  Zobrist hash computed after rights correction (guaranteed TT consistency)

---

### moves

Complete legal move generation.

- `pawn.rs`   — single/double pushes, captures, en passant, promotions
- `knight.rs` — knight moves (precomputed table)
- `bishop.rs` — bishop moves (sliding piece)
- `rook.rs`   — rook moves (sliding piece)
- `queen.rs`  — queen moves (bishop + rook)
- `king.rs`   — king moves (precomputed table) + castling (legality checked)
- `mod.rs`    — generate_legal_moves(), generate_legal_captures()
                (captures only, avoids ~30 silent make/unmake per quiescence node),
                is_in_check(), is_square_attacked(), perft(), perft_divide()
                • `MoveList`: fixed-capacity move list (`[Move; 256]`)
                  stack-allocated (Deref to `[Move]`), zero heap allocation
                  per node — replaces `Vec<Move>` on the hot path
                • `generate_legal_moves_into()` / `generate_legal_captures_into()`:
                  zero-allocation variants (fill a provided MoveList),
                  used by the search; the `Vec`-returning versions are
                  kept as wrappers for binaries/tests/UCI
                • `filter_legal_into()` + `pinned_pieces()`: accelerated legal
                  filtering — fast path WITHOUT make/unmake for the common case
                  (not in check, piece not pinned, no king/castling/en passant),
                  safe make/unmake path for tricky cases. Safety net in debug
                  builds (re-checking every fast decision against make/unmake)

---

### eval

Static evaluation of a position. Score in centipawns, positive = good for
the side to move.

- `material.rs`   — piece values (P=100, N=320, B=330, R=500, Q=900, K=20000),
                    bishop pair bonus (+30), piece_value()
- `position.rs`   — piece-square tables (PST) for middlegame and endgame,
                    interpolated by phase (tapered eval)
- `pawns.rs`      — doubled pawns (-24), isolated pawns (-19),
                    passed pawns (Texel v3 values based on advancement).
                    THREAD-LOCAL pawn hash table: cache of the pawn structure
                    eval, key = pair of pawn bitboards (exact verification,
                    zero false correlation), white-relative value. The eval
                    depends only on pawns → high hit rate, never needs to
                    be cleared
- `king_safety.rs`— pawn shield (+10/pawn), king-in-center penalty (-30),
                    open files near the king (-15/file)
- `phase.rs`      — GamePhase, compute_phase() based on remaining material,
                    is_endgame(), middlegame_factor(), taper()
- `mobility.rs`   — mobility bonus per accessible square (N=4, B=3, R=2, Q=1)
- `center.rs`     — center control (d4/d5/e4/e5): presence (+15),
                    attacks (+5) — disabled in the endgame
- `endgame.rs`    — 6 endgame criteria:
                    (1) Mop-up: pushing the enemy king into a corner (+500 cp required)
                    (2) Rook on the 7th rank (+25 cp)
                    (3) Rook behind a passed pawn — Tarrasch rule (+20 cp)
                    (4) King proximity to passed pawns (enemy_dist - friendly_dist) × 5
                    (5) Opposite-colored bishops: advantage reduced by 50%
                    (6) Passed pawn advancement bonus (enemy king distance 1/2/3)
- `threats.rs`    — Threats / hanging pieces: (1) piece attacked by a
                    cheaper enemy piece (fixed penalty); (2) piece
                    attacked and undefended ("hanging", via the piece's own
                    attack bitboard as a defense proxy). No full SEE —
                    cheap square control check. Active at every
                    phase (not disabled in the endgame)
- `mod.rs`        — evaluate(): weighted sum of all criteria,
                    is_insufficient_material()

---

### search

Search algorithm with all heuristics.

- `transposition.rs` — 64 MB transposition table, lock-free via AtomicU64 (pairs),
                        Zobrist hashing, TTFlag (Exact/LowerBound/UpperBound),
                        mate score adjustment (store/probe),
                        generation-based replacement policy (new_search() increments
                        the generation counter — stale entries are replaceable
                        even at higher depth),
                        prefetch(hash): prefetches the slot's cache line
                        (prfm aarch64 / _mm_prefetch x86-64 / no-op elsewhere), called
                        after make_move to hide latency before the child probe;
                        fallible allocation (try_new via try_reserve_exact) with graceful
                        fallback on the UCI side — a Hash setting that's too large reduces
                        the size instead of crashing (size adjustable up to 32 GB, default 32 MB)
- `killers.rs`       — Killer moves: 2 quiet moves per depth (max 64 levels)
- `history.rs`       — History heuristic: score [piece][destination_square],
                        update_good() / update_bad() with gravity
- `countermove.rs`   — Countermove heuristic: one refutation move per
                        [enemy_piece][enemy_destination_square], derived via
                        board.piece_at(prev_move.to) — a single slot per key
- `continuation_history.rs` — Continuation History: cumulative
                        generalization of the countermove, score [enemy_piece]
                        [enemy_square][piece][destination_square] (147,456 entries,
                        flat Vec<i32> — no nested array, to avoid
                        any stack overflow risk at this size). Aged like
                        history, not reset like the countermove
- `see.rs`           — Static Exchange Evaluation (SEE):
                        evaluates the full capture sequence on a square,
                        recursive LVA (Least Valuable Attacker) with early-stop option,
                        X-ray handling via dynamic occupancy,
                        promotion handling (pawn → queen)
- `alphabeta.rs`     — Main search algorithm:
                        • move_score(): ordering TT → SEE captures → promotions
                          → killers → history → losing captures
                        • order_moves(): pre-computes scores in O(N) then sorts —
                          avoids redundant see() calls inside sort_unstable_by
                        • lmr_reduction(): dynamic logarithmic reduction
                          `1 + ln(depth) × ln(move_index) / 2`, via a OnceLock
                          precomputed table on first call (avoids repeated float computations)
                        • quiescence(): stand-pat, captures sorted and filtered by SEE;
                          if the side to move IS in check → no stand-pat,
                          search ALL legal evasions + mate
                          detection (safe re-enablement; generating
                          quiet counter-check moves remains
                          unimplemented — too costly). Depth bounded by
                          MAX_QUIESCENCE_PLY (= MAX_PLY + 64), guarded at the top of the function
                        • alpha_beta() with:
                          - Draw detection (50-move rule, repetition, insufficient material)
                          - TT probe with Exact/LowerBound/UpperBound cutoffs
                          - Reverse Futility Pruning / Static Null Move
                            (depth ≤ 6, margin 120×depth, beta-side cutoff)
                          - Razoring (depth ≤ 2, margin 150×depth, alpha-side
                            cutoff — formerly named "Futility Pruning"
                            in this file, renamed to match
                            standard terminology)
                          - Null Move Pruning (R=3, anti-zugzwang)
                          - Singular Extension (depth ≥ 6, verification at depth/2)
                          - Multi-cut (if SE score ≥ beta without the TT move)
                          - Dynamic Late Move Reduction (depth ≥ 3, move_index ≥ 3)
                            ENRICHED (⚠️ adjustments to be validated by SPRT): base
                            logarithmic reduction, +1 if the position is not improving,
                            −1 for a PV node (wide window), −1 for a killer or
                            countermove move; bounded to r ≥ 1
                          - Late Move Pruning (depth ≤ 8, threshold 4+2×depth²
                            moves — the move is not searched at all, not reduced;
                            implemented after the Countermove Heuristic, which
                            it depends on for reliable move ordering;
                            killers and countermoves explicitly exempted)
                          - Per-move Futility Pruning (depth ≤ 6, prunes a
                            quiet non-checking move if static_eval +
                            100×depth ≤ alpha — ⚠️ margins TO BE VALIDATED BY SPRT,
                            can be disabled via FUTILITY_MAX_DEPTH = 0)
                          - Mate Distance Pruning (tightens alpha/beta to the
                            mate scores reachable from this ply — exact,
                            not a heuristic, placed before the
                            quiescence dispatch)
                          - Internal Iterative Reduction (depth -= 1 if no
                            TT move, depth ≥ 4; placed after Singular Extension,
                            before move generation; replaces IID,
                            no extra recursive call)
                          - "Improving" flag (RFP/NMP/LMP) — RE-ENABLED and
                            fixed: eval_history[ply] written on every real visit
                            (unconditionally, sentinel value when in check),
                            except during SE search — the invariant makes
                            eval_history[ply-2] reliable (ancestor of the
                            current path)
                          - Correction History (⚠️ to be validated by SPRT): the
                            static eval is corrected by the SearchInfo
                            .correction_history table (per thread, indexed [color]
                            [pawn structure key], bounded ±64 cp); learned
                            at the end of the node via (score − corrected eval).
                            Can be disabled via CORRHIST_MAX = 0
                          - Check Extension (+1 if the move gives check, depth ≤ 4,
                            bounded by ply + 1 < MAX_PLY to avoid any infinite recursion)
                          - Killer/history/countermove/continuation history
                            updates on beta cutoffs (history and continuation
                            history exclude moves pruned by LMP via
                            lmp_pruned[])
                        • Safety constants:
                          MAX_PLY = 192 (128 + 64) — absolute recursion bound
                          MAX_QUIESCENCE_PLY = 256 — quiescence bound
- `mod.rs`           — SearchEngine, Iterative Deepening, Aspiration Windows
                        (initial delta 50 cp, doubled on fail),
                        Lazy SMP (up to 768 threads, 8 MiB stack per
                        secondary thread via Builder::stack_size — deep recursion +
                        move lists on the stack; graceful fallback if thread
                        creation fails, shared Arc<TT>,
                        Arc<AtomicBool> stop signal shared with the UCI thread,
                        depth variation across secondary threads: t % 3),
                        time management (compute_time_limit, movestogo protected
                        against division by zero),
                        pondering: infinite search until ponderhit or stop,
                        64 difficulty levels:
                          • skill_level_max_depth(): quadratic interpolation
                            of max depth (1→1, 16→4, 32→7, 48→11, 64→∞)
                          • apply_skill_level(): decreasing error probability
                            (level 1 = 90% random, level 57+ = 0% error)

---

### game

Management of the current game.

- `mod.rs`      — Game struct, position history, make/unmake coordination
- `rules.rs`    — draw detection (50-move rule, repetition, insufficient material),
                  checkmate, stalemate
- `history.rs`  — position history for threefold repetition detection

---

### uci

UCI (Universal Chess Interface) communication protocol.

- `parser.rs` — UCI command parsing:
                uci, isready, ucinewgame, position (startpos/fen + moves),
                go (movetime, wtime/btime/winc/binc, movestogo, depth,
                    infinite, ponder, searchmoves, nodes, mate), stop, ponderhit,
                setoption (Hash, Threads, Skill Level, Ponder, Debug,
                    UCI_LimitStrength, UCI_Elo, MultiPV, Move Overhead,
                    Clear Hash, UCI_AnalyseMode, Contempt, UCI_EngineAbout), quit
- `mod.rs`    — UciEngine state machine, separate stdin thread (mpsc channel),
                search thread (spawn_search), pondering management,
                info emission (depth, seldepth, score, nodes, nps, time,
                hashfull, lowerbound/upperbound, pv, currmove/currmovenumber,
                multipv) and bestmove

#### UCI extensions (beyond the strict minimum required)

Added after a compliance audit — Vendetta Chess Motor already supported all
mandatory commands; these additions cover use cases and standard
conventions useful for interoperability with a wider range of GUIs/platforms:

- **`UCI_LimitStrength` + `UCI_Elo`** (`search::elo_to_skill_level()`) —
  linear interpolation 600-2600 Elo → levels 1-64 (the existing Skill
  Level scale). Lets standard GUIs/platforms (e.g. Lichess
  bot hosting) limit Vendetta Chess Motor's strength without knowing the custom
  "Skill Level" option. Priority in the Go command: `UCI_AnalyseMode` >
  `UCI_LimitStrength` > `Skill Level`.
- **`go nodes <x>`** — `SearchInfo.max_nodes`, checked in `check_time()`
  alongside the time limit (same frequency: every 4096 nodes).
- **`go mate <x>`** — translated into a search depth (2×x plies),
  reuses the existing mate score system (`format_score()`).
- **`MultiPV`** (`SearchEngine::search_multipv()`) — reuses the existing
  `searchmoves` mechanism to progressively exclude the best
  lines already found, **without modifying** `search()` or `alpha_beta()`.
  Default behavior (`multipv=1`) strictly unchanged. Known limitation:
  the intermediate "info depth..." lines for each line do not
  carry a "multipv" field (only the final summary per line
  includes it) — no impact on the result shown by the GUI.
- **`info currmove`/`currmovenumber`** — emitted in `alpha_beta()` at the
  root (`ply == 0`) only. Guarded by `SearchInfo.show_currmove`
  (false by default): **bug fixed along the way** — the first
  version printed unconditionally as soon as `ply == 0`, polluting the output of
  `src/bin/benchmark.rs`, which calls `alpha_beta()` directly (outside
  the UCI layer) to measure raw NPS. Only `SearchEngine::search()` (the
  real UCI-driven search) sets this flag.
- **`Move Overhead`** — replaces the old fixed 50 ms margin hard-coded
  in `compute_time_limit()`; same default value, now
  configurable (`SearchConfig.move_overhead`). Important online/in tournaments to
  avoid losing on time due to communication latency.
- **`Clear Hash`** (button) — clears the TT immediately without going through
  `ucinewgame` (which also resets killers/history, unnecessary mid-thought).
- **`UCI_AnalyseMode`** — forces `skill_level = 64`, takes priority over
  any other strength limiting.
- **`Contempt`** (`alphabeta.rs::draw_score()`) — slightly penalizes
  drawn positions (50-move rule, repetition, insufficient material, stalemate) from the
  point of view of the side at the root of the search. 0 by default =
  exact `SCORE_DRAW`, unchanged behavior. Derived via `ply` parity
  (even → root directly, odd → opponent, inverted by negamax) —
  no need to know the engine's color. **Point of caution**:
  `info.contempt` is copied IDENTICALLY across all Lazy SMP threads, otherwise
  the shared TT would store inconsistent draw scores depending on
  which thread computed them.
- **`UCI_EngineAbout`** — cosmetic information string (name, version,
  author, license), declared for UCI compliance but with no
  functional effect.

---

## Square representation

```
Square: u8, value 0 to 63
sq = rank * 8 + file
File: 0=a, 1=b, ..., 7=h
Rank: 0=rank1, 1=rank2, ..., 7=rank8
Example: e4 = rank 3, file 4 → sq = 28
```

## Move representation

```rust
pub struct Move {
    pub from:      u8,         // starting square (0–63)
    pub to:        u8,         // destination square (0–63)
    pub flags:     MoveFlags,  // Quiet | DoublePush | Castle* | Capture |
                               // EnPassant | Promotion | PromotionCapture
    pub promotion: u8,         // 0=none, 1=N, 2=B, 3=R, 4=Q
}
```

## Multi-threading: Lazy SMP

```
Main thread
  ├── Board (clone)
  ├── SearchInfo (shared stop signal)
  ├── KillerMoves (private)
  ├── HistoryTable (private)
  └── Arc<TranspositionTable>  ←──┐
                                   │ (shared)
Secondary thread ×N                │
  ├── Board (independent clone)    │
  ├── SearchInfo (stop signal)     │
  ├── KillerMoves (private)        │
  ├── HistoryTable (private)       │
  └── Arc<TranspositionTable>  ────┘
```

Secondary threads populate the TT at various depths.
The main thread benefits from this via TT hits (better move ordering,
earlier cutoffs).

## Search flow (simplified)

```
go wtime ... btime ...
  └── SearchEngine::search()
        └── Iterative Deepening (depth 1, 2, 3, ...)
              └── Aspiration Windows [prev_score ± 50]
                    └── alpha_beta(depth, alpha, beta, ply=0)
                          ├── Draw detection / TT probe
                          ├── Mate Distance Pruning (tightens alpha/beta, exact)
                          ├── "Improving" flag (eval vs ply-2, RFP/NMP/LMP)
                          ├── Reverse Futility Pruning (depth ≤ 6, beta side)
                          ├── Razoring (depth ≤ 2, alpha side)
                          ├── Null Move (depth ≥ 3)
                          ├── Singular Extension (depth ≥ 6)
                          ├── Internal Iterative Reduction (depth -= 1 if no TT move)
                          └── For each move (ordered by SEE/killers/countermove/history)
                                ├── Check Extension
                                ├── Late Move Pruning (late move → not searched)
                                ├── LMR (late move → reduced depth)
                                └── alpha_beta(depth-1+ext, ...)
                                      └── quiescence() if depth ≤ 0
                                            └── captures filtered SEE ≥ 0
```

---

## Development binaries

Four additional binaries are included in `src/bin/`, intended for
development only — they are not part of the UCI engine shipped to the
end user.

### perft — Move generation validation

```
cargo run --release --bin perft
cargo run --release --bin perft -- "<fen>" <depth>
cargo run --release --bin perft -- divide "<fen>" <depth>
```

Counts leaf nodes at depth N and compares them against the reference values
from the Chess Programming Wiki. A discrepancy of 1 node reveals a specific bug in
generation (illegal castling, missed en passant, ignored pin, etc.).
Divide mode breaks down the result by root move to isolate the divergence.

**v1.0.0 result: 6/6 PASS** across the full set of reference positions.

### benchmark — Performance measurement

```
cargo run --release --bin benchmark
cargo run --release --bin benchmark -- --time 5000 --threads 8
```

Runs a real alpha-beta search for N seconds on 5 typical positions,
varying the number of threads (1, 2, 4, 8, N). Shows the NPS and
the Lazy SMP scaling gain at each tier.

**v1.0.0 result on Apple M2 Pro (10 cores): ×7.75 on 10 threads.**

### extract_positions — Texel Tuning step 1

```
cargo run --release --bin extract_positions -- <input.pgn> <output.txt>
```

Replays every game in a PGN file with the engine itself (SAN resolution
via `generate_legal_moves` — no reimplementation of the rules),
samples one position every 8 half-moves after the first 10
(opening theory ignored), and writes `<FEN>;<result>` to an intermediate
text file. Separates the cost of PGN parsing (done once) from the
cost of tuning (repeated at every coordinate descent pass).

The input PGN file is itself prepared beforehand by `filter_pgn.rs` —
a **standalone tool, outside the vendetta_chess_motor repository** (in the project's
root folder), which filters a monthly Lichess dump (.pgn.zst) by the Elo of both
players and the time control (Rapid/Classical), streaming decompression via the
`zstd` subprocess — zero added Cargo dependency. Direct compilation:
`rustc -O filter_pgn.rs -o filter_pgn`.

### tuner — Texel Tuning step 2 (K calibration + coordinate descent)

```
cargo run --release --bin tuner -- <positions.txt>
```

Automatically calibrates a subset of the evaluation constants, in
two phases:

**Phase 1 — K calibration (`calibrate_k()`)**
The Texel error compares `sigmoid(eval / K)` to the actual game result.
K must be calibrated on the data BEFORE touching the weights — otherwise
the optimizer compensates for a bad scale calibration by shrinking all
the weights instead of calibrating them correctly (see "Version
history" below — this pitfall actually occurred). Ternary
search on K over [50, 1000], with fixed parameters (the engine's starting
values). K is then **fixed** for the whole of phase 2.

**Phase 2 — coordinate descent on the weights**
  1. Loads all positions (FEN + actual result) into memory.
  2. Computes the mean squared error between `sigmoid(eval, K)` and the
     actual result, over the entire dataset — in parallel across
     all available cores (`std::thread::scope`, independent sum
     per chunk of positions, no coordination during computation).
  3. For each parameter, tries ±1; keeps the change if it reduces
     the overall error, otherwise rejects it.
  4. Repeats until a full pass no longer improves any parameter.
  5. Prints the detail of the 22 parameters every `PRINT_EVERY` passes
     (default 100), to track progress without flooding the output.

**v3 scope: 22 parameters** — material (Knight/Bishop/Rook/Queen, Pawn fixed
at 100 as an anchor), doubled/isolated pawn penalties, passed pawn bonus (6
tiers), bishop pair, mobility (4 pieces), king safety (3 criteria),
center control (2 criteria). Deliberately without PST (384 values)
for now — possible extension in v4.

Important: the evaluation used by the tuner (`tunable_eval_white_pov`)
is a simplified reimplementation, **separate** from `eval::evaluate()`. The
real engine maintains material and PST incrementally
(`board.eval_mg`/`eval_eg`, updated on every `place_piece`/`remove_piece`)
for search performance — a choice incompatible with the tuner's need
to recompute the score for thousands of candidate parameter sets. The
tuner therefore recomputes material, pawn structure,
mobility, king safety, and center directly from the bitboards
on every call: slower, but with no impact (offline computation, not a
time-limited search). The production engine is not touched by
this file — only the final VALUES are extracted from it, manually.

#### Tuner version history — why K calibration was necessary

- **v1 (12 parameters, no K calibration)**: converged in ~150 passes to
  a **degenerate** result — all piece values divided by ~2 in an
  almost uniform way, and **negative** passed pawn bonuses at the
  early ranks (impossible in chess: a passed pawn is never a weakness).
- **v2 (22 parameters, no K calibration)**: result almost identical to
  v1, which invalidated the initial hypothesis ("model too weak") —
  adding more criteria changed nothing about the problem.
- **Diagnosis**: a pure scale collapse shrinks the values toward
  zero WITHOUT changing their sign. The fact that the passed pawn bonuses
  went from positive to negative at the early ranks, in an almost
  identical way between v1 and v2, pointed to a structural cause common
  to both models rather than a lack of expressiveness: `SIGMOID_SCALE`
  was fixed at 400 (a "historical" value) without being calibrated on this
  specific data.
- **v3 (K calibration + 22 parameters)**: calibrated K = **748.22** (vs 400).
  With this fixed K, tuning converges to strictly positive and
  increasing passed pawn bonuses, and piece value ratios
  consistent with classical theory.

**v3 values applied in production** (measured on 2,464,785 positions,
302,864 Lichess Rapid/Classical games, Elo ≥ 2100, May 2026 dump):

| Parameter | Before | After v3 |
|---|---|---|
| Knight / Bishop / Rook / Queen | 320 / 330 / 500 / 900 | 216 / 224 / 382 / 817 |
| Bishop pair bonus | 30 | 50 |
| Doubled / isolated pawns | -20 / -20 | -24 / -19 |
| Passed pawns (ranks 2-7) | 5,10,20,35,60,100 | 7,8,33,75,138,218 |
| Knight/Bishop/Rook/Queen mobility | 4/3/2/1 | 11/10/10/5 |
| Pawn shield | 10 | 14 |
| King in center | -30 | **-7** (most notable change) |
| Open file near king | -15 | -21 |
| Center — pawn / attack | 15 / 5 | 9 / 6 |

Perft re-verified after applying: 6/6 PASS (change limited to
the evaluation).

**Validation confirmed by real games**: after applying it, Vendetta Chess Motor
beat Stockfish successively set to 2100, 2300, then 2500 limited Elo.
Playing strength now estimated at ~2,600 Elo (see README.md). The drop in
the "king in center" penalty (-30 → -7) translated concretely into a
noticeably more active white king in the middlegame/endgame in the games
observed — risky behavior in theory, but one that has not caused any
problem in the games played so far (positions where Vendetta Chess Motor already
had the advantage). Worth watching if unexpected losses appear.

**Performance measurements on Apple M2 Pro (10 cores):** K calibration ≈ 3.4s,
then ~0.86s per pass in parallel over 2.46 million positions
(versus ~6s per pass sequentially — a ×7 gain, consistent with the number of
cores); convergence reached after 156 passes for v3.

---

## Future work under consideration

- **Extend Texel Tuning to the PST** (384 values) — now that Texel Tuning v3
  has been validated by real games, this is the logical next step
- **More rigorous Elo measurement** — a rating tool (Bayeselo, ordo)
  over a large number of games, to replace the current empirical
  estimate (~2,600 Elo based on a handful of wins)
- **NNUE**: neural network for evaluation, as a very long-term goal
