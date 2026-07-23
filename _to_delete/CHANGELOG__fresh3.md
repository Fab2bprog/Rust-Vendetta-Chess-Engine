# Changelog — Vendetta Chess Motor

All notable changes to the project are documented here.
Format inspired by [Keep a Changelog](https://keepachangelog.com/),
semantic versioning [SemVer](https://semver.org/).

---

## [1.1.2] — 2026-06-26

Renamed the project to **Vendetta Chess Motor** (former name: "VendettaChess";
crate `vendetta_chess` → `vendetta_chess_motor`, main binary likewise). Set up
an Elo measurement framework and hardened the test tooling.

### Added
- **Self-play SPRT testing tool** (`selfplay` binary) — objectively measures
  the Elo gain of a change by having the engine play against itself (two
  variants A/B), parallelized, **zero dependencies** (custom parsing/PRNG/SPRT).
  See `COMMENT_TESTER_SPRT.md`.
- **Runtime search toggles** grouped in `FeatureToggles`
  (`SearchInfo.toggles`) — allow isolating a heuristic for an A/B test
  without recompiling or affecting normal play (zero NPS cost): `improving`,
  `futility`, enriched LMR, Correction History, king attack.
- **King safety via attack** (king attack) in the evaluation — non-linear
  danger weighted by piece type on the enemy king's zone, merged into
  the mobility pass (minimal NPS cost). Deliberately conservative tuning
  (SPRT-tested, ~+3 Elo).
- **UCI `register` handler** (no-op: engine has no copy protection) →
  coverage of the **entire** UCI spec command set.

### Changed
- **Reworked Correction History** — depth-weighted learning (in
  fixed point) and **several combined tables** (pawn structure, non-pawn
  pieces by color, continuation), instead of a single fixed-step table.

### Fixed / Robustness
- `selfplay`: `concurrency` bounded (`clamp(1, 64)`) — avoids any risk of OOM
  from an absurd config value.
- `selfplay`: anti-spam display — progress is only reprinted when the
  game counter actually advances.
- `selfplay`: fixed an outdated header comment (now points to
  `COMMENT_TESTER_SPRT.md`).
- Robustness/crash/cleanliness audit from 2026-06-26 — no critical bug found;
  see `AUDIT_STABILITE_2026-06-26.md`.

---

## [1.1.0] — 2026-06-24

An enrichment and hardening release: two search features correctly
re-enabled, a crash bug fixed, seven speed (NPS) optimizations with strictly
identical behavior, and a code cleanliness pass.
Backward-compatible with 1.0.0 (no UCI interface change).

> ⚠️ **To be re-validated after this version** (move generation and search
> code has changed): `cargo test -- --include-ignored`, perft 6/6, and an
> A/B match before/after to quantify the real Elo gain.

### Fixed
- **Crash on non-ASCII UCI move token** — `parse_move_uci` was splitting
  tokens by bytes; a token containing a multi-byte character (e.g. an emoji)
  could panic. Now cleanly rejected (`!mv_str.is_ascii()`), with a
  regression test.
- **"Improving" flag (RFP/LMP/NMP)** — re-enabled after fixing the root
  cause: `eval_history[ply]` is now written on every real visit to the
  node (sentinel value when in check), except during Singular Extension
  search. The invariant guarantees that `eval_history[ply-2]` always
  reflects the ancestor of the current path (standard `ss->staticEval`
  technique).

### Added
- **Check handling in quiescence** — when the side to move is in check,
  no more (illegal) stand-pat: all evasions are generated and mate is
  detected. Cost is kept under control (only triggered on nodes actually
  in check). Generating quiet counter-check moves remains deliberately
  unimplemented.
- **Transposition table prefetching** (`prefetch`) — the TT slot's cache
  line is prefetched right after `make_move` (`prfm` on aarch64,
  `_mm_prefetch` on x86-64).
- **Thread-local pawn structure cache** (pawn hash) — pawn structure
  evaluation (doubled/isolated/passed) is memoized by pair of pawn
  bitboards.
- **Per-move Futility Pruning** (`alpha_beta`) — prunes a quiet
  non-checking move, at low depth (≤ 6), if `static_eval + 100×depth ≤
  alpha`. Complements Razoring (node-level) and LMP (move count).
  ⚠️ **Heuristic to be validated via SPRT match** before being considered
  settled (conservative default margins; can be disabled via
  `FUTILITY_MAX_DEPTH = 0`).
- **Enriched LMR** (`alpha_beta`) — the Late Move Reduction, until now a
  function of depth × move rank alone, is now adjusted by ±1 based on
  additional signals: +1 if the position is not improving, −1 at PV
  nodes (wide window), −1 for a killer/countermove (bounded to r ≥ 1).
  ⚠️ **Adjustments to be validated via SPRT match** (small and conservative
  by default).
- **Correction History** (`SearchInfo` + `alpha_beta`) — a node's static
  evaluation is corrected based on the historical eval↔search gap observed
  for positions with the SAME pawn structure (per-thread table, indexed
  [color][pawn key], correction bounded to ±64 cp). All downstream pruning
  (RFP, NMP, futility, improving) benefits from a better-calibrated eval.
  ⚠️ **Heuristic to be validated via SPRT match**; can be disabled via
  `CORRHIST_MAX = 0`. Simplified version (beta-cutoff nodes are not yet
  learned from).
- **`.gitignore`** and **`CHANGELOG.md`**.

### Performance (NPS — with no change in search results whatsoever)
- **LTO + `codegen-units = 1`** in the release profile.
- **`target-cpu=native`** via `.cargo/config.toml` (non-portable binary — to
  be removed for a distributable release).
- **`piece_on[64]` mailbox** — `piece_at()` goes from scanning 12 bitboards
  to an O(1) read, maintained incrementally.
- **Move lists on the stack (`MoveList`)** — removes the per-node heap
  allocation and allocator contention between Lazy SMP threads.
- **Legal generation via pin detection** — fast path with no make/unmake
  for the common case, safe fallback for tricky cases (king, castling,
  en passant, pinned piece, check), verification safety net in debug builds.

### Changed
- **Default hash size: 16 MB → 32 MB**, and **max cap raised from
  512 MB to 32 GB** (`Hash` UCI option). The cautious default protects
  modest setups; the high cap lets larger machines allocate lots of
  TT for analysis.
- **Graceful fallback on transposition table allocation** (robustness) —
  allocation is now fallible (`try_reserve_exact` instead of aborting the
  process): if a `Hash` setting exceeds available memory, the size is
  halved until it succeeds (and as a last resort the current table is
  kept), with an `info string` message. An overly ambitious setting can
  no longer crash the engine.
- **Search thread stack size raised to 8 MiB** (Lazy SMP threads **and**
  the main search thread) — anti-overflow margin given move arrays now
  allocated on the stack. No performance impact.
- **Code cleanliness** — removed dead code (`piece_bb`, `piece_bb_mut`,
  vestigial `qs_ply` parameter), refreshed outdated comments, and a
  `cargo clippy` pass (from 41 down to 6 lib warnings, the remaining 6
  being deliberate, documented choices).

---

## [1.0.0] — 2026-06

First stable, complete release.

### Added
- Complete chess engine in pure Rust, **zero external dependencies**, 100%
  **UCI** compatible.
- Representation via **bitboards** + **Magic Bitboards** (O(1) attacks),
  incremental **Zobrist** hashing, symmetric make/unmake, strict FEN
  validation.
- **Legal move generation** validated by **Perft 6/6** against the
  reference positions from the Chess Programming Wiki.
- **Alpha-beta** search: Iterative Deepening, Aspiration Windows, PVS, LMR,
  Null Move Pruning, Razoring, Reverse Futility Pruning, Late Move Pruning,
  Mate Distance Pruning, Internal Iterative Reduction, Check & Singular
  Extensions, SEE, quiescence with Delta Pruning.
- Move ordering heuristics: TT, killers, history, countermove,
  continuation history.
- **Lazy SMP** (multi-threading, up to 768 threads), lock-free
  transposition table.
- Evaluation: material, tapered PST, mobility, center, pawn structure,
  king safety, endgames, threats, tempo.
- **Texel Tuning v3** applied (K calibration + 22 parameters) — playing
  strength estimated at **~2,600 Elo** (validated by wins against
  Stockfish at limited Elo).
- 64 difficulty levels, extended UCI options.
