// =============================================================================
// Vendetta Chess Engine — src/moves/bishop.rs
//
// Role: Generates all pseudo-legal bishop moves for a given color.
//        Uses the bishop_attacks() function which computes the attacked squares
//        taking into account blocking pieces (classic loop-based approach).
//
// Contents:
//   - Quiet moves (move to an empty square)
//   - Captures (move to a square occupied by the enemy)
//
// The bishop moves diagonally as many squares as possible,
// stopping at the first piece encountered (which it can capture if it is an enemy piece).
// =============================================================================

use crate::utils::types::{Color, Piece, Move};
use crate::board::state::Board;
use crate::board::bitboard::{pop_lsb, bishop_attacks};

/// Generates all pseudo-moves for the bishops of color `color`.
/// The moves are added to the `moves` vector.
pub fn generate_bishop_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let mut bishops = board.pieces[color.index()][Piece::Bishop.index()];
    let own_pieces  = board.occupancy[color.index()];
    let occupied    = board.all_pieces;

    while bishops != 0 {
        let from    = pop_lsb(&mut bishops);
        // Squares attacked by the bishop from this square, excluding our own pieces.
        let attacks = bishop_attacks(from, occupied) & !own_pieces;

        let mut bb = attacks;
        while bb != 0 {
            let to = pop_lsb(&mut bb);
            if board.all_pieces & (1u64 << to) != 0 {
                moves.push(Move::capture(from, to));
            } else {
                moves.push(Move::quiet(from, to));
            }
        }
    }
}
