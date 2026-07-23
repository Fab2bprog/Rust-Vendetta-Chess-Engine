// =============================================================================
// Vendetta Chess Motor — src/eval/endgame.rs
//
// Role: Endgame-specific evaluations.
//        These six criteria give the engine a deep understanding
//        of won, drawn, or lost positions in the endgame.
//
// Contents:
//   1. Passed pawn distance bonus
//      A passed pawn 2 ranks from promotion is worth much more than one 6 ranks away.
//      This bonus adds to the base bonus from pawns.rs and is amplified in the endgame.
//
//   2. King proximity to passed pawns
//      In the endgame, the king must escort its passed pawns (push them)
//      and intercept the opponent's (block them).
//      Criterion: Chebyshev distance of the friendly king vs. the enemy king relative to the pawn.
//
//   3. Mop-up evaluation
//      When one side has a decisive material advantage (≥ a rook), the winning king
//      must push the enemy king toward a corner AND move closer to it.
//      Without this criterion, the engine can wander and waste the advantage.
//
//   4. Rook on the 7th rank
//      A rook on the 7th (2nd for Black) cuts off the enemy king,
//      threatens unadvanced pawns and dominates the endgame.
//
//   5. Rook behind a passed pawn (Tarrasch rule)
//      "The rook should always be placed behind a passed pawn, whether it be
//      friend or foe." — Siegbert Tarrasch.
//      Bonus for a friendly rook behind a friendly passed pawn.
//
//   6. Opposite-colored bishops
//      When each side has exactly one bishop on opposite-colored squares,
//      the position is strongly drawish. The material advantage is reduced by 50%
//      to reflect this strategic reality.
//
// Geometry:
//   - Chebyshev distance: minimum number of king moves between two squares.
//   - Rank of a square sq: sq / 8  (0 = rank 1, 7 = rank 8)
//   - File of a square sq: sq % 8  (0 = file a, 7 = file h)
//   - Square color: (rank + file) % 2  (0 = dark, 1 = light)
//
// Internal score convention:
//   - Positive = White advantage, negative = Black advantage.
//   - The perspective flip (active player) is done in endgame_eval().
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::{Bitboard, file_mask, rank_mask};
use super::material::material_score;

// =============================================================================
// Geometric utilities
// =============================================================================

/// Chebyshev distance between two squares (minimum number of king moves).
/// Examples: a1→h8 = 7, e4→d5 = 1, e4→d6 = 2.
#[inline]
fn chebyshev(sq1: u8, sq2: u8) -> i32 {
    let r1 = (sq1 / 8) as i32;
    let c1 = (sq1 % 8) as i32;
    let r2 = (sq2 / 8) as i32;
    let c2 = (sq2 % 8) as i32;
    (r1 - r2).abs().max((c1 - c2).abs())
}

/// Computes the passed pawn bitboard for a color.
/// A passed pawn has no enemy pawn on its file or on adjacent files,
/// in the ranks separating it from promotion.
/// This detection is identical to the one in pawns.rs but self-contained here
/// to avoid modifying existing files and remain dependency-free.
fn passed_pawns_bb(board: &Board, color: Color) -> Bitboard {
    let pawns       = board.pieces[color.index()][Piece::Pawn.index()];
    let enemy_pawns = board.pieces[color.opposite().index()][Piece::Pawn.index()];
    let mut result  = 0u64;

    let mut bb = pawns;
    while bb != 0 {
        let sq   = bb.trailing_zeros() as u8;
        bb      &= bb - 1;

        let file = sq % 8;
        let rank = sq / 8;

        let col  = file_mask(file);
        let left = if file > 0 { file_mask(file - 1) } else { 0 };
        let rght = if file < 7 { file_mask(file + 1) } else { 0 };
        let zone = col | left | rght;

        let front: Bitboard = match color {
            Color::White => {
                let mut mask = 0u64;
                for r in (rank + 1)..8 { mask |= rank_mask(r) & zone; }
                mask
            }
            Color::Black => {
                let mut mask = 0u64;
                for r in 0..rank { mask |= rank_mask(r) & zone; }
                mask
            }
        };

        if enemy_pawns & front == 0 {
            result |= 1u64 << sq;
        }
    }

    result
}

