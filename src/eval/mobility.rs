// =============================================================================
// Vendetta Chess Motor — src/eval/mobility.rs
//
// Role: Evaluation of piece mobility AND center control by
//        the pieces (knight, bishop, rook, queen) — both criteria are
//        calculated in a single pass per piece, see optimization note
//        below.
//
// Method:
//   For each piece (knight, bishop, rook, queen), we count the number of
//   squares it can reach without being blocked by its own pieces.
//   Pawns and the king are excluded: their mobility is handled elsewhere
//   (pawn structure, king safety).
//
// Mobility bonus per accessible square (in centipawns):
//   - Knight : 4  (very sensitive to its mobility, a knight in the corner is weak)
//   - Bishop  : 3  (its strength depends on its open diagonals)
//   - Rook    : 2  (needs open files)
//   - Queen   : 1  (already very mobile, little marginal bonus)
//
// Optimization — merge with center control (eval/center.rs):
//   Before: mobility.rs AND center.rs EACH independently calculated,
//   the attack bitboard of each knight/bishop/rook/queen (knight_attacks,
//   bishop_attacks, rook_attacks, queen_attacks — the latter costly because it
//   combines two magic bitboard lookups). Result: the same calculation done
//   twice per piece and per evaluation node.
//   After: the raw attack bitboard of each piece is calculated ONLY ONCE
//   here, then reused for BOTH bonuses:
//     - mobility: attacks AND squares occupied by a friendly piece (!own_pieces)
//     - center  : attacks AND the 4 central squares (CENTER_SQUARES),
//                  WITHOUT excluding friendly squares — behavior identical to
//                  the old center.rs (defending the center counts as much as
//                  attacking it).
//   The numerical result of each bonus is rigorously unchanged
//   compared to the two separate functions — only the number of attack calculations
//   is halved. Pawns are not affected (handled solely by
//   center::center_pawn_eval(), no pawn mobility evaluated here).
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::{knight_attacks, bishop_attacks, rook_attacks, queen_attacks, king_attacks};
use super::center::{CENTER_SQUARES, CENTER_ATTACK_BONUS};
use super::king_safety::{
    king_attack_danger,
    KING_ATTACK_WEIGHT_KNIGHT, KING_ATTACK_WEIGHT_BISHOP,
    KING_ATTACK_WEIGHT_ROOK, KING_ATTACK_WEIGHT_QUEEN,
};

/// Result of the per-piece pass for a color: mobility, center
/// control, and pressure on the opponent's king (attack units + number of attackers).
pub struct PieceActivity {
    pub mobility:        i32,
    pub center:          i32,
    pub king_units:      i32, // weighted sum of attacked squares in the opponent king's zone
    pub king_attackers:  i32, // number of distinct pieces touching this zone
}

/// Builds the bitboard of the "king zone" of the `defender` side: squares
/// adjacent to the king (+ its square), extended one rank forward (the side from which
/// the enemy attacks). The vertical shifts naturally discard bits
/// off the board; no east-west overflow is possible.
#[inline]
fn king_zone(board: &Board, defender: Color) -> u64 {
    let ksq = board.king_square(defender);
    let adj = king_attacks(ksq) | (1u64 << ksq);
    adj | if defender == Color::White { adj << 8 } else { adj >> 8 }
}

/// Pressure of a piece on the opponent king's zone, from its bitboard
/// of attacks ALREADY calculated for mobility. Returns (attacker ? 1 : 0,
/// attack units) — an AND + a popcount, almost free.
#[inline]
fn king_pressure(attacks: u64, enemy_zone: u64, weight: i32) -> (i32, i32) {
    let hits = (attacks & enemy_zone).count_ones() as i32;
    if hits > 0 { (1, hits * weight) } else { (0, 0) }
}

/// Mobility bonus per accessible square, depending on piece type.
/// Calibrated by Texel Tuning v3 (were 4, 3, 2, 1) — see
/// material.rs::PIECE_VALUE for the full tuning context.
const KNIGHT_MOBILITY_BONUS: i32 = 11;
const BISHOP_MOBILITY_BONUS: i32 = 10;
const ROOK_MOBILITY_BONUS:   i32 = 10;
const QUEEN_MOBILITY_BONUS:  i32 = 5;

