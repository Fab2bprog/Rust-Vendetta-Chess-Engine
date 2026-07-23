// =============================================================================
// Vendetta Chess Motor — src/moves/queen.rs
//
// Role: Generates all pseudo-legal queen moves for a given color.
//        The queen combines the movements of the bishop and the rook: it can
//        move in a straight line AND diagonally.
//
// Contents:
//   - Quiet moves
//   - Captures
//
// Implementation: we reuse queen_attacks() which combines rook_attacks()
// and bishop_attacks(). Simple, correct, and maintainable.
// =============================================================================

use crate::utils::types::{Color, Piece, Move};
use crate::board::state::Board;
use crate::board::bitboard::{pop_lsb, queen_attacks};

/// Generates all pseudo-legal queen moves for color `color`.
/// The moves are added to the `moves` vector.
pub fn generate_queen_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let mut queens = board.pieces[color.index()][Piece::Queen.index()];
    let own_pieces = board.occupancy[color.index()];
    let occupied   = board.all_pieces;

    while queens != 0 {
        let from    = pop_lsb(&mut queens);
        // The queen attacks like the rook + the bishop
        let attacks = queen_attacks(from, occupied) & !own_pieces;

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
