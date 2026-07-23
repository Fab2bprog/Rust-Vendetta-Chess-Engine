// =============================================================================
// Vendetta Chess Engine — src/bin/tuner.rs
//
// Role: Texel Tuning — automatically calibrates a subset of the
//        evaluation constants on the positions file produced by
//        extract_positions.rs (FEN;result, one per line).
//
// Scope of this first version (v1):
//   Tuned parameters: piece values (Knight, Bishop, Rook, Queen —
//   the Pawn stays fixed at 100 as the scale anchor, standard convention of
//   Texel Tuning) + doubled/isolated pawn penalties + passed pawn bonuses
//   (6 advancement tiers). That's 12 parameters.
//
//   Deliberately NOT included in this v1: the piece-square tables (PST,
//   384 values) and the other criteria (mobility, center, king, endgames).
//   Reason: first validate that the whole pipeline works correctly
//   on a restricted set of parameters before extending the scope — each
//   additional parameter multiplies the computation time of a pass.
//
// Why a separate evaluation from eval::evaluate():
//   The real engine maintains material and PST INCREMENTALLY
//   (board.eval_mg / board.eval_eg, updated on every place_piece /
//   remove_piece) for search performance. This optimization is
//   incompatible with tuning, which must be able to recompute the score of a
//   position for THOUSANDS of different candidate parameter sets.
//   tunable_eval() below therefore recomputes material and pawn structure
//   DIRECTLY from the bitboards on every call — slower than
//   the real engine, but with no impact: tuning is an offline computation,
//   not a time-limited search. The production engine is not
//   affected by this file.
//
// Algorithm (Texel's Tuning Method — local search by coordinate):
//   1. Load all positions (FEN + result) into memory.
//   2. Compute the total error (MSE between sigmoid(eval) and the actual result)
//      with the starting parameters (current engine values).
//   3. For each parameter, in turn: try +1, recompute the error
//      over the whole dataset; if better, keep it and continue in
//      that direction; otherwise try -1; otherwise leave this parameter unchanged.
//   4. Repeat over all parameters until a full pass no longer improves
//      anything (convergence).
//   5. Print the new values — this must be manually carried over into
//      the production code (material.rs, pawns.rs) after verification.
//
// Usage:
//   cargo run --release --bin tuner -- positions.txt
// =============================================================================

use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::time::Instant;

use vendetta_chess_engine::board::bitboard::{
    init_attack_tables,
    knight_attacks, bishop_attacks, rook_attacks, queen_attacks, king_attacks,
    white_pawn_attacks, black_pawn_attacks, file_mask,
};
use vendetta_chess_engine::board::state::Board;
use vendetta_chess_engine::utils::types::{Color, Piece};
use vendetta_chess_engine::eval::tables::{
    mirror_square, PAWN_TABLE, KNIGHT_TABLE, BISHOP_TABLE, ROOK_TABLE, QUEEN_TABLE,
};

// =============================================================================
// Tunable parameters
// =============================================================================
//
// v2 — Extension of the v1 model (material + pawn structure, 12 parameters).
//
// Why this extension was necessary:
//   The v1 tuning converged to a degenerate solution — all the
//   piece values divided by ~2, and NEGATIVE passed pawn bonuses on the
//   early ranks (impossible in chess: a passed pawn is never
//   a weakness). Cause: the v1 model was too poor to explain the
//   variance of actual results (blunders, time pressure, tactics that
//   the model doesn't see) — the optimizer "cheated" by flattening all
//   weights toward zero, which reduces the squared error on a noisy signal
//   without having any relation to the actual quality of the positions.
//
//   v2 adds 10 parameters (mobility ×4, king safety ×3, center ×2,
//   bishop pair ×1) to give the model enough expressiveness — it can
//   now explain part of the variance by something other than raw
//   material, which should remove the incentive to flatten the scale.
//
//   Deliberate simplification kept: unlike evaluate(), these
//   criteria are NOT disabled in the endgame (no phase detection
//   implemented in the tuner for now) — they apply to all
//   sampled positions, middlegame and endgame alike.
//
// v4 — Addition of the piece-square tables (PST), Pawn/Knight/Bishop/Rook/Queen
//   only (5 × 64 = 320 new parameters, total 342).
//
//   Deliberate choice to EXCLUDE the King from this extension (decided explicitly
//   with the user before writing this code): in production, the King has
//   TWO distinct tables (KING_MIDDLEGAME_TABLE: stay protected,
//   KING_ENDGAME_TABLE: centralize) selected by the phase of the
//   game. This tuner still has no phase detection (see the v2 note
//   above) — it computes only ONE score per position, not a weighted
//   MG/EG blend. Tuning a single King table would amount to either applying it
//   to both production tables (losing the shelter/centralization distinction
//   that is precisely the point of this split), or to having to add
//   phase detection to the tuner — a separate undertaking, riskier for
//   convergence, reserved for a possible v5.
//
//   For the 5 pieces tuned here, this choice poses NO fidelity problem:
//   in production, these 5 pieces already use A SINGLE table for MG and EG
//   (see eval/tables.rs::piece_square_values — only the King has two tables).
//   The tuner's simplified model is therefore an EXACT representation of the
//   production PST structure for these 5 pieces, not an approximation.
//
//   Starting values imported DIRECTLY from eval::tables (no
//   manual copy-paste of the constants — eliminates any risk of desyn-
//   chronization between the tuner and production).
//
//   Cost: 342 parameters versus 22 in v3, i.e. ~15.5× more trials per
//   coordinate descent pass. The total convergence time will be
//   noticeably longer than in v3 (probably tens of minutes to
//   a few hours depending on the number of passes required) — to be observed
//   empirically rather than promising a precise figure here.

