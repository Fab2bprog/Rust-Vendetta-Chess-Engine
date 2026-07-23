// =============================================================================
// Vendetta Chess Engine — src/eval/mod.rs
//
// Role: Main evaluation function. Combines all evaluation
//        criteria into a single score representing the quality of the position
//        from the point of view of the player to move.
//
// Criteria taken into account (in order of importance):
//   1. Material (piece values + bishop pair bonus)
//   2. Position (piece-square tables)
//   3. Mobility (squares accessible to each piece)
//   4. Center control (presence and attacks on d4/d5/e4/e5)
//   5. Pawn structure (doubled, isolated, passed)
//   6. King safety (pawn shield + danger from enemy attack)
//   7. Endgame specifics (mop-up, 7th rank rook, king near passed pawns)
//   8. Threats / hanging pieces (attacked by something cheaper, or undefended)
//   9. Tempo (fixed bonus for the player to move)
//
// Convention:
//   - Positive score → favorable for the active player
//   - Negative score → unfavorable for the active player
//   - The unit is the centipawn (100 = value of a pawn)
// =============================================================================

pub mod material;
pub mod position;
pub mod tables;
pub mod pawns;
pub mod king_safety;
pub mod phase;
pub mod mobility;
pub mod center;
pub mod endgame;
pub mod threats;

use crate::board::state::Board;
use crate::utils::types::Color;
use material::bishop_pair_eval;
use pawns::pawn_eval;
use king_safety::king_safety_eval;
use phase::compute_phase;
use mobility::mobility_and_center_eval;
use center::center_pawn_eval;
use endgame::endgame_eval;
use threats::threats_eval;

/// Evaluates the current position from the point of view of the player to move.
/// Returns a score in centipawns.
///
/// Positive score = good for the active player.
/// Negative score = bad for the active player.
///
/// Incremental optimization:
///   The material and PST components are no longer recomputed here.
///   They are maintained in real time in board.eval_mg / board.eval_eg
///   by place_piece() and remove_piece(), which eliminates O(32) iterations
///   over the pieces at each leaf node.
pub fn evaluate(board: &Board) -> i32 {
    // Normal play: all terms active, including king safety from attack.
    evaluate_opt(board, true)
}

/// Variant of `evaluate()` with a switch for the "king attack" term (king safety
/// from attack). Used by the SPRT tests of the selfplay binary, which isolate this
/// term: `king_attack = false` ⇒ behavior strictly identical to the eval
/// before this term (pawn shield alone). In normal play, we always pass
/// `true` via `evaluate()`.
pub fn evaluate_opt(board: &Board, king_attack: bool) -> i32 {
    // Compute the game phase (middlegame or endgame).
    let phase      = compute_phase(board);
    let is_endgame = phase.is_endgame();

    // --- 1+3. Material + PST (incremental, O(1)) ---
    // board.eval_mg / board.eval_eg are in White's perspective (White − Black).
    // We choose the appropriate table according to the phase, then orient
    // according to the active player.
    let mat_pst = if is_endgame { board.eval_eg } else { board.eval_mg };
    let mat_pst_relative = if board.side_to_move == Color::White { mat_pst } else { -mat_pst };

    // --- 2. Bishop pair bonus (2 × count_ones, negligible) ---
    let bishop_pair = bishop_pair_eval(board);

    // --- 4+5. Mobility + Center control (pieces), computed in one pass ---
    // Optimization: mobility.rs and center.rs each separately computed
    // the attack bitboards for knights/bishops/rooks/queens. Now a
    // single function does both at once (same raw attack bitboard
    // reused for both bonuses) — identical numerical result, half
    // as many attack lookups (including magic bitboards, the most costly ones).
    // include_center = !is_endgame exactly reproduces the previous behavior
    // (center_eval() never called in the endgame).
    let (mobility, center_pieces, king_attack_danger) =
        mobility_and_center_eval(board, is_endgame, king_attack);

    // --- Center control (pawns) — not affected by the merge above ---
    let center_pawns = if is_endgame { 0 } else { center_pawn_eval(board) };
    let center       = center_pieces + center_pawns;

    // --- 6. Pawn structure ---
    let pawn_structure = pawn_eval(board);

    // --- 7. King safety ---
    let king_safety = king_safety_eval(board, is_endgame);

    // --- 8. Endgame specifics ---
    let endgame     = endgame_eval(board, is_endgame);

    // --- 9. Threats / hanging pieces ---
    // Deliberately NOT conditioned on is_endgame (unlike
    // mobility/center/king_safety): a hanging piece is a weakness
    // at any point in the game, not just in the middlegame.
    let threats = threats_eval(board);

    mat_pst_relative + bishop_pair + mobility + center
        + pawn_structure + king_safety + king_attack_danger + endgame + threats + TEMPO_BONUS
}

/// Tempo bonus: having the move is an advantage in itself (initiative,
/// potential threats, the opponent must respond). evaluate() is already
/// "from the point of view of the player to move" (see convention at the top
/// of this file) — a simple constant added at the end is therefore enough to
/// represent this advantage, with no additional computation.
///
/// Reasonable initial value, consistent with typical values used
/// by other engines (10-30 cp) — to be refined by Texel Tuning if a
/// future version extends the tuner to this parameter (not currently included,
/// like threats.rs — see ARCHITECTURE.md "Future work under consideration").
const TEMPO_BONUS: i32 = 10;

/// Checks whether the position is drawn due to insufficient material to mate.
///
/// Optimization: uses board.piece_count (u8, updated incrementally)
/// instead of calling count_ones() on 10 bitboards at every node.
/// Cost: 10 u8 reads instead of 10 popcnt operations on u64.
pub fn is_insufficient_material(board: &Board) -> bool {
    use crate::utils::types::Piece;

    let pc = &board.piece_count;
    let w  = 0usize; // White index
    let b  = 1usize; // Black index
    let p  = Piece::Pawn.index();
    let n  = Piece::Knight.index();
    let bi = Piece::Bishop.index();
    let r  = Piece::Rook.index();
    let q  = Piece::Queen.index();

    // Immediate bail-out (most common case — ~99% of nodes):
    // pawns, rooks or queens present → sufficient material to mate.
    if pc[w][p] + pc[b][p] + pc[w][r] + pc[b][r] + pc[w][q] + pc[b][q] > 0 {
        return false;
    }

    let wn = pc[w][n]; let wb = pc[w][bi];
    let bn = pc[b][n]; let bb = pc[b][bi];

    // KvK
    if wn + wb + bn + bb == 0 { return true; }

    // K+N vs K  or  K+B vs K
    if wn + wb <= 1 && bn + bb == 0 { return true; }
    if bn + bb <= 1 && wn + wb == 0 { return true; }

    // KN/KB vs KN/KB (max 1 minor piece per side — no forced mate)
    if wn + wb <= 1 && bn + bb <= 1 { return true; }

    // KNN vs K (two knights cannot force mate)
    if wn == 2 && wb == 0 && bn + bb == 0 { return true; }
    if bn == 2 && bb == 0 && wn + wb == 0 { return true; }

    false
}
