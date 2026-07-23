// =============================================================================
// Vendetta Chess Motor — src/eval/material.rs
//
// Role: Defines the material values of the pieces and calculates the
//        material imbalance between the two sides.
//
// Contents:
//   - Piece values in centipawns (100 centipawns = 1 pawn)
//   - Calculation of the material score for a color
//   - Calculation of the material differential (White - Black, from the player's point of view)
//
// Score convention:
//   - Positive score → favorable to the player to move
//   - Negative score → unfavorable to the player to move
//   - Values are in centipawns (standard unit for chess engines)
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;

/// Piece values in centipawns.
///
/// Calibrated by Texel Tuning (v3, sigmoid scale K=748.22 calibrated on
/// the data before weight tuning — see src/bin/tuner.rs) on
/// 2,464,785 positions from 302,864 Lichess Rapid/Classical games,
/// Elo ≥ 2100 (May 2026 dump). Validated: consistent ratios between pieces
/// (Bishop > Knight, Rook well above, Queen the strongest) and bonus for
/// passed pawns strictly positive and increasing — unlike two
/// previous tuning attempts (without K calibration) that had produced
/// a scale collapse and passed pawn bonuses with inconsistent sign.
///
/// Old values (before tuning): Pawn=100, Knight=320, Bishop=330,
/// Rook=500, Queen=900 — kept as a comment for rollback if the
/// real-game A/B test does not confirm the improvement.
pub const PIECE_VALUE: [i32; 6] = [
    100,   // Pawn (fixed anchor for tuning, not adjusted)
    216,   // Knight (was 320)
    224,   // Bishop (was 330)
    382,   // Rook (was 500)
    817,   // Queen (was 900)
    20000, // King (very high value to force its protection — not affected by tuning)
];

/// Returns the value of a piece in centipawns.
#[inline]
pub fn piece_value(piece: Piece) -> i32 {
    PIECE_VALUE[piece.index()]
}

/// Calculates the total material score for a given color.
/// Sums the values of all pieces of that color (excluding the king).
pub fn material_score(board: &Board, color: Color) -> i32 {
    let mut score = 0i32;

    // We add up the value of each piece type (excluding the king)
    for piece in [Piece::Pawn, Piece::Knight, Piece::Bishop, Piece::Rook, Piece::Queen] {
        let count = board.pieces[color.index()][piece.index()].count_ones() as i32;
        score += count * piece_value(piece);
    }

    score
}

/// Calculates the material differential from the point of view of the player to move.
/// Positive score → advantage for the active player.
pub fn material_eval(board: &Board) -> i32 {
    let white_score = material_score(board, Color::White);
    let black_score = material_score(board, Color::Black);
    let diff = white_score - black_score;

    // Return from the point of view of the active player
    if board.side_to_move == Color::White { diff } else { -diff }
}

/// Calculates the total material present on the board (for phase detection).
/// Returns the sum of the values of all pieces (excluding kings and pawns).
pub fn non_pawn_material(board: &Board) -> i32 {
    let mut total = 0i32;
    for color in [Color::White, Color::Black] {
        for piece in [Piece::Knight, Piece::Bishop, Piece::Rook, Piece::Queen] {
            let count = board.pieces[color.index()][piece.index()].count_ones() as i32;
            total += count * piece_value(piece);
        }
    }
    total
}

/// Bonus for possessing both bishops (bishop pair).
/// The bishop pair is a well-established strategic advantage: together, the two
/// bishops cover all colors and dominate open positions.
/// Calibrated by Texel Tuning v3 (was 30) — see PIECE_VALUE above.
const BISHOP_PAIR_BONUS: i32 = 50;

/// Returns the bishop pair bonus for a given color.
/// Bonus granted if the side has at least 2 bishops.
pub fn bishop_pair_score(board: &Board, color: Color) -> i32 {
    let bishop_count = board.pieces[color.index()][Piece::Bishop.index()].count_ones();
    if bishop_count >= 2 { BISHOP_PAIR_BONUS } else { 0 }
}

/// Calculates the bishop pair differential from the point of view of the active player.
pub fn bishop_pair_eval(board: &Board) -> i32 {
    let white_score = bishop_pair_score(board, Color::White);
    let black_score = bishop_pair_score(board, Color::Black);
    let diff        = white_score - black_score;

    if board.side_to_move == Color::White { diff } else { -diff }
}

/// Returns true if the player of the given color has enough material
/// to mate (excludes cases of insufficient material).
pub fn has_mating_material(board: &Board, color: Color) -> bool {
    // King alone → cannot mate
    let non_king = board.occupancy[color.index()]
        & !board.pieces[color.index()][Piece::King.index()];

    if non_king == 0 {
        return false;
    }

    // King + lone knight or king + lone bishop → insufficient
    let count = non_king.count_ones();
    if count == 1 {
        // A single piece other than the king
        if board.pieces[color.index()][Piece::Knight.index()] != 0 { return false; }
        if board.pieces[color.index()][Piece::Bishop.index()] != 0 { return false; }
    }

    true
}