// =============================================================================
// 1. Passed pawn distance bonus
// =============================================================================

/// Additional bonus for passed pawns close to promotion.
/// The base bonus (pawns.rs) already grows with advancement, but not enough
/// to reflect the real threat of a pawn 1 or 2 ranks from queening.
///
/// These bonuses add to the base bonus and are amplified in the endgame
/// (fewer pieces to stop the pawn = a more concrete threat).
///
/// Values (extra on top of the base bonus from pawns.rs):
///   - Rank 7 / rank 2 (1 square from queening): +60 mg / +120 eg
///   - Rank 6 / rank 3 (2 squares from queening): +25 mg / +50  eg
///   - Rank 5 / rank 4 (3 squares from queening): +0  mg / +15  eg
fn passed_pawn_advancement_bonus(passed: Bitboard, color: Color, is_endgame: bool) -> i32 {
    let mut score = 0i32;

    let mut bb = passed;
    while bb != 0 {
        let sq   = bb.trailing_zeros() as u8;
        bb      &= bb - 1;
        let rank = sq / 8;

        // Distance in ranks to the promotion square (1 = one square, 6 = far)
        let dist = match color {
            Color::White => 7 - rank,
            Color::Black => rank,
        } as i32;

        // Passed pawn bonus based on distance to promotion and phase.
        // Table (distance, endgame?) → bonus: the more advanced the pawn and the more
        // it is in the endgame, the higher the bonus. (3, false) falls to 0.
        let extra = match (dist, is_endgame) {
            (1, true)  => 120,
            (1, false) => 60,
            (2, true)  => 50,
            (2, false) => 25,
            (3, true)  => 15,
            _          => 0,
        };

        score += extra;
    }

    score
}

// =============================================================================
// 2. King proximity to passed pawns
// =============================================================================

/// In the endgame, the king must escort its passed pawns toward promotion
/// and intercept enemy passed pawns before they promote.
///
/// For each friendly passed pawn, we measure:
///   - dist_ally    : Chebyshev distance between the friendly king and the pawn
///   - dist_enemy   : Chebyshev distance between the enemy king and the pawn
///   - advantage    : dist_enemy - dist_ally
///
/// A positive advantage (friendly king closer) is worth 5 cp per unit.
fn king_passed_pawn_proximity(board: &Board, passed: Bitboard, color: Color) -> i32 {
    let own_king   = board.king_square(color);
    let enemy_king = board.king_square(color.opposite());
    let mut score  = 0i32;

    let mut bb = passed;
    while bb != 0 {
        let sq  = bb.trailing_zeros() as u8;
        bb     &= bb - 1;

        let own_dist   = chebyshev(own_king, sq);
        let enemy_dist = chebyshev(enemy_king, sq);

        // Positive = friendly king closer = good escort
        score += (enemy_dist - own_dist) * 5;
    }

    score
}

// =============================================================================
// 3. Mop-up evaluation
// =============================================================================

/// "Mop-up" evaluation when one side has a decisive material advantage.
///
/// Without this criterion, the engine may wander needlessly when it has largely
/// won, wasting time or causing a stalemate through carelessness.
///
/// Two components:
///   a) Confinement: push the enemy king toward the edge or the corner.
///      dist_to_edge : 0 = king at the edge, 3 = king at the exact center.
///      Score = (3 - dist_to_edge) × 15  → max 45 cp.
///   b) Proximity: bring the winning king closer to the losing king.
///      Score = (7 - chebyshev) × 5      → max 30 cp.
///   Total max: 75 cp (enough to guide the king without disturbing the material balance).
///
/// Activation condition: material advantage ≥ 500 cp (about a rook).
/// Below that, the position is too uncertain to impose this behavior.
fn mop_up_eval(board: &Board) -> i32 {
    let white_mat = material_score(board, Color::White);
    let black_mat = material_score(board, Color::Black);
    let advantage = white_mat - black_mat;

    const MOPUP_THRESHOLD: i32 = 500;
    if advantage.abs() < MOPUP_THRESHOLD {
        return 0;
    }

    let (winning, losing) = if advantage > 0 {
        (Color::White, Color::Black)
    } else {
        (Color::Black, Color::White)
    };

    let winning_king = board.king_square(winning);
    let losing_king  = board.king_square(losing);

    // a) Confinement of the losing king toward the edge/corner
    let lk_rank      = (losing_king / 8) as i32;
    let lk_file      = (losing_king % 8) as i32;
    let dist_to_edge = lk_rank.min(7 - lk_rank).min(lk_file).min(7 - lk_file);
    let confinement  = (3 - dist_to_edge) * 15;   // 0–45 cp

    // b) Proximity of the winning king to the losing king
    let king_dist  = chebyshev(winning_king, losing_king);
    let proximity  = (7 - king_dist) * 5;          // 0–30 cp

    let score = confinement + proximity;

    if winning == Color::White { score } else { -score }
}

