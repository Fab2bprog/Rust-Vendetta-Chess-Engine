// =============================================================================
// Vendetta Chess Motor — src/eval/tables.rs
//
// Role: Position tables (PST) and pure lookup functions.
//        This file NEVER imports board::state — it must remain accessible
//        from board::state without creating a circular dependency.
//
// Contents:
//   - 7 PST tables (pawn, knight, bishop, rook, queen, king×2)
//   - mirror_square() — vertical symmetry for Black
//   - piece_square_values() — returns (mg, eg) for a piece on a square
//
// Usage:
//   - board::state   → piece_square_values() for incremental update
//   - eval::position → imports the tables for positional_eval() (debug/test)
// =============================================================================

use crate::utils::types::{Color, Piece};

// =============================================================================
// Position tables — from White's point of view
// Index 0 = a1, index 63 = h8 (rank × 8 + file).
// The tables are read rank by rank from bottom to top (rank 1 → rank 8).
// =============================================================================

/// Positional table for pawns.
pub const PAWN_TABLE: [i32; 64] = [
     0,  0,  0,  0,  0,  0,  0,  0,
    50, 50, 50, 50, 50, 50, 50, 50,
    10, 10, 20, 30, 30, 20, 10, 10,
     5,  5, 10, 25, 25, 10,  5,  5,
     0,  0,  0, 20, 20,  0,  0,  0,
     5, -5,-10,  0,  0,-10, -5,  5,
     5, 10, 10,-20,-20, 10, 10,  5,
     0,  0,  0,  0,  0,  0,  0,  0,
];

/// Positional table for knights.
pub const KNIGHT_TABLE: [i32; 64] = [
    -50,-40,-30,-30,-30,-30,-40,-50,
    -40,-20,  0,  0,  0,  0,-20,-40,
    -30,  0, 10, 15, 15, 10,  0,-30,
    -30,  5, 15, 20, 20, 15,  5,-30,
    -30,  0, 15, 20, 20, 15,  0,-30,
    -30,  5, 10, 15, 15, 10,  5,-30,
    -40,-20,  0,  5,  5,  0,-20,-40,
    -50,-40,-30,-30,-30,-30,-40,-50,
];

/// Positional table for bishops.
pub const BISHOP_TABLE: [i32; 64] = [
    -20,-10,-10,-10,-10,-10,-10,-20,
    -10,  0,  0,  0,  0,  0,  0,-10,
    -10,  0,  5, 10, 10,  5,  0,-10,
    -10,  5,  5, 10, 10,  5,  5,-10,
    -10,  0, 10, 10, 10, 10,  0,-10,
    -10, 10, 10, 10, 10, 10, 10,-10,
    -10,  5,  0,  0,  0,  0,  5,-10,
    -20,-10,-10,-10,-10,-10,-10,-20,
];

/// Positional table for rooks.
pub const ROOK_TABLE: [i32; 64] = [
     0,  0,  0,  0,  0,  0,  0,  0,
     5, 10, 10, 10, 10, 10, 10,  5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
     0,  0,  0,  5,  5,  0,  0,  0,
];

/// Positional table for queens.
pub const QUEEN_TABLE: [i32; 64] = [
    -20,-10,-10, -5, -5,-10,-10,-20,
    -10,  0,  0,  0,  0,  0,  0,-10,
    -10,  0,  5,  5,  5,  5,  0,-10,
     -5,  0,  5,  5,  5,  5,  0, -5,
      0,  0,  5,  5,  5,  5,  0, -5,
    -10,  5,  5,  5,  5,  5,  0,-10,
    -10,  0,  5,  0,  0,  0,  0,-10,
    -20,-10,-10, -5, -5,-10,-10,-20,
];

/// Positional table for the king in the middlegame.
/// The king must remain protected (preferably after castling).
pub const KING_MIDDLEGAME_TABLE: [i32; 64] = [
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -20,-30,-30,-40,-40,-30,-30,-20,
    -10,-20,-20,-20,-20,-20,-20,-10,
     20, 20,  0,  0,  0,  0, 20, 20,
     20, 30, 10,  0,  0, 10, 30, 20,
];

/// Positional table for the king in the endgame.
/// In the endgame, the king should centralize.
pub const KING_ENDGAME_TABLE: [i32; 64] = [
    -50,-40,-30,-20,-20,-30,-40,-50,
    -30,-20,-10,  0,  0,-10,-20,-30,
    -30,-10, 20, 30, 30, 20,-10,-30,
    -30,-10, 30, 40, 40, 30,-10,-30,
    -30,-10, 30, 40, 40, 30,-10,-30,
    -30,-10, 20, 30, 30, 20,-10,-30,
    -30,-30,  0,  0,  0,  0,-30,-30,
    -50,-30,-30,-30,-30,-30,-30,-50,
];

// =============================================================================
// Lookup functions
// =============================================================================

/// Returns the mirror index of a square (for Black).
/// White sees rank 1 at the bottom (index 0–7), Black at the top.
/// Example: a1 (0) ↔ a8 (56).
#[inline]
pub fn mirror_square(sq: u8) -> u8 {
    (7 - sq / 8) * 8 + sq % 8
}

/// Returns the PST contributions (midgame, endgame) of a piece on a square,
/// from the point of view of its color.
///
/// For all pieces except the king, mg == eg (same table).
/// For the king: mg = KING_MIDDLEGAME_TABLE, eg = KING_ENDGAME_TABLE.
///
/// The ±1 sign (White = +1, Black = −1) is applied by the caller
/// (`place_piece` / `remove_piece`), not here — clear separation of responsibilities.
#[inline]
pub fn piece_square_values(piece: Piece, color: Color, sq: u8) -> (i32, i32) {
    // The tables are oriented for White (rank 1 at the bottom).
    // For Black, the mirror square is read.
    let idx = if color == Color::White {
        sq as usize
    } else {
        mirror_square(sq) as usize
    };

    let mg = match piece {
        Piece::Pawn   => PAWN_TABLE[idx],
        Piece::Knight => KNIGHT_TABLE[idx],
        Piece::Bishop => BISHOP_TABLE[idx],
        Piece::Rook   => ROOK_TABLE[idx],
        Piece::Queen  => QUEEN_TABLE[idx],
        Piece::King   => KING_MIDDLEGAME_TABLE[idx],
    };

    // The king is the only piece with two distinct tables.
    let eg = if piece == Piece::King {
        KING_ENDGAME_TABLE[idx]
    } else {
        mg // All other pieces: same MG and EG table.
    };

    (mg, eg)
}