/// Number of scalar parameters (material, pawn structure, mobility,
/// king safety, center — inherited from v1/v2/v3). The PST tables (v4)
/// start at this index.
const NUM_SCALAR_PARAMS: usize = 22;

/// Base index of each of the 5 tuned PST tables, 64 values each.
/// Convention IDENTICAL to eval/tables.rs: square a1 = index 0 from White's
/// point of view, mirror_square() for Black.
const IDX_PST_PAWN_BASE:   usize = NUM_SCALAR_PARAMS;             // 22
const IDX_PST_KNIGHT_BASE: usize = IDX_PST_PAWN_BASE   + 64;      // 86
const IDX_PST_BISHOP_BASE: usize = IDX_PST_KNIGHT_BASE + 64;      // 150
const IDX_PST_ROOK_BASE:   usize = IDX_PST_BISHOP_BASE + 64;      // 214
const IDX_PST_QUEEN_BASE:  usize = IDX_PST_ROOK_BASE   + 64;      // 278

/// Total number of tunable parameters (22 scalars + 320 PST = 342).
const NUM_PARAMS: usize = IDX_PST_QUEEN_BASE + 64;

/// Names of the SCALAR parameters only (indices 0..NUM_SCALAR_PARAMS),
/// in the same order as the indices below — for periodic and
/// final display. The 320 PST parameters (v4) have their own dedicated
/// display function (print_pst_table()) — a name per square would be unreadable here.
const PARAM_NAMES: [&str; NUM_SCALAR_PARAMS] = [
    "knight", "bishop", "rook", "queen",
    "doubled_pawn_penalty", "isolated_pawn_penalty",
    "passed_pawn_bonus[rang2]", "passed_pawn_bonus[rang3]",
    "passed_pawn_bonus[rang4]", "passed_pawn_bonus[rang5]",
    "passed_pawn_bonus[rang6]", "passed_pawn_bonus[rang7]",
    "bishop_pair_bonus",
    "knight_mobility", "bishop_mobility", "rook_mobility", "queen_mobility",
    "shield_pawn_bonus", "king_center_penalty", "open_file_near_king_penalty",
    "center_pawn_bonus", "center_attack_bonus",
];

const IDX_KNIGHT: usize = 0;
const IDX_BISHOP: usize = 1;
const IDX_ROOK:   usize = 2;
const IDX_QUEEN:  usize = 3;
const IDX_DOUBLED:  usize = 4;
const IDX_ISOLATED: usize = 5;
const IDX_PASSED_BASE: usize = 6; // occupies indices 6 to 11 (ranks 2 to 7)
const IDX_BISHOP_PAIR: usize = 12;
const IDX_KNIGHT_MOB: usize = 13;
const IDX_BISHOP_MOB: usize = 14;
const IDX_ROOK_MOB:   usize = 15;
const IDX_QUEEN_MOB:  usize = 16;
const IDX_SHIELD_PAWN:        usize = 17;
const IDX_KING_CENTER_PEN:    usize = 18;
const IDX_OPEN_FILE_KING_PEN: usize = 19;
const IDX_CENTER_PAWN:  usize = 20;
const IDX_CENTER_ATTACK: usize = 21;

/// Tunable parameters, represented as a simple array of i32.
/// Deliberate choice (rather than a struct with named fields and
/// mutable references): a flat array avoids any construction
/// of nested borrows in the coordinate descent loop — simpler
/// to review and guarantee correct without being able to compile to check here.
#[derive(Clone, Debug)]
struct EvalParams {
    values: [i32; NUM_PARAMS],
}

