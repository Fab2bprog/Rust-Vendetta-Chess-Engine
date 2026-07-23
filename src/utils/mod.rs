// =============================================================================
// Vendetta Chess Engine — src/utils/mod.rs
//
// Role: Utilities module. Entry point for all common types and
//        functions shared throughout the entire project.
//
// Content:
//   - Re-exports the types module (Color, Piece, Move, MoveFlags, constants...)
// =============================================================================

pub mod types;

// Re-export of the most commonly used types to simplify imports.
pub use types::{
    Color, Piece, Move, MoveFlags,
    SCORE_INF, SCORE_MATE, SCORE_DRAW,
    NUM_PIECES, NUM_COLORS, NUM_SQUARES,
    file_of, rank_of, make_square, square_from_str, square_to_str,
};
