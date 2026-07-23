// =============================================================================
// Vendetta Chess Engine — src/game/rules.rs
//
// Role: Verification of end-of-game rules that are not handled
//        directly by move generation.
//
// Contents:
//   - 50-move rule (draw if 100 half-moves without a capture or pawn move)
//   - Position repetition (draw if 3 repetitions)
//   - Insufficient material (draw if checkmate is impossible)
//   - GameResult: final result of a game
//
// Note: The 50-move rule uses the board's halfmove_clock.
//        Repetition uses the position history (PositionHistory).
// =============================================================================

use crate::board::state::Board;
use crate::eval::is_insufficient_material;
use super::history::PositionHistory;

/// Possible result of a game.
#[derive(Debug, Clone, PartialEq)]
pub enum GameResult {
    /// The game continues.
    Ongoing,
    /// Draw by the 50-move rule.
    DrawFiftyMoves,
    /// Draw by position repetition (3 times).
    DrawRepetition,
    /// Draw by insufficient material.
    DrawInsufficientMaterial,
    /// Stalemate.
    DrawStalemate,
    /// Checkmate: the given color has lost.
    Checkmate,
}

/// Checks whether the game is a draw by the 50-move rule.
/// The rule states: after 50 full moves (100 half-moves) without a pawn move
/// or capture, the game is a draw.
pub fn is_fifty_move_draw(board: &Board) -> bool {
    board.halfmove_clock >= 100
}

/// Checks whether the game is a draw by position repetition.
/// Draw if the current position has already occurred 2 times (3rd occurrence).
pub fn is_repetition_draw(board: &Board, history: &PositionHistory) -> bool {
    history.is_threefold_repetition(board.hash)
}

/// Checks all draw conditions (excluding stalemate and checkmate, which require
/// move generation, handled in moves/mod.rs).
pub fn check_draw(board: &Board, history: &PositionHistory) -> Option<GameResult> {
    if is_fifty_move_draw(board) {
        return Some(GameResult::DrawFiftyMoves);
    }

    if is_repetition_draw(board, history) {
        return Some(GameResult::DrawRepetition);
    }

    if is_insufficient_material(board) {
        return Some(GameResult::DrawInsufficientMaterial);
    }

    None
}