impl EvalParams {
    /// Starting values = current constants of the production engine.
    /// Scalars: material.rs, pawns.rs, mobility.rs, king_safety.rs, center.rs.
    /// PST (v4): imported DIRECTLY from eval::tables (see the v4 header note
    /// above) — no manual copy-paste, therefore no risk of desynchronization
    /// between the tuner's starting values and production.
    fn default_from_engine() -> Self {
        // [0i32; NUM_PARAMS]: flat array, filled in slices below.
        let mut values = [0i32; NUM_PARAMS];

        let scalars: [i32; NUM_SCALAR_PARAMS] = [
            320, 330, 500, 900,     // knight, bishop, rook, queen
            -20, -20,                // doubled, isolated
            5, 10, 20, 35, 60, 100,  // passed[rank2..rank7]
            30,                      // bishop_pair_bonus
            4, 3, 2, 1,              // knight/bishop/rook/queen mobility
            10, -30, -15,            // shield_pawn, king_center_pen, open_file_pen
            15, 5,                   // center_pawn, center_attack
        ];
        values[0..NUM_SCALAR_PARAMS].copy_from_slice(&scalars);

        values[IDX_PST_PAWN_BASE   .. IDX_PST_PAWN_BASE   + 64].copy_from_slice(&PAWN_TABLE);
        values[IDX_PST_KNIGHT_BASE .. IDX_PST_KNIGHT_BASE + 64].copy_from_slice(&KNIGHT_TABLE);
        values[IDX_PST_BISHOP_BASE .. IDX_PST_BISHOP_BASE + 64].copy_from_slice(&BISHOP_TABLE);
        values[IDX_PST_ROOK_BASE   .. IDX_PST_ROOK_BASE   + 64].copy_from_slice(&ROOK_TABLE);
        values[IDX_PST_QUEEN_BASE  .. IDX_PST_QUEEN_BASE  + 64].copy_from_slice(&QUEEN_TABLE);

        EvalParams { values }
    }

    /// PST value from `params` for base table `base`, square `sq`
    /// (already returned from the color's point of view by the caller — see
    /// pst_score() which applies mirror_square() for Black before calling
    /// this function).
    #[inline]
    fn pst(&self, base: usize, sq: u8) -> i32 {
        self.values[base + sq as usize]
    }

    #[inline]
    fn knight(&self) -> i32 { self.values[IDX_KNIGHT] }
    #[inline]
    fn bishop(&self) -> i32 { self.values[IDX_BISHOP] }
    #[inline]
    fn rook(&self)   -> i32 { self.values[IDX_ROOK] }
    #[inline]
    fn queen(&self)  -> i32 { self.values[IDX_QUEEN] }
    #[inline]
    fn doubled_pawn_penalty(&self)  -> i32 { self.values[IDX_DOUBLED] }
    #[inline]
    fn isolated_pawn_penalty(&self) -> i32 { self.values[IDX_ISOLATED] }
    /// `advancement`: 1 (rank 2) to 6 (rank 7).
    #[inline]
    fn passed_pawn_bonus(&self, advancement: i32) -> i32 {
        self.values[IDX_PASSED_BASE + (advancement - 1) as usize]
    }
    #[inline]
    fn bishop_pair_bonus(&self) -> i32 { self.values[IDX_BISHOP_PAIR] }
    #[inline]
    fn knight_mobility(&self) -> i32 { self.values[IDX_KNIGHT_MOB] }
    #[inline]
    fn bishop_mobility(&self) -> i32 { self.values[IDX_BISHOP_MOB] }
    #[inline]
    fn rook_mobility(&self)   -> i32 { self.values[IDX_ROOK_MOB] }
    #[inline]
    fn queen_mobility(&self)  -> i32 { self.values[IDX_QUEEN_MOB] }
    #[inline]
    fn shield_pawn_bonus(&self) -> i32 { self.values[IDX_SHIELD_PAWN] }
    #[inline]
    fn king_center_penalty(&self) -> i32 { self.values[IDX_KING_CENTER_PEN] }
    #[inline]
    fn open_file_near_king_penalty(&self) -> i32 { self.values[IDX_OPEN_FILE_KING_PEN] }
    #[inline]
    fn center_pawn_bonus(&self) -> i32 { self.values[IDX_CENTER_PAWN] }
    #[inline]
    fn center_attack_bonus(&self) -> i32 { self.values[IDX_CENTER_ATTACK] }
}

// =============================================================================
// Tunable evaluation (material + pawn structure only, see header)
// =============================================================================

const PAWN_VALUE: i32 = 100; // fixed anchor, standard Texel Tuning convention

fn piece_value(params: &EvalParams, piece: Piece) -> i32 {
    match piece {
        Piece::Pawn   => PAWN_VALUE,
        Piece::Knight => params.knight(),
        Piece::Bishop => params.bishop(),
        Piece::Rook   => params.rook(),
        Piece::Queen  => params.queen(),
        Piece::King   => 0,
    }
}