// =============================================================================
// 4. Rook on the 7th rank
// =============================================================================

/// Bonus for a rook on the 7th rank (2nd for Black).
///
/// A rook on the 7th:
///   - Cuts off the enemy king on its starting rank
///   - Threatens all unadvanced pawns on that rank
///   - Is active in both the middlegame AND the endgame
///
/// Value: 25 cp per rook (significant but conservative).
fn rook_on_seventh(board: &Board, color: Color) -> i32 {
    let rooks   = board.pieces[color.index()][Piece::Rook.index()];
    let seventh = match color {
        Color::White => rank_mask(6),  // Rank 7 (index 6, 0-based)
        Color::Black => rank_mask(1),  // Rank 2 (index 1, 0-based)
    };
    (rooks & seventh).count_ones() as i32 * 25
}

// =============================================================================
// 5. Rook behind a passed pawn (Tarrasch rule)
// =============================================================================

/// "The rook should always be placed behind the passed pawn, whether it be
/// friend or foe." — Siegbert Tarrasch.
///
/// For a friendly rook behind a friendly passed pawn:
///   - White: rook on a rank lower than the white passed pawn (pawn advances upward)
///   - Black : rook on a rank higher than the black passed pawn (pawn advances downward)
///
/// A rook behind its own passed pawn can push it indefinitely
/// without blocking itself, this is the ideal configuration.
///
/// Value: 20 cp per (rook, passed pawn) pair that follows the rule.
fn rook_behind_passed_pawn(board: &Board, passed: Bitboard, color: Color) -> i32 {
    let rooks  = board.pieces[color.index()][Piece::Rook.index()];
    let mut score = 0i32;

    let mut pawn_bb = passed;
    while pawn_bb != 0 {
        let pawn_sq   = pawn_bb.trailing_zeros() as u8;
        pawn_bb      &= pawn_bb - 1;
        let pawn_file = pawn_sq % 8;
        let pawn_rank = pawn_sq / 8;

        // Friendly rooks on the same file as the passed pawn
        let file_rooks = rooks & file_mask(pawn_file);

        let mut rook_bb = file_rooks;
        while rook_bb != 0 {
            let rook_sq   = rook_bb.trailing_zeros() as u8;
            rook_bb      &= rook_bb - 1;
            let rook_rank = rook_sq / 8;

            // "Behind" according to the pawn's direction of travel
            let behind = match color {
                Color::White => rook_rank < pawn_rank,  // White pawn advances upward → rook below
                Color::Black => rook_rank > pawn_rank,  // Black pawn advances downward → rook above
            };

            if behind {
                score += 20;
            }
        }
    }

    score
}

// =============================================================================
// 6. Opposite-colored bishops
// =============================================================================

