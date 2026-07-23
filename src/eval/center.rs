// =============================================================================
// Vendetta Chess Motor — src/eval/center.rs
//
// Role: Evaluation of control of the center of the chessboard.
//        Controlling the center (d4, d5, e4, e5) is a fundamental principle
//        in chess: pieces in the center have more mobility and can
//        intervene on both flanks.
//
// Two criteria are evaluated:
//   1. Physical presence: pawns on the central squares (direct bonus)
//   2. Attacks: number of times the side attacks the 4 central squares
//
// Central squares:
//   d4 = square 27, e4 = square 28, d5 = square 35, e5 = square 36
//
// Bonus (in centipawns):
//   - Pawn present on a central square   : +15
//   - Attack on a central square          : +5
//
// Note: multiple attacks on the same square are counted
// separately (a rook and a bishop attacking e4 = 2×5 = +10).
//
// Split of the computation (optimization — see eval/mobility.rs):
//   This file now only handles the "pawns" part (presence + attacks).
//   The "piece attacks" part (knight/bishop/rook/queen) on the center
//   has been merged into mobility.rs: these pieces are already iterated over there and
//   their attack bitboard already computed for mobility — reusing this
//   same bitboard for the center bonus avoids recomputing it a second
//   time (same magic bitboard lookup, twice, for nothing). Pawns are
//   never handled by mobility.rs, so no merge is possible/useful
//   here: this module remains solely responsible for their contribution to the center.
//   CENTER_SQUARES and CENTER_ATTACK_BONUS are exposed (pub(crate)) so
//   that mobility.rs can reuse them without duplicating these constants.
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::{Bitboard, white_pawn_attacks, black_pawn_attacks};

/// Mask of the 4 central squares: d4(27), e4(28), d5(35), e5(36).
/// Visible from mobility.rs for the merged computation of the center bonus
/// for pieces (knight/bishop/rook/queen).
pub(crate) const CENTER_SQUARES: Bitboard =
    (1u64 << 27) | (1u64 << 28) | (1u64 << 35) | (1u64 << 36);

/// Bonus for a pawn physically present on a central square.
/// Calibrated by Texel Tuning v3 (was 15) — see material.rs::PIECE_VALUE.
const CENTER_PAWN_BONUS: i32 = 9;

/// Bonus per attack on a central square.
/// Visible from mobility.rs (see note above).
/// Calibrated by Texel Tuning v3 (was 5).
pub(crate) const CENTER_ATTACK_BONUS: i32 = 6;

/// Computes the center control score by PAWNS only for
/// a given color (presence + attacks). The contribution of the other
/// pieces is computed in mobility.rs (see file header).
/// Returns a positive score = good center control for this color.
pub fn center_pawn_score(board: &Board, color: Color) -> i32 {
    let mut score = 0i32;

    // --- Pawns physically in the center (strong direct bonus) ---
    let pawns = board.pieces[color.index()][Piece::Pawn.index()];
    score += (pawns & CENTER_SQUARES).count_ones() as i32 * CENTER_PAWN_BONUS;

    // --- Pawn attacks on the center ---
    let pawn_attacks = if color == Color::White {
        white_pawn_attacks(pawns)
    } else {
        black_pawn_attacks(pawns)
    };
    score += (pawn_attacks & CENTER_SQUARES).count_ones() as i32 * CENTER_ATTACK_BONUS;

    score
}

/// Computes the center control differential (pawns only) from the
/// point of view of the active player. The contribution of the pieces is added
/// separately by mobility::mobility_and_center_eval() — see eval/mod.rs.
/// Positive score = better center control for the active player.
pub fn center_pawn_eval(board: &Board) -> i32 {
    let white_score = center_pawn_score(board, Color::White);
    let black_score = center_pawn_score(board, Color::Black);
    let diff        = white_score - black_score;

    if board.side_to_move == Color::White { diff } else { -diff }
}