fn material_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let mut score = 0i32;
    for piece in [Piece::Pawn, Piece::Knight, Piece::Bishop, Piece::Rook, Piece::Queen] {
        let count = board.pieces[color.index()][piece.index()].count_ones() as i32;
        score += count * piece_value(params, piece);
    }
    score
}

/// Pawn structure: doubled, isolated, passed — logic identical to
/// pawns.rs, but with the penalties/bonuses drawn from `params` instead of the
/// fixed constants (and the passed pawn table precomputed on every call,
/// here without the OnceLock table from pawns.rs — acceptable since not hot).
fn pawn_structure_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let pawns       = board.pieces[color.index()][Piece::Pawn.index()];
    let enemy_pawns = board.pieces[color.opposite().index()][Piece::Pawn.index()];
    let mut score   = 0i32;

    for file in 0u8..8 {
        let col_mask = 0x0101_0101_0101_0101u64 << file;
        let pawns_on_file = pawns & col_mask;
        let count = pawns_on_file.count_ones() as i32;
        if count == 0 { continue; }

        if count > 1 {
            score += params.doubled_pawn_penalty() * (count - 1);
        }

        let left  = if file > 0 { 0x0101_0101_0101_0101u64 << (file - 1) } else { 0 };
        let right = if file < 7 { 0x0101_0101_0101_0101u64 << (file + 1) } else { 0 };
        let adjacent = left | right;

        if pawns & adjacent == 0 {
            score += params.isolated_pawn_penalty() * count;
        }

        let mut bb = pawns_on_file;
        while bb != 0 {
            let sq = bb.trailing_zeros() as u8;
            bb &= bb - 1;
            let rank = sq / 8;

            let zone = col_mask | left | right;
            let front: u64 = match color {
                Color::White => {
                    let mut m = 0u64;
                    for r in (rank + 1)..8 { m |= 0x0000_0000_0000_00FFu64 << (r * 8) & zone; }
                    m
                }
                Color::Black => {
                    let mut m = 0u64;
                    for r in 0..rank { m |= 0x0000_0000_0000_00FFu64 << (r * 8) & zone; }
                    m
                }
            };

            if enemy_pawns & front == 0 {
                let advancement = match color {
                    Color::White => rank as i32,
                    Color::Black => 7 - rank as i32,
                };
                // advancement: 0=rank1(promoted, never happens), 1=rank2, ... 6=rank7, 7=rank8
                if (1..=6).contains(&advancement) {
                    score += params.passed_pawn_bonus(advancement);
                }
            }
        }
    }

    score
}

/// Piece-square tables (PST) — Pawn/Knight/Bishop/Rook/Queen only (see
/// v4 note in the header: the King is excluded, its two production MG/EG
/// tables cannot be faithfully represented by this tuner without phase
/// detection).
///
/// Convention IDENTICAL to eval/tables.rs::piece_square_values(): the square
/// is read directly for White, via mirror_square() for Black —
/// the tuned tables are therefore, as in production, "from White's point of view".
fn pst_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let mut score = 0i32;

    let mirrored = |sq: u8| -> u8 {
        if color == Color::White { sq } else { mirror_square(sq) }
    };

    let mut bb = board.pieces[color.index()][Piece::Pawn.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        score += params.pst(IDX_PST_PAWN_BASE, mirrored(sq));
    }

    let mut bb = board.pieces[color.index()][Piece::Knight.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        score += params.pst(IDX_PST_KNIGHT_BASE, mirrored(sq));
    }

    let mut bb = board.pieces[color.index()][Piece::Bishop.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        score += params.pst(IDX_PST_BISHOP_BASE, mirrored(sq));
    }

    let mut bb = board.pieces[color.index()][Piece::Rook.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        score += params.pst(IDX_PST_ROOK_BASE, mirrored(sq));
    }

    let mut bb = board.pieces[color.index()][Piece::Queen.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        score += params.pst(IDX_PST_QUEEN_BASE, mirrored(sq));
    }

    score
}

/// Bishop pair bonus — logic identical to material::bishop_pair_score().
fn bishop_pair_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let count = board.pieces[color.index()][Piece::Bishop.index()].count_ones();
    if count >= 2 { params.bishop_pair_bonus() } else { 0 }
}