/// Recognizes endgames with opposite-colored bishops and reduces the advantage.
///
/// When both sides have exactly one bishop each on squares of different
/// colors, the winning side's bishop can never influence the squares
/// controlled by the enemy bishop. This configuration is strongly drawish.
///
/// Detection:
///   - Each side has exactly 1 bishop (more = not an issue)
///   - Both bishops are on squares of different colors
///   - Color of a square sq: (sq/8 + sq%8) % 2  (0=dark, 1=light)
///
/// Action: reduce the material advantage by 50% for this node.
/// Half of the material differential is subtracted (brings the eval closer to 0).
///
/// Note: returns a score from the absolute point of view (positive = White).
fn opposite_colored_bishops_eval(board: &Board, is_endgame: bool) -> i32 {
    // This criterion is specific to endgames
    if !is_endgame { return 0; }

    let white_bbs = board.pieces[Color::White.index()][Piece::Bishop.index()];
    let black_bbs = board.pieces[Color::Black.index()][Piece::Bishop.index()];

    // Only if each side has exactly one bishop
    if white_bbs.count_ones() != 1 || black_bbs.count_ones() != 1 {
        return 0;
    }

    let ws = white_bbs.trailing_zeros() as u8;
    let bs = black_bbs.trailing_zeros() as u8;

    // Square color: (rank + file) % 2
    let wc = (ws / 8 + ws % 8) % 2;
    let bc = (bs / 8 + bs % 8) % 2;

    // If same square color → not opposite-colored bishops
    if wc == bc { return 0; }

    // Opposite-colored bishops confirmed: reduce the advantage by 50%
    let white_mat = material_score(board, Color::White);
    let black_mat = material_score(board, Color::Black);
    let advantage = white_mat - black_mat;

    // We return the opposite of half the advantage
    // Example: White is winning by 200 cp → we return -100
    // The final evaluation will be 200 (material) - 100 (here) = 100 net → drawish
    -advantage / 2
}

// =============================================================================
// Entry function — combination of the 6 criteria
// =============================================================================

/// Computes the complete endgame evaluation from the active player's point of view.
///
/// Criteria active in all phases:
///   - Rook on the 7th rank
///   - Mop-up (conditional on material advantage)
///   - Rook behind passed pawn (Tarrasch)
///
/// Criteria active only in the endgame:
///   - Passed pawn distance bonus (amplified in the endgame)
///   - King proximity to passed pawns
///   - Opposite-colored bishops
pub fn endgame_eval(board: &Board, is_endgame: bool) -> i32 {
    // --- Single passed pawn computation, shared by all criteria below ---
    //
    // Optimization — lighter computation, identical result:
    //   Before: passed_pawns_bb(board, color) was called separately in
    //   passed_pawn_advancement_bonus(), king_passed_pawn_proximity() and
    //   rook_behind_passed_pawn() — up to 3 times per color (6 times in total
    //   in the endgame) to compute EXACTLY the same bitboard each time.
    //   After: computed once per color here, then passed as a
    //   parameter to the three functions. Same definition, same result —
    //   only the number of calls decreases.
    let white_passed = passed_pawns_bb(board, Color::White);
    let black_passed = passed_pawns_bb(board, Color::Black);

    // --- All phases ---

    // Rook on the 7th (relevant from the middlegame onward)
    let rook_7th   = rook_on_seventh(board, Color::White)
                   - rook_on_seventh(board, Color::Black);

    // Mop-up (activates only if material advantage ≥ 500 cp)
    let mop_up     = mop_up_eval(board);

    // Tarrasch: rook behind passed pawn (profitable at any phase)
    let tarrasch   = rook_behind_passed_pawn(board, white_passed, Color::White)
                   - rook_behind_passed_pawn(board, black_passed, Color::Black);

    // --- Additional bonuses for advanced passed pawns ---
    // Present in all phases but stronger in the endgame (is_endgame parameter)
    let pawn_adv   = passed_pawn_advancement_bonus(white_passed, Color::White, is_endgame)
                   - passed_pawn_advancement_bonus(black_passed, Color::Black, is_endgame);

    // --- Endgame only ---
    let endgame_only = if is_endgame {
        // King proximity to passed pawns (escort or blockade)
        let king_prox  = king_passed_pawn_proximity(board, white_passed, Color::White)
                       - king_passed_pawn_proximity(board, black_passed, Color::Black);

        // Opposite-colored bishops (advantage reduction)
        // Already in absolute perspective (White positive) → no flip here
        let ocb        = opposite_colored_bishops_eval(board, is_endgame);

        king_prox + ocb
    } else {
        0
    };

    // Total score from the absolute point of view (White positive)
    let total = rook_7th + mop_up + tarrasch + pawn_adv + endgame_only;

    // Perspective flip: side to move
    if board.side_to_move == Color::White { total } else { -total }
}
