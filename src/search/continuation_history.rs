// =============================================================================
// Vendetta Chess Motor — src/search/continuation_history.rs
//
// Role: Management of the "Continuation History" (continuation history,
//        sometimes called "2-move history").
//
// Difference with the Countermove Heuristic (countermove.rs):
//   - Countermove  : "IN RESPONSE TO A SPECIFIC opponent move, A GIVEN move is THE
//                     BEST observed" — a single slot per context, overwritten
//                     each time a new refutation is found.
//   - Continuation : "IN RESPONSE TO A SPECIFIC opponent move, A GIVEN move has been
//                     ON AVERAGE good or bad" — a CUMULATIVE score per
//                     context, exactly like the classic History Heuristic
//                     but with an additional context (the
//                     previous opponent move).
//
//   The two are complementary: countermove captures "THE" best
//   known response, continuation history captures a finer statistical
//   trend. Used together, as in this engine.
//
// Indexing: table[opponent_piece][opponent_square][piece][destination_square].
//   6 × 64 × 6 × 64 = 147,456 i32 entries ≈ 576 KiB.
//
// Storage choice — flat Vec<i32> rather than [[[[i32; 64]; 6]; 64]; 6]:
//   The other tables in this module (killers, history, countermove) are
//   tiny (a few KiB) and stored as fixed arrays without
//   risk. This one is ~1500× larger. A nested array of this
//   size built BY VALUE (e.g. in a new() function that returns it)
//   could pass through a large temporary allocation on the stack before
//   being moved — a real risk with many Lazy SMP threads that each create
//   their own instance. A Vec<i32> is allocated directly on the
//   heap as soon as it's created (vec![0; N]): no risk of stack overflow,
//   regardless of size.
// =============================================================================

use crate::board::state::Board;
use crate::utils::types::{Move, Piece};

const PIECES:  usize = 6;
const SQUARES: usize = 64;
const TABLE_SIZE: usize = PIECES * SQUARES * PIECES * SQUARES;

/// Continuation history table: a cumulative score per
/// (opponent piece, opponent square, piece, destination square).
pub struct ContinuationHistoryTable {
    table: Vec<i32>,
}

impl ContinuationHistoryTable {
    /// Creates a new continuation history table (empty, heap-allocated).
    pub fn new() -> ContinuationHistoryTable {
        ContinuationHistoryTable {
            table: vec![0i32; TABLE_SIZE],
        }
    }

    #[inline]
    fn index(prev_piece: Piece, prev_to: u8, piece: Piece, to: u8) -> usize {
        ((prev_piece.index() * SQUARES + prev_to as usize) * PIECES + piece.index())
            * SQUARES
            + to as usize
    }

    /// Increases the score of the move that caused a beta cutoff, in the
    /// context of the last opponent move (prev_piece, prev_to).
    /// The bonus is proportional to depth² — same convention as
    /// HistoryTable::update_good().
    ///
    /// `board` must be in the state BEFORE the move `mv` (that is, after
    /// an unmake_move() — exactly like HistoryTable::update_good(), in order to
    /// be able to read the piece still present on mv.from).
    pub fn update_good(
        &mut self,
        prev_piece: Piece,
        prev_to:    u8,
        board:      &Board,
        mv:         Move,
        depth:      i32,
    ) {
        if mv.flags.is_capture() { return; }

        if let Some((piece, _)) = board.piece_at(mv.from) {
            let idx   = Self::index(prev_piece, prev_to, piece, mv.to);
            let bonus = depth * depth;
            self.table[idx] = (self.table[idx] + bonus).min(10_000);
        }
    }

    /// Slightly reduces the score of a move that did not cause a cutoff,
    /// in the same context. Same convention as HistoryTable::update_bad().
    pub fn update_bad(
        &mut self,
        prev_piece: Piece,
        prev_to:    u8,
        board:      &Board,
        mv:         Move,
        depth:      i32,
    ) {
        if mv.flags.is_capture() { return; }

        if let Some((piece, _)) = board.piece_at(mv.from) {
            let idx = Self::index(prev_piece, prev_to, piece, mv.to);
            self.table[idx] = (self.table[idx] - depth).max(-10_000);
        }
    }

    /// Returns the continuation history score for (piece, destination
    /// square) in the context (prev_piece, prev_to).
    #[inline]
    pub fn get(&self, prev_piece: Piece, prev_to: u8, piece: Piece, to: u8) -> i32 {
        self.table[Self::index(prev_piece, prev_to, piece, to)]
    }

    /// Resets the entire table to zero (between two games).
    pub fn clear(&mut self) {
        self.table.iter_mut().for_each(|v| *v = 0);
    }

    /// Divides all scores by 2 (aging between searches) —
    /// same policy as HistoryTable::age(), not a full clear() like
    /// killers/countermove: a cumulative score retains value in being
    /// carried over, attenuated, from one search to the next.
    pub fn age(&mut self) {
        self.table.iter_mut().for_each(|v| *v /= 2);
    }
}

impl Default for ContinuationHistoryTable {
    fn default() -> Self {
        ContinuationHistoryTable::new()
    }
}