/// Mobility of knights/bishops/rooks/queens — logic identical to
/// mobility::mobility_score(), bonuses drawn from `params` instead of constants.
fn mobility_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let own_pieces = board.occupancy[color.index()];
    let occupied   = board.all_pieces;
    let mut score  = 0i32;

    let mut knights = board.pieces[color.index()][Piece::Knight.index()];
    while knights != 0 {
        let sq = knights.trailing_zeros() as u8;
        knights &= knights - 1;
        let moves = knight_attacks(sq) & !own_pieces;
        score += moves.count_ones() as i32 * params.knight_mobility();
    }

    let mut bishops = board.pieces[color.index()][Piece::Bishop.index()];
    while bishops != 0 {
        let sq = bishops.trailing_zeros() as u8;
        bishops &= bishops - 1;
        let moves = bishop_attacks(sq, occupied) & !own_pieces;
        score += moves.count_ones() as i32 * params.bishop_mobility();
    }

    let mut rooks = board.pieces[color.index()][Piece::Rook.index()];
    while rooks != 0 {
        let sq = rooks.trailing_zeros() as u8;
        rooks &= rooks - 1;
        let moves = rook_attacks(sq, occupied) & !own_pieces;
        score += moves.count_ones() as i32 * params.rook_mobility();
    }

    let mut queens = board.pieces[color.index()][Piece::Queen.index()];
    while queens != 0 {
        let sq = queens.trailing_zeros() as u8;
        queens &= queens - 1;
        let moves = queen_attacks(sq, occupied) & !own_pieces;
        score += moves.count_ones() as i32 * params.queen_mobility();
    }

    score
}

/// King safety — logic identical to king_safety::king_safety_score(),
/// WITHOUT disabling in the endgame (the tuner has no phase detection —
/// see the note in the file header).
fn king_safety_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let mut score  = 0i32;
    let king_sq    = board.king_square(color);
    let king_file  = king_sq % 8;
    let pawns      = board.pieces[color.index()][Piece::Pawn.index()];

    let shield_area  = king_attacks(king_sq) | (1u64 << king_sq);
    let shield_count = (shield_area & pawns).count_ones() as i32;
    score += shield_count * params.shield_pawn_bonus();

    if (2..=5).contains(&king_file) {
        score += params.king_center_penalty();
    }

    let enemy_rooks_queens = board.pieces[color.opposite().index()][Piece::Rook.index()]
                           | board.pieces[color.opposite().index()][Piece::Queen.index()];
    if enemy_rooks_queens != 0 {
        for f in king_file.saturating_sub(1)..=(king_file + 1).min(7) {
            let col = file_mask(f);
            if pawns & col == 0 {
                score += params.open_file_near_king_penalty();
            }
        }
    }

    score
}

/// Center control (pawns + pieces) — logic identical to center.rs,
/// merged into a single function here (no need to separate it for mobility
/// as in the production engine, the tuner does not have this attack
/// computation sharing concern).
fn center_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    const CENTER_SQUARES: u64 = (1u64 << 27) | (1u64 << 28) | (1u64 << 35) | (1u64 << 36);
    let occupied = board.all_pieces;
    let mut score = 0i32;

    let pawns = board.pieces[color.index()][Piece::Pawn.index()];
    score += (pawns & CENTER_SQUARES).count_ones() as i32 * params.center_pawn_bonus();

    let pawn_attacks = if color == Color::White {
        white_pawn_attacks(pawns)
    } else {
        black_pawn_attacks(pawns)
    };
    score += (pawn_attacks & CENTER_SQUARES).count_ones() as i32 * params.center_attack_bonus();

    let mut knights = board.pieces[color.index()][Piece::Knight.index()];
    while knights != 0 {
        let sq = knights.trailing_zeros() as u8;
        knights &= knights - 1;
        score += (knight_attacks(sq) & CENTER_SQUARES).count_ones() as i32 * params.center_attack_bonus();
    }

    let mut bishops = board.pieces[color.index()][Piece::Bishop.index()];
    while bishops != 0 {
        let sq = bishops.trailing_zeros() as u8;
        bishops &= bishops - 1;
        score += (bishop_attacks(sq, occupied) & CENTER_SQUARES).count_ones() as i32 * params.center_attack_bonus();
    }

    let mut rooks = board.pieces[color.index()][Piece::Rook.index()];
    while rooks != 0 {
        let sq = rooks.trailing_zeros() as u8;
        rooks &= rooks - 1;
        score += (rook_attacks(sq, occupied) & CENTER_SQUARES).count_ones() as i32 * params.center_attack_bonus();
    }

    let mut queens = board.pieces[color.index()][Piece::Queen.index()];
    while queens != 0 {
        let sq = queens.trailing_zeros() as u8;
        queens &= queens - 1;
        score += (queen_attacks(sq, occupied) & CENTER_SQUARES).count_ones() as i32 * params.center_attack_bonus();
    }

    score
}

/// Total score (material + pawn structure + mobility + king safety +
/// center + bishop pair), from White's point of view.
/// Always simpler than evaluate() (no PST, no dedicated endgame,
/// no phase) — see v2 scope in the file header.
fn tunable_eval_white_pov(params: &EvalParams, board: &Board) -> i32 {
    let mut white = 0i32;
    let mut black = 0i32;

    for color in [Color::White, Color::Black] {
        let total = material_score(params, board, color)
            + pawn_structure_score(params, board, color)
            + pst_score(params, board, color)
            + bishop_pair_score(params, board, color)
            + mobility_score(params, board, color)
            + king_safety_score(params, board, color)
            + center_score(params, board, color);

        if color == Color::White { white = total; } else { black = total; }
    }

    white - black
}

