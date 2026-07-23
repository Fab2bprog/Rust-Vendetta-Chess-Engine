// =============================================================================
// Vendetta Chess Engine — src/search/countermove.rs
//
// Role: Management of the "Countermove" heuristic (refutation move).
//        Idea: if a certain type of opponent move (e.g.: "the knight goes to
//        e5") has already been effectively refuted by a specific move elsewhere in
//        the tree, that same move is probably still a good response here
//        — even with a different piece position, same refutation "motif".
//
// Difference from killer moves and the history heuristic:
//   - Killers  : "THIS precise move was good at THIS depth level"
//   - History  : "THIS TYPE of move (piece + destination) has generally
//                 often been good, across all depths combined"
//   - Countermove : "IN RESPONSE to THIS precise opponent move (piece + destination
//                 square), THIS move has already been a good refutation"
//
// Indexing: countermoves[opponent_piece][opponent_destination_square] → move.
//   The piece and destination square of the LAST MOVE PLAYED (the one that led to the
//   current node) are derived via board.piece_at(prev_move.to) — already
//   applied to the board by the time this node is reached, so it can be read
//   directly without an extra parameter to propagate besides the Move itself.
//
// Only one countermove per key (not 2 like killers): the literature
// (historical Stockfish, Crafty) shows that one is largely sufficient, the gain
// from a second slot being marginal compared to the added complexity.
// =============================================================================

use crate::utils::types::{Move, Piece};

/// Countermove table: one refutation move per (piece, destination square) of the
/// last opponent move played.
pub struct CountermoveTable {
    /// table[piece_index 0-5][destination_square 0-63] → refutation move.
    table: [[Move; 64]; 6],
}

impl CountermoveTable {
    /// Creates a new countermove table (empty).
    pub fn new() -> CountermoveTable {
        CountermoveTable {
            table: [[Move::NULL; 64]; 6],
        }
    }

    /// Records `mv` as the refutation of the last move played, identified by
    /// (`prev_piece`, `prev_to`).
    ///
    /// Only stores quiet moves — consistent with killers/history:
    /// captures are already well ordered by SEE, no need to duplicate them
    /// here (and a capture countermove would often be illegal or irrelevant
    /// in a different position).
    pub fn store(&mut self, prev_piece: Piece, prev_to: u8, mv: Move) {
        if mv.flags.is_capture() { return; }
        self.table[prev_piece.index()][prev_to as usize] = mv;
    }

    /// Returns the countermove recorded for (`prev_piece`, `prev_to`).
    /// `Move::NULL` if none is recorded.
    pub fn get(&self, prev_piece: Piece, prev_to: u8) -> Move {
        self.table[prev_piece.index()][prev_to as usize]
    }

    /// Resets the entire table to zero (between two games, or between two "go"
    /// commands like killers — a countermove relevant in one search has
    /// no reason to be relevant in the following position after the opponent
    /// has actually played a move).
    pub fn clear(&mut self) {
        self.table = [[Move::NULL; 64]; 6];
    }
}

impl Default for CountermoveTable {
    fn default() -> Self {
        CountermoveTable::new()
    }
}