/// Calculates, for a color and in ONE pass over the pieces, the mobility, the
/// center control, and the pressure on the opponent's king (see PieceActivity) —
/// all from the same per-piece attack bitboard.
///
/// `include_center`: if false, the center bonus is neither accumulated nor calculated
/// (endgame — reproduces the old behavior, see eval/mod.rs).
/// `king_attack`: if false, the pressure on the king is neither accumulated nor
/// calculated (endgame, or term disabled for an SPRT test) → zero work.
pub fn mobility_and_center_score(
    board: &Board,
    color: Color,
    include_center: bool,
    king_attack: bool,
) -> PieceActivity {
    let own_pieces   = board.occupancy[color.index()];
    let occupied     = board.all_pieces;
    let mut mobility = 0i32;
    let mut center   = 0i32;
    let mut king_units     = 0i32;
    let mut king_attackers = 0i32;

    // Zone of the OPPONENT's king (what `color` threatens). Calculated only if the
    // king-attack term is active (not endgame / not disabled) — otherwise zero work.
    let enemy_zone = if king_attack { king_zone(board, color.opposite()) } else { 0 };

    // --- Knights ---
    // A knight stuck in a corner loses a lot of power.
    let mut knights = board.pieces[color.index()][Piece::Knight.index()];
    while knights != 0 {
        let sq      = knights.trailing_zeros() as u8;
        knights    &= knights - 1;
        let attacks = knight_attacks(sq);
        mobility   += (attacks & !own_pieces).count_ones() as i32 * KNIGHT_MOBILITY_BONUS;
        if include_center {
            center += (attacks & CENTER_SQUARES).count_ones() as i32 * CENTER_ATTACK_BONUS;
        }
        if king_attack {
            let (a, u) = king_pressure(attacks, enemy_zone, KING_ATTACK_WEIGHT_KNIGHT);
            king_attackers += a; king_units += u;
        }
    }

    // --- Bishops ---
    // Open diagonals are the bishop's strength.
    let mut bishops = board.pieces[color.index()][Piece::Bishop.index()];
    while bishops != 0 {
        let sq      = bishops.trailing_zeros() as u8;
        bishops    &= bishops - 1;
        let attacks = bishop_attacks(sq, occupied);
        mobility   += (attacks & !own_pieces).count_ones() as i32 * BISHOP_MOBILITY_BONUS;
        if include_center {
            center += (attacks & CENTER_SQUARES).count_ones() as i32 * CENTER_ATTACK_BONUS;
        }
        if king_attack {
            let (a, u) = king_pressure(attacks, enemy_zone, KING_ATTACK_WEIGHT_BISHOP);
            king_attackers += a; king_units += u;
        }
    }

    // --- Rooks ---
    // Rooks need open files and ranks to be active.
    let mut rooks = board.pieces[color.index()][Piece::Rook.index()];
    while rooks != 0 {
        let sq      = rooks.trailing_zeros() as u8;
        rooks      &= rooks - 1;
        let attacks = rook_attacks(sq, occupied);
        mobility   += (attacks & !own_pieces).count_ones() as i32 * ROOK_MOBILITY_BONUS;
        if include_center {
            center += (attacks & CENTER_SQUARES).count_ones() as i32 * CENTER_ATTACK_BONUS;
        }
        if king_attack {
            let (a, u) = king_pressure(attacks, enemy_zone, KING_ATTACK_WEIGHT_ROOK);
            king_attackers += a; king_units += u;
        }
    }

    // --- Queens ---
    // The queen is already powerful: small marginal bonus per additional square.
    let mut queens = board.pieces[color.index()][Piece::Queen.index()];
    while queens != 0 {
        let sq      = queens.trailing_zeros() as u8;
        queens     &= queens - 1;
        let attacks = queen_attacks(sq, occupied);
        mobility   += (attacks & !own_pieces).count_ones() as i32 * QUEEN_MOBILITY_BONUS;
        if include_center {
            center += (attacks & CENTER_SQUARES).count_ones() as i32 * CENTER_ATTACK_BONUS;
        }
        if king_attack {
            let (a, u) = king_pressure(attacks, enemy_zone, KING_ATTACK_WEIGHT_QUEEN);
            king_attackers += a; king_units += u;
        }
    }

    PieceActivity { mobility, center, king_units, king_attackers }
}

/// Calculates the mobility and center (pieces) differentials from the
/// point of view of the active player. Positive score = advantage for the active player.
///
/// Replaces the old separate calls to mobility_eval() and to the "pieces"
/// part of center_eval() — see eval/mod.rs for the assembly with
/// center::center_pawn_eval() (pawns, not affected by this merge).
pub fn mobility_and_center_eval(
    board: &Board,
    is_endgame: bool,
    king_attack: bool,
) -> (i32, i32, i32) {
    // include_center and king-attack are both inactive in the endgame (the
    // center control and king safety are no longer relevant there).
    let include_center = !is_endgame;
    let do_king_attack = king_attack && !is_endgame;

    let white = mobility_and_center_score(board, Color::White, include_center, do_king_attack);
    let black = mobility_and_center_score(board, Color::Black, include_center, do_king_attack);

    let mobility_diff = white.mobility - black.mobility;
    let center_diff   = white.center   - black.center;

    // king-attack danger: White's pressure targets the BLACK king (White bonus),
    // and vice versa. Differential in White's perspective.
    let white_danger_to_black = king_attack_danger(white.king_units, white.king_attackers);
    let black_danger_to_white = king_attack_danger(black.king_units, black.king_attackers);
    let king_attack_diff = white_danger_to_black - black_danger_to_white;

    if board.side_to_move == Color::White {
        (mobility_diff, center_diff, king_attack_diff)
    } else {
        (-mobility_diff, -center_diff, -king_attack_diff)
    }
}