// =============================================================================
// Error function (Texel Tuning)
// =============================================================================

/// Sigmoid scale — calibrated on THE DATA (see calibrate_k() further
/// down) rather than fixed to the "historical" value of 400.
///
/// Why this is essential and not a minor detail:
///   The v1 and v2 tunings both converged toward a scale
///   collapse (material divided by ~2) AND an inconsistent sign on the
///   passed pawn bonuses on the early ranks. The cause of the collapse: K=400
///   had never been calibrated for THIS model and THIS exact data — the
///   original Texel method calibrates K first (1D search on the
///   starting values), BEFORE touching the evaluation weights, precisely
///   to prevent the optimizer from compensating for a bad scale calibration
///   by shrinking all the other parameters. This was the missing step.
#[inline]
fn sigmoid(score: i32, k: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf(-(score as f64) / k))
}

/// Pre-loaded position: board already parsed + actual game result
/// (from White's point of view: 1.0, 0.5 or 0.0).
struct Sample {
    board:  Board,
    result: f64,
}

/// Computes the mean squared error over the whole dataset for a
/// given set of parameters. This is the function called on EVERY candidate
/// value trial in the coordinate descent loop — its cost dominates
/// the total tuning time.
///
/// Sequential version — kept for small datasets or the
/// num_threads == 1 case. The version used by default is total_error()
/// below (parallel).
fn total_error_sequential(params: &EvalParams, samples: &[Sample], k: f64) -> f64 {
    let mut sum = 0.0f64;
    for s in samples {
        let eval = tunable_eval_white_pov(params, &s.board);
        let pred = sigmoid(eval, k);
        let diff = s.result - pred;
        sum += diff * diff;
    }
    sum / samples.len() as f64
}

/// Computes the mean squared error by splitting the dataset
/// across `num_threads` threads — each thread processes a contiguous and
/// independent slice of the Vec<Sample>, with no shared writes at all (local sum
/// per thread, combined at the end). This is an "embarrassingly
/// parallel" parallelization: no lock, no coordination during the computation.
///
/// Expected speedup: close to the number of available cores, since each
/// position is independent of the others (unlike the engine's
/// alpha-beta search, where Lazy SMP has to deal with redundant
/// work between threads — here, zero redundancy).
fn total_error(params: &EvalParams, samples: &[Sample], num_threads: usize, k: f64) -> f64 {
    if num_threads <= 1 || samples.len() < num_threads * 1000 {
        return total_error_sequential(params, samples, k);
    }

    // Integer division rounded up (each thread handles a block, the
    // last one possibly smaller). div_ceil has been stable since Rust 1.73.
    let chunk_size = samples.len().div_ceil(num_threads);

    std::thread::scope(|scope| {
        let handles: Vec<_> = samples
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut sum = 0.0f64;
                    for s in chunk {
                        let eval = tunable_eval_white_pov(params, &s.board);
                        let pred = sigmoid(eval, k);
                        let diff = s.result - pred;
                        sum += diff * diff;
                    }
                    sum
                })
            })
            .collect();

        let total: f64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        total / samples.len() as f64
    })
}

/// Calibrates the sigmoid scale K by minimizing the error on the
/// dataset, with FIXED EVALUATION PARAMETERS (the starting values, see
/// the note on sigmoid() above). Ternary search: works because
/// the error as a function of K, for a given eval, is unimodal (a single
/// minimum) — typical of this kind of scale calibration.
///
/// This step must be performed ONCE, before the coordinate
/// descent loop on the weights — never during, at the risk of reintroducing the
/// confusion between "the right K" and "the right weights" that caused the scale
/// collapse observed in the tuner's previous versions.
fn calibrate_k(params: &EvalParams, samples: &[Sample], num_threads: usize) -> f64 {
    let mut lo = 50.0f64;
    let mut hi = 1000.0f64;

    // ~30 iterations are more than enough to reach a precision of 0.01
    // over this range (each iteration reduces the interval by a factor of 2/3);
    // the bound of 60 is a safety margin, the early exit does the work.
    for _ in 0..60 {
        if (hi - lo) < 0.01 { break; }
        let m1 = lo + (hi - lo) / 3.0;
        let m2 = hi - (hi - lo) / 3.0;
        let e1 = total_error(params, samples, num_threads, m1);
        let e2 = total_error(params, samples, num_threads, m2);
        if e1 < e2 {
            hi = m2;
        } else {
            lo = m1;
        }
    }

    (lo + hi) / 2.0
}

// =============================================================================
// Loading the dataset
// =============================================================================

