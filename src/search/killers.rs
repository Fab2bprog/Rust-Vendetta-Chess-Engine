// =============================================================================
// Vendetta Chess Engine — src/search/killers.rs
//
// Role: Management of "killer moves".
//        A killer move is a quiet move (no capture) that caused a
//        beta cutoff at the same depth in the search tree.
//        It is tested with priority because it can be effective in other branches.
//
// Contents:
//   - KillerMoves: storage of 2 killer moves per depth
//   - Update on a beta cutoff
//   - Check whether a move is a killer move
//
// Why 2 killers per depth?
//   A single killer is not always applicable (even if good, it may be
//   illegal in the current position). Two killers increase the chances
//   of having a valid one.
// =============================================================================

use crate::utils::types::Move;
use super::alphabeta::MAX_PLY;

// Maximum supported search depth.
//
// BUG FIXED (post-session audit): this file previously defined its
// OWN MAX_PLY = 128 constant, distinct from the one in alphabeta.rs (192).
// Beyond depth 128, killer moves were therefore silently
// disabled (store()/is_killer()/get() all returned a no-op), even though
// the search can legitimately reach up to 192 plies with
// extensions. Not a crash, but an unnecessary loss of efficiency and a source
// of confusion (two constants with the same name, different values, no link
// between them). Imported from alphabeta.rs: a single source of truth.

/// Killer moves manager.
/// For each depth level (ply), up to 2 killer moves are stored.
pub struct KillerMoves {
    /// killers[ply][0] and killers[ply][1]: the two killers for this ply.
    killers: [[Move; 2]; MAX_PLY],
}

impl KillerMoves {
    /// Creates a new killers manager (empty).
    pub fn new() -> KillerMoves {
        KillerMoves {
            killers: [[Move::NULL; 2]; MAX_PLY],
        }
    }

    /// Registers a killer move for depth `ply`.
    /// If the move is already the first killer, does nothing.
    /// Otherwise, shifts the first into second and puts the new one first.
    pub fn store(&mut self, mv: Move, ply: usize) {
        if ply >= MAX_PLY { return; }

        // Do not store captures as killers
        if mv.flags.is_capture() { return; }

        // Avoid duplicates
        if self.killers[ply][0] == mv { return; }

        // Shift and store
        self.killers[ply][1] = self.killers[ply][0];
        self.killers[ply][0] = mv;
    }

    /// Returns true if the move is a killer move for depth `ply`.
    pub fn is_killer(&self, mv: Move, ply: usize) -> bool {
        if ply >= MAX_PLY { return false; }
        self.killers[ply][0] == mv || self.killers[ply][1] == mv
    }

    /// Returns the killers for a given depth.
    pub fn get(&self, ply: usize) -> [Move; 2] {
        if ply >= MAX_PLY { return [Move::NULL; 2]; }
        self.killers[ply]
    }

    /// Resets all killers to zero (between two searches).
    pub fn clear(&mut self) {
        self.killers = [[Move::NULL; 2]; MAX_PLY];
    }
}

impl Default for KillerMoves {
    fn default() -> Self {
        KillerMoves::new()
    }
}
