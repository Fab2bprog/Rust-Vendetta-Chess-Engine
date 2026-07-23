// =============================================================================
// Vendetta Chess Engine — src/game/history.rs
//
// Role: Tracking position history for repetition detection.
//        The threefold repetition rule in chess states that a game is drawn
//        if the same position repeats 3 times (not necessarily consecutively).
//
// Contents:
//   - PositionHistory: storage of Zobrist hashes of positions played
//   - count_repetitions() : counts how many times the current position
//     has already been encountered in the game
//
// Note: We use the Zobrist hash as the position identifier.
//        Two identical positions have the same hash (with a tiny probability
//        of collision that is acceptable in practice).
// =============================================================================

/// History of positions in a game.
pub struct PositionHistory {
    /// List of Zobrist hashes of all positions played.
    hashes: Vec<u64>,
}

impl PositionHistory {
    /// Creates an empty history.
    pub fn new() -> PositionHistory {
        PositionHistory {
            hashes: Vec::with_capacity(256),
        }
    }

    /// Adds the current position to the history.
    pub fn push(&mut self, hash: u64) {
        self.hashes.push(hash);
    }

    /// Removes the last position from the history (during an unmake_move).
    pub fn pop(&mut self) {
        self.hashes.pop();
    }

    /// Counts the number of times the given hash appears in the history.
    /// Used to detect position repetition.
    pub fn count_occurrences(&self, hash: u64) -> u32 {
        self.hashes.iter().filter(|&&h| h == hash).count() as u32
    }

    /// Returns true if the position (identified by its hash) has already
    /// repeated at least 2 times (so this occurrence would be the 3rd → draw).
    pub fn is_threefold_repetition(&self, hash: u64) -> bool {
        self.count_occurrences(hash) >= 2
    }

    /// Clears the history (start of a new game).
    pub fn clear(&mut self) {
        self.hashes.clear();
    }

    /// Returns the number of positions in the history.
    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    /// Returns true if the history is empty (no position recorded).
    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }
}

impl Default for PositionHistory {
    fn default() -> Self {
        PositionHistory::new()
    }
}