fn load_samples(path: &str) -> Vec<Sample> {
    let file   = File::open(path).expect("Impossible d'ouvrir le fichier de positions");
    let reader = BufReader::with_capacity(4 << 20, file);

    let mut samples = Vec::with_capacity(2_000_000);
    let mut skipped = 0u64;

    for line in reader.lines() {
        let line = line.expect("Erreur de lecture");
        let Some(sep) = line.rfind(';') else { skipped += 1; continue; };
        let fen_str    = &line[..sep];
        let result_str = &line[sep + 1..];

        let Ok(result) = result_str.parse::<f64>() else { skipped += 1; continue; };
        let Ok(board)  = Board::from_fen(fen_str) else { skipped += 1; continue; };

        samples.push(Sample { board, result });
    }

    eprintln!("Positions chargées : {}", samples.len());
    if skipped > 0 {
        eprintln!("Lignes ignorées    : {} (FEN ou résultat invalide)", skipped);
    }
    samples
}

// =============================================================================
// Entry point
// =============================================================================

/// Displays the current state: error, elapsed time, and the
/// SCALAR parameters only (22, see PARAM_NAMES) — the 320 PST parameters
/// (v4) are deliberately omitted here: a dump of 320 values every
/// PRINT_EVERY passes would be unreadable. See print_pst_table() for
/// the final display, formatted and much more useful (ready to copy-paste).
///
/// BUG AVOIDED: the previous version (v1-v3) looped over `0..NUM_PARAMS`
/// while indexing PARAM_NAMES[i] — with NUM_PARAMS now at 342 (v4) versus
/// PARAM_NAMES.len() == 22, this would have panicked (index out of bounds) on the
/// very first call. Fixed by explicitly looping over NUM_SCALAR_PARAMS.
fn print_status(pass: u32, error: f64, elapsed_s: f64, params: &EvalParams) {
    eprintln!(
        "── Passe {:>4} — erreur = {:.6} — {:.1}s écoulées ──",
        pass, error, elapsed_s
    );
    for (name, value) in PARAM_NAMES.iter().zip(params.values.iter()).take(NUM_SCALAR_PARAMS) {
        eprintln!("    {:<28} = {}", name, value);
    }
    eprintln!("    (320 paramètres PST omis ici — voir le rapport final)");
}

/// Displays a tuned PST table, formatted as a Rust array ready to copy
/// directly into eval/tables.rs (8 values per line, aligned to 4
/// characters — same presentation as the current tables in the file).
fn print_pst_table(name: &str, params: &EvalParams, base: usize) {
    eprintln!("pub const {}: [i32; 64] = [", name);
    for rank in 0..8 {
        let row: Vec<String> = (0..8)
            .map(|file| format!("{:4}", params.values[base + rank * 8 + file]))
            .collect();
        eprintln!("    {},", row.join(","));
    }
    eprintln!("];");
}

