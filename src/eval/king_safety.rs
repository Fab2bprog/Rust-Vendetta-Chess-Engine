// =============================================================================
// Vendetta Chess Motor — src/eval/king_safety.rs
//
// Role: King safety evaluation.
//        A poorly protected king is a major weakness in chess.
//        This module evaluates the quality of the pawn shield in front of the king
//        and direct threats.
//
// Contents:
//   - Evaluation of the pawn shield (pawns in front of the king after castling)
//   - Penalty for a king in the center in the middlegame
//   - Detection of open files near the king (danger)
//
// Simplified but effective approach:
//   - We look at the side's pawns in front of the king
//   - The more pawns nearby, the safer the king
//   - A king in the center in the middlegame is penalized
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::{king_attacks, file_mask};

/// Bonus per shield pawn present (pawn in front of the king).
/// Calibrated by Texel Tuning v3 (was 10) — see material.rs::PIECE_VALUE.
const SHIELD_PAWN_BONUS: i32 = 14;

/// Penalty for a king in the center (files c, d, e, f) in the middlegame.
/// Calibrated by Texel Tuning v3 (was -30). The most significant change in the
/// tuning — to be monitored particularly closely during the real-game A/B test.
const KING_CENTER_PENALTY: i32 = -7;

/// Penalty for an open or semi-open file next to the king.
/// Calibrated by Texel Tuning v3 (was -15).
const OPEN_FILE_NEAR_KING_PENALTY: i32 = -21;

/// Evaluates king safety for a given color.
/// Returns a score (positive = good for this color).
pub fn king_safety_score(board: &Board, color: Color, is_endgame: bool) -> i32 {
    // In the endgame, king safety is less important
    if is_endgame {
        return 0;
    }

    let mut score = 0i32;
    let king_sq   = board.king_square(color);
    let king_file = king_sq % 8;
    let pawns     = board.pieces[color.index()][Piece::Pawn.index()];

    // --- Pawn shield ---
    // The squares directly in front of the king (and diagonally) should have pawns.
    // Count the pawns in the king zone (shield)
    let shield_pawns_area = king_attacks(king_sq) | (1u64 << king_sq);
    let shield_count = (shield_pawns_area & pawns).count_ones() as i32;
    score += shield_count * SHIELD_PAWN_BONUS;

    // --- Penalty for king in the center ---
    // Files c(2), d(3), e(4), f(5) are central
    if (2..=5).contains(&king_file) {
        score += KING_CENTER_PENALTY;
    }

    // --- Open files near the king ---
    // Check the files adjacent to the king
    let enemy_rooks_queens = board.pieces[color.opposite().index()][Piece::Rook.index()]
                           | board.pieces[color.opposite().index()][Piece::Queen.index()];

    if enemy_rooks_queens != 0 {
        for f in king_file.saturating_sub(1)..=(king_file + 1).min(7) {
            let col = file_mask(f);
            // Open file if no friendly pawn on it
            if pawns & col == 0 {
                score += OPEN_FILE_NEAR_KING_PENALTY;
            }
        }
    }

    score
}

/// Computes the king safety differential from the active player's point of view.
pub fn king_safety_eval(board: &Board, is_endgame: bool) -> i32 {
    let white_score = king_safety_score(board, Color::White, is_endgame);
    let black_score = king_safety_score(board, Color::Black, is_endgame);
    let diff = white_score - black_score;

    if board.side_to_move == Color::White { diff } else { -diff }
}

// =============================================================================
// King safety by ATTACK (king danger) — new term
// =============================================================================
//
// Complements the pawn shield above, which does NOT see enemy pieces
// massed around the king. Classic idea (CPW "king safety" / Stockfish king
// danger): the more enemy pieces attacking the king's ZONE, and
// the heavier they are, the more the king is in danger — and this danger rises
// NON-LINEARLY (two coordinated attackers are worth much more than double
// a single one). The "attack units" (weighted sum of zone squares
// attacked) are accumulated in the mobility pass (eval/mobility.rs), which
// already computes the attack bitboards — see king_attack_danger() below
// for the units → penalty conversion.

/// Attack weight per piece type (per attacked square of the king zone).
/// A heavy piece near the king is far more threatening than a light piece.
pub const KING_ATTACK_WEIGHT_KNIGHT: i32 = 2;
pub const KING_ATTACK_WEIGHT_BISHOP: i32 = 2;
pub const KING_ATTACK_WEIGHT_ROOK:   i32 = 3;
pub const KING_ATTACK_WEIGHT_QUEEN:  i32 = 5;

/// Divisor for the quadratic rise of danger (larger = more cautious).
/// Chosen setting: 16 (conservative). The more aggressive v2 (10) was tested
/// via SPRT and FAILED (−0.5 vs +3.1) → pushes the over-attack term too far and loses Elo.
const KING_DANGER_DIV: i32 = 16;
/// Danger cap (centipawns) to avoid outlandish evaluations that would
/// push toward dubious sacrifices. Chosen setting: 100 (v2 at 150: FAIL).
const KING_DANGER_CAP: i32 = 100;

/// Converts "attack units" (and the number of attackers) into a danger
/// penalty for the side whose king is targeted (= bonus for the attacker), in
/// centipawns, positive.
///
/// Requires AT LEAST 2 attackers: a single piece touching the king zone
/// is not a real attack (otherwise we would penalize noise). Bounded
/// quadratic rise (CONSERVATIVE — to be tuned via SPRT).
#[inline]
pub fn king_attack_danger(units: i32, attackers: i32) -> i32 {
    if attackers < 2 || units <= 0 {
        return 0;
    }
    (units * units / KING_DANGER_DIV).min(KING_DANGER_CAP)
}
