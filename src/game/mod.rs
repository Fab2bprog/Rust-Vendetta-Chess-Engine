// =============================================================================
// Vendetta Chess Engine — src/game/mod.rs
//
// Role: Manages the current game. Coordinates the board state, the history
//        of positions, and checks the game-ending conditions.
//
// Contents:
//   - Game: main structure of the game
//   - Play/undo moves with history tracking
//   - Detection of game-ending conditions
// =============================================================================

pub mod history;
pub mod rules;

use crate::board::state::Board;
use crate::utils::types::Move;
use history::PositionHistory;
use rules::{GameResult, check_draw};
use crate::moves::{is_checkmate, is_stalemate};

/// Representation of a chess game in progress.
pub struct Game {
    /// The current state of the board.
    pub board: Board,
    /// History of position hashes for repetition detection.
    pub position_history: PositionHistory,
}

impl Game {
    /// Creates a new game at the initial position.
    pub fn new() -> Game {
        let board = Board::start_position();
        let hash  = board.hash;
        let mut history = PositionHistory::new();
        history.push(hash);

        Game {
            board,
            position_history: history,
        }
    }

    /// Creates a game from a FEN.
    pub fn from_fen(fen: &str) -> Result<Game, String> {
        let board = Board::from_fen(fen)?;
        let hash  = board.hash;
        let mut history = PositionHistory::new();
        history.push(hash);

        Ok(Game {
            board,
            position_history: history,
        })
    }

    /// Plays a move and updates the history.
    pub fn make_move(&mut self, mv: Move) {
        self.board.make_move(mv);
        self.position_history.push(self.board.hash);
    }

    /// Undoes the last move played.
    pub fn unmake_move(&mut self, mv: Move) {
        self.position_history.pop();
        self.board.unmake_move(mv);
    }

    /// Checks the current result of the game.
    pub fn result(&mut self) -> GameResult {
        // First check the simple draw conditions (without move generation)
        if let Some(result) = check_draw(&self.board, &self.position_history) {
            return result;
        }

        // Check checkmate and stalemate (requires move generation)
        if is_checkmate(&mut self.board) {
            return GameResult::Checkmate;
        }

        if is_stalemate(&mut self.board) {
            return GameResult::DrawStalemate;
        }

        GameResult::Ongoing
    }

    /// Resets for a new game.
    pub fn reset(&mut self) {
        self.board = Board::start_position();
        self.position_history.clear();
        self.position_history.push(self.board.hash);
    }
}

impl Default for Game {
    fn default() -> Self {
        Game::new()
    }
}