/// Frequency of parameter detail display (in number of passes).
///
/// Value 100 (inherited from v1/v2/v3) lowered to 5 for v4: with 342
/// parameters (~15.5x more expensive per pass than in v3, see the v4
/// header note), 100 passes can represent 20-30 minutes of total silence in
/// the terminal — to the point of (wrongly) giving the impression that the program
/// is stuck. At 5 passes, visual feedback appears within a few dozen
/// seconds to a few minutes depending on the hardware, enough to confirm
/// that it's progressing without flooding the output the way a display on every
/// pass would.
const PRINT_EVERY: u32 = 5;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage : {} <positions.txt>", args[0]);
        std::process::exit(1);
    }

    init_attack_tables();

    eprintln!("Chargement du jeu de données...");
    let load_start = Instant::now();
    let samples = load_samples(&args[1]);
    eprintln!("Chargé en {:.1}s", load_start.elapsed().as_secs_f64());

    // All positions are already in RAM in `samples` (Vec<Sample>) —
    // no pass touches the disk again. The only remaining speed lever
    // is the parallelization of the error computation itself across the available
    // cores (independent sum per position, with no coordination).
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    eprintln!("Threads utilisés  : {}", num_threads);
    eprintln!();

    let params0 = EvalParams::default_from_engine();

    // --- K calibration (see calibrate_k() for the detailed reasoning) ---
    // MANDATORY step before tuning the weights: without it, the optimizer
    // compensates for a bad scale calibration by shrinking the weights instead
    // of truly calibrating them — this is what caused the collapse
    // observed in the tuner's previous versions.
    eprintln!("Calibrage de l'échelle K (recherche ternaire sur les valeurs de départ)...");
    let calib_start = Instant::now();
    let k = calibrate_k(&params0, &samples, num_threads);
    eprintln!("K calibré = {:.2} (référence historique : 400) — {:.1}s", k, calib_start.elapsed().as_secs_f64());
    eprintln!();

    let mut params = params0;

    let mut best_error = total_error(&params, &samples, num_threads, k);
    eprintln!("Erreur initiale (valeurs actuelles du moteur, K calibré) : {:.6}", best_error);
    eprintln!();

    // No adjustment: ±1 on the centipawn scale — fine enough
    // for these parameters, standard Texel Tuning convention.
    const STEP: i32 = 1;
    let mut improved_any = true;
    let mut pass = 0u32;
    let tune_start = Instant::now();

    while improved_any {
        improved_any = false;
        pass += 1;

        for idx in 0..NUM_PARAMS {
            // --- Try +STEP ---
            params.values[idx] += STEP;
            let err_plus = total_error(&params, &samples, num_threads, k);

            if err_plus < best_error {
                best_error = err_plus;
                improved_any = true;
                continue; // keep +STEP, move to the next parameter
            }

            // --- Undo the +STEP, then try -STEP ---
            params.values[idx] -= 2 * STEP; // amounts to -STEP relative to the original
            let err_minus = total_error(&params, &samples, num_threads, k);

            if err_minus < best_error {
                best_error = err_minus;
                improved_any = true;
            } else {
                // Neither +STEP nor -STEP helps: revert to the original value.
                params.values[idx] += STEP;
            }
        }

        if pass.is_multiple_of(PRINT_EVERY) {
            print_status(pass, best_error, tune_start.elapsed().as_secs_f64(), &params);
        }
    }

    eprintln!();
    eprintln!("Convergence atteinte après {} passe(s).", pass);
    eprintln!();
    print_status(pass, best_error, tune_start.elapsed().as_secs_f64(), &params);
    eprintln!();
    eprintln!("=== Nouvelles valeurs (à reporter manuellement dans le code) ===");
    eprintln!("material.rs PIECE_VALUE :");
    eprintln!("  Pion     = {} (fixe, ancre)", PAWN_VALUE);
    eprintln!("  Cavalier = {}", params.knight());
    eprintln!("  Fou      = {}", params.bishop());
    eprintln!("  Tour     = {}", params.rook());
    eprintln!("  Dame     = {}", params.queen());
    eprintln!();
    eprintln!("pawns.rs :");
    eprintln!("  DOUBLED_PAWN_PENALTY  = {}", params.doubled_pawn_penalty());
    eprintln!("  ISOLATED_PAWN_PENALTY = {}", params.isolated_pawn_penalty());
    eprintln!(
        "  PASSED_PAWN_BONUS = [0, {}, {}, {}, {}, {}, {}, 0]",
        params.passed_pawn_bonus(1), params.passed_pawn_bonus(2), params.passed_pawn_bonus(3),
        params.passed_pawn_bonus(4), params.passed_pawn_bonus(5), params.passed_pawn_bonus(6),
    );
    eprintln!();
    eprintln!("material.rs :");
    eprintln!("  BISHOP_PAIR_BONUS = {}", params.bishop_pair_bonus());
    eprintln!();
    eprintln!("mobility.rs :");
    eprintln!("  KNIGHT_MOBILITY_BONUS = {}", params.knight_mobility());
    eprintln!("  BISHOP_MOBILITY_BONUS = {}", params.bishop_mobility());
    eprintln!("  ROOK_MOBILITY_BONUS   = {}", params.rook_mobility());
    eprintln!("  QUEEN_MOBILITY_BONUS  = {}", params.queen_mobility());
    eprintln!();
    eprintln!("king_safety.rs :");
    eprintln!("  SHIELD_PAWN_BONUS           = {}", params.shield_pawn_bonus());
    eprintln!("  KING_CENTER_PENALTY         = {}", params.king_center_penalty());
    eprintln!("  OPEN_FILE_NEAR_KING_PENALTY = {}", params.open_file_near_king_penalty());
    eprintln!();
    eprintln!("center.rs :");
    eprintln!("  CENTER_PAWN_BONUS  = {}", params.center_pawn_bonus());
    eprintln!("  CENTER_ATTACK_BONUS = {}", params.center_attack_bonus());
    eprintln!();
    eprintln!("eval/tables.rs (v4 — PST, Roi exclu, voir note d'en-tête) :");
    eprintln!();
    print_pst_table("PAWN_TABLE",   &params, IDX_PST_PAWN_BASE);
    eprintln!();
    print_pst_table("KNIGHT_TABLE", &params, IDX_PST_KNIGHT_BASE);
    eprintln!();
    print_pst_table("BISHOP_TABLE", &params, IDX_PST_BISHOP_BASE);
    eprintln!();
    print_pst_table("ROOK_TABLE",   &params, IDX_PST_ROOK_BASE);
    eprintln!();
    print_pst_table("QUEEN_TABLE",  &params, IDX_PST_QUEEN_BASE);
}
