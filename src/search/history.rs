// =============================================================================
// Vendetta Chess Engine — src/search/history.rs
//
// Role: Management of the history heuristic (History Heuristic).
//        Each move that caused a beta cutoff has its score increased
//        in a table [piece][destination_square]. This score is used to
//        sort quiet moves (the better a move has been in the past,
//        the earlier it is tested).
//
// Contents:
//   - HistoryTable: 2D table [piece 0-5][square 0-63]
//   - Score update on a beta cutoff
//   - Score reduction for moves that did not cause a cutoff
//     (aging — to prevent old scores from dominating)
//
// Note: The history heuristic is complementary to killer moves.
//   Killers: "this specific move at this level has been good"
//   History: "this type of move has generally often been good"
// =============================================================================

use crate::utils::types::{Move, Piece};
use crate::board::state::Board;

/// History table: history[piece_type][destination_square].
/// Positive values indicate that this move has often caused cutoffs.
pub struct HistoryTable {
    table: [[i32; 64]; 6],
}

impl HistoryTable {
    /// Creates a new empty history table.
    pub fn new() -> HistoryTable {
        HistoryTable { table: [[0; 64]; 6] }
    }

    /// Increases the score of the move that caused a beta cutoff.
    /// The bonus is proportional to the depth (depth^2).
    pub fn update_good(&mut self, board: &Board, mv: Move, depth: i32) {
        if mv.flags.is_capture() { return; } // Only quiet moves are updated

        if let Some((piece, _)) = board.piece_at(mv.from) {
            let bonus = depth * depth;
            self.table[piece.index()][mv.to as usize] += bonus;
            // Limit to avoid overflow
            if self.table[piece.index()][mv.to as usize] > 10_000 {
                self.table[piece.index()][mv.to as usize] = 10_000;
            }
        }
    }

    /// Slightly reduces the score of moves that did not cause a cutoff.
    /// Prevents old good scores from dominating indefinitely.
    pub fn update_bad(&mut self, board: &Board, mv: Move, depth: i32) {
        if mv.flags.is_capture() { return; }

        if let Some((piece, _)) = board.piece_at(mv.from) {
            let penalty = depth;
            self.table[piece.index()][mv.to as usize] -= penalty;
            if self.table[piece.index()][mv.to as usize] < -10_000 {
                self.table[piece.index()][mv.to as usize] = -10_000;
            }
        }
    }

    /// Returns the history score of a move (for ordering).
    pub fn get(&self, piece: Piece, to: u8) -> i32 {
        self.table[piece.index()][to as usize]
    }

    /// Resets the entire table to zero (between two games).
    pub fn clear(&mut self) {
        self.table = [[0; 64]; 6];
    }

    /// Divides all scores by 2 (aging between iterations).
    pub fn age(&mut self) {
        for row in &mut self.table {
            for val in row.iter_mut() {
                *val /= 2;
            }
        }
    }
}

impl Default for HistoryTable {
    fn default() -> Self {
        HistoryTable::new()
    }
}
