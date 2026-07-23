// =============================================================================
// Vendetta Chess Motor — src/moves/king.rs
//
// Role: Generates all pseudo-legal king moves for a given color.
//        Handles normal moves AND castling (kingside and queenside).
//
// Contents:
//   - Normal moves (1 square in all directions)
//   - Kingside castling (king's side)
//   - Queenside castling (queen's side)
//
// Castling rules:
//   - The king must not be in check before castling
//   - The squares crossed by the king must not be attacked
//   - The squares between the king and the rook must be empty
//   - The castling right must be available (not lost)
//
// IMPORTANT: We do NOT check here whether the king is in check after castling.
//   This filtering is done in generate_legal_moves() in mod.rs.
//   However, we do check that the king does NOT CROSS a square under check.
// =============================================================================

use crate::utils::types::{Color, Move};
use crate::board::state::Board;
use crate::board::bitboard::{pop_lsb, king_attacks, get_bit};

/// Generates all pseudo-moves of the king for the `color` color.
/// The moves are added to the `moves` vector.
pub fn generate_king_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let king_sq    = board.king_square(color);
    let own_pieces = board.occupancy[color.index()];

    // --- Normal moves ---
    let attacks = king_attacks(king_sq) & !own_pieces;

    let mut bb = attacks;
    while bb != 0 {
        let to = pop_lsb(&mut bb);
        if board.all_pieces & (1u64 << to) != 0 {
            moves.push(Move::capture(king_sq, to));
        } else {
            moves.push(Move::quiet(king_sq, to));
        }
    }

    // --- Castling ---
    // We generate castling moves here, but the final check (king in check,
    // squares crossed attacked) is done in generate_legal_moves().
    generate_castling_moves(board, color, king_sq, moves);
}

/// Generates the castling moves available for the `color` color.
/// Checks that the intermediate squares are empty and that the castling rights
/// are available. The check of attacked squares is in mod.rs.
fn generate_castling_moves(board: &Board, color: Color, king_sq: u8, moves: &mut crate::moves::MoveList) {
    match color {
        Color::White => {
            // White kingside castling: e1 (4) → g1 (6), rook on h1 (7)
            // Squares f1 (5) and g1 (6) must be empty
            if board.castling.can_castle_kingside(Color::White)
                && !get_bit(board.all_pieces, 5)
                && !get_bit(board.all_pieces, 6)
            {
                moves.push(Move::castle_kingside(king_sq, 6));
            }

            // White queenside castling: e1 (4) → c1 (2), rook on a1 (0)
            // Squares d1 (3), c1 (2) and b1 (1) must be empty
            if board.castling.can_castle_queenside(Color::White)
                && !get_bit(board.all_pieces, 3)
                && !get_bit(board.all_pieces, 2)
                && !get_bit(board.all_pieces, 1)
            {
                moves.push(Move::castle_queenside(king_sq, 2));
            }
        }

        Color::Black => {
            // Black kingside castling: e8 (60) → g8 (62), rook on h8 (63)
            // Squares f8 (61) and g8 (62) must be empty
            if board.castling.can_castle_kingside(Color::Black)
                && !get_bit(board.all_pieces, 61)
                && !get_bit(board.all_pieces, 62)
            {
                moves.push(Move::castle_kingside(king_sq, 62));
            }

            // Black queenside castling: e8 (60) → c8 (58), rook on a8 (56)
            // Squares d8 (59), c8 (58) and b8 (57) must be empty
            if board.castling.can_castle_queenside(Color::Black)
                && !get_bit(board.all_pieces, 59)
                && !get_bit(board.all_pieces, 58)
                && !get_bit(board.all_pieces, 57)
            {
                moves.push(Move::castle_queenside(king_sq, 58));
            }
        }
    }
}
