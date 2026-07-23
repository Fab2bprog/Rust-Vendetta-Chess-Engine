// =============================================================================
// Vendetta Chess Engine — src/moves/knight.rs
//
// Role: Generates all pseudo-legal knight moves for a color.
//        Uses the precomputed attack table (knight_attacks) for
//        fast and simple generation.
//
// Content:
//   - Quiet moves (move to an empty square)
//   - Captures (move to a square occupied by the enemy)
//
// Note: The knight is the only piece that can jump over other
//        pieces. Its generation is therefore particularly simple.
// =============================================================================

use crate::utils::types::{Color, Piece, Move};
use crate::board::state::Board;
use crate::board::bitboard::{pop_lsb, knight_attacks};

/// Generates all pseudo-moves of the knights of color `color`.
/// The moves are added to the `moves` vector.
pub fn generate_knight_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let mut knights = board.pieces[color.index()][Piece::Knight.index()];
    let own_pieces  = board.occupancy[color.index()];

    // For each knight, generate its attacks from the precomputed table.
    while knights != 0 {
        let from    = pop_lsb(&mut knights);
        // Squares attacked by the knight, excluding our own pieces.
        let attacks = knight_attacks(from) & !own_pieces;

        let mut bb = attacks;
        while bb != 0 {
            let to = pop_lsb(&mut bb);
            // If an enemy piece is on the target square, it's a capture.
            if board.all_pieces & (1u64 << to) != 0 {
                moves.push(Move::capture(from, to));
            } else {
                moves.push(Move::quiet(from, to));
            }
        }
    }
}
