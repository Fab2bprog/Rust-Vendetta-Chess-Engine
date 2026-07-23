// =============================================================================
// Vendetta Chess Motor — src/eval/phase.rs
//
// Role: Detection of the game phase (opening, middlegame, endgame).
//        The phase influences several aspects of the evaluation:
//        - The king must be safe in the middlegame but active in the endgame
//        - Piece-square tables change depending on the phase
//        - King safety is less important in the endgame
//
// Contents:
//   - Calculation of the phase score based on remaining material
//   - Smooth interpolation between middlegame and endgame (tapered eval)
//
// Method:
//   A "phase weight" is assigned to each piece type.
//   When the board is full, we are in the middlegame.
//   When the heavy pieces disappear, we enter the endgame.
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;

/// Weight of each piece type for the phase calculation.
/// The weights are calibrated so that 24 = full middlegame.
const PHASE_WEIGHT: [i32; 6] = [
    0,  // Pawn (does not count for the phase)
    1,  // Knight
    1,  // Bishop
    2,  // Rook
    4,  // Queen
    0,  // King (does not count)
];

/// Maximum phase score (full game in the middlegame).
/// 2 knights + 2 bishops + 4 rooks + 2 queens per side:
/// (2*1 + 2*1 + 4*2 + 2*4) * 2 sides = (2+2+8+8)*2 = 40
const MAX_PHASE: i32 = 40;

/// Representation of the game phase.
#[derive(Clone, Copy, Debug)]
pub struct GamePhase {
    /// Phase score: 0 = pure endgame, MAX_PHASE = pure middlegame.
    pub phase_score: i32,
}

impl GamePhase {
    /// Returns true if we are in the endgame (little material).
    pub fn is_endgame(self) -> bool {
        self.phase_score < MAX_PHASE / 2
    }

    /// Returns a factor between 0.0 (endgame) and 1.0 (middlegame).
    /// Used for tapered interpolation.
    pub fn middlegame_factor(self) -> f32 {
        (self.phase_score as f32 / MAX_PHASE as f32).clamp(0.0, 1.0)
    }

    /// Smooth interpolation between middlegame score and endgame score.
    /// Allows a gradual transition rather than an abrupt jump.
    pub fn taper(self, middlegame_score: i32, endgame_score: i32) -> i32 {
        let mg = self.middlegame_factor();
        let eg = 1.0 - mg;
        (middlegame_score as f32 * mg + endgame_score as f32 * eg) as i32
    }
}

/// Calculates the game phase of the current position.
///
/// Optimization — lighter calculation, identical result:
///   Before: 8 popcounts on bitboards (count_ones() on 64 bits) ×
///           2 colors × 4 piece types.
///   After: 8 simple reads of board.piece_count, an array [u8; 6]
///           already maintained in real time by place_piece()/remove_piece()
///           (also used for is_insufficient_material()).
///   Same value guaranteed: piece_count is synchronized with the bitboards
///   on every move, it's just a direct read instead of a recalculation.
pub fn compute_phase(board: &Board) -> GamePhase {
    let mut phase_score = 0i32;

    for color in [Color::White, Color::Black] {
        for piece in [Piece::Knight, Piece::Bishop, Piece::Rook, Piece::Queen] {
            let count = board.piece_count[color.index()][piece.index()] as i32;
            phase_score += count * PHASE_WEIGHT[piece.index()];
        }
    }

    // Clamp between 0 and MAX_PHASE
    phase_score = phase_score.clamp(0, MAX_PHASE);

    GamePhase { phase_score }
}
