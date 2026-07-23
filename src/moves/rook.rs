// =============================================================================
// Vendetta Chess Motor — src/moves/rook.rs
//
// Role: Generates all pseudo-legal rook moves for a given color.
//        Uses the rook_attacks() function which computes the attacked squares
//        while taking blocking pieces into account (classic loop-based approach).
//
// Content:
//   - Quiet moves (move to an empty square)
//   - Captures (move to a square occupied by the enemy)
//
// The rook moves in a straight line (horizontal/vertical) across as many squares
// as possible, stopping at the first piece encountered.
// =============================================================================

use crate::utils::types::{Color, Piece, Move};
use crate::board::state::Board;
use crate::board::bitboard::{pop_lsb, rook_attacks};

/// Generates all pseudo-moves of the rooks of the color `color`.
/// The moves are added to the `moves` vector.
pub fn generate_rook_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let mut rooks  = board.pieces[color.index()][Piece::Rook.index()];
    let own_pieces = board.occupancy[color.index()];
    let occupied   = board.all_pieces;

    while rooks != 0 {
        let from    = pop_lsb(&mut rooks);
        let attacks = rook_attacks(from, occupied) & !own_pieces;

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
