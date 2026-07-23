// =============================================================================
// Vendetta Chess Engine — src/search/see.rs
//
// Role: Static Exchange Evaluation (SEE).
//        Evaluates the net result of a sequence of captures on a square,
//        each side always playing its least valuable piece (LVA =
//        Least Valuable Attacker). Each side can choose to stop the
//        sequence if it would lose material by continuing.
//
// Usage in Vendetta Chess Engine:
//   1. Ordering of captures in move_score()
//      → Winning captures (SEE ≥ 0) before quiet moves.
//      → Losing captures (SEE < 0) after quiet moves.
//   2. Pruning in quiescence
//      → Captures with SEE < 0 are ignored (too costly).
//
// Algorithm (iterative, gains[32] on the stack):
//
//   Forward pass:
//     gains[d] = value of the piece that the current side could capture
//                at depth d (the opponent's piece on `to` at that moment).
//     Each LVA is removed from the occupancy → automatically reveals X-rays.
//
//   Backward pass (minimax + stand-pat):
//     result = 0
//     for d in (0..depth).rev():
//       result = max(0, gains[d] - result)
//
//   Final result = captured_value - result
//
// Handling X-rays:
//   By removing the piece that just captured from `occupied`, new
//   attacks from sliding pieces placed behind it are naturally
//   revealed when bishop_attacks / rook_attacks / queen_attacks are recalculated.
//
// Accepted limitations (standard in all engines):
//   - Pins are NOT checked (too costly, negligible effect on quality)
//   - En passant → returns 0 (pawn exchanges pawn, result assumed neutral)
//   - Promotion → the pawn is treated as a queen (most common promotion)
// =============================================================================

use crate::utils::types::{Color, Piece, Move, MoveFlags};
use crate::board::state::Board;
use crate::board::bitboard::{
    bishop_attacks, rook_attacks, queen_attacks, knight_attacks, king_attacks,
};
use crate::eval::material::piece_value;

// =============================================================================
// Internal helpers
// =============================================================================

/// Returns the bitboard of `side`'s pawns present in `side_pawns` that
/// attack the square `to`.
///
/// A white pawn on square `sq` attacks `sq+7` and `sq+9`.
/// A black pawn on square `sq` attacks `sq-7` and `sq-9`.
/// We therefore look for which pawns are in position to attack `to`.
fn pawn_attackers_to(to: u8, side: Color, side_pawns: u64) -> u64 {
    let file = to % 8;
    let mut mask = 0u64;

    match side {
        Color::White => {
            // White pawn on (to - 9) attacks `to` (bottom-left diagonal)
            // Conditions: to >= 9 and file(to) > 0 (avoids column overflow)
            if file > 0 {
                if let Some(sq) = to.checked_sub(9) {
                    mask |= 1u64 << sq;
                }
            }
            // White pawn on (to - 7) attacks `to` (bottom-right diagonal)
            // Conditions: to >= 7 and file(to) < 7
            if file < 7 {
                if let Some(sq) = to.checked_sub(7) {
                    mask |= 1u64 << sq;
                }
            }
        }
        Color::Black => {
            // Use u16 to avoid overflow (to is u8, max 63)
            let to_u16 = to as u16;
            // Black pawn on (to + 7) attacks `to` (top-left diagonal)
            // Conditions: to + 7 < 64 and file(to) > 0
            if file > 0 && to_u16 + 7 < 64 {
                mask |= 1u64 << (to + 7);
            }
            // Black pawn on (to + 9) attacks `to` (top-right diagonal)
            // Conditions: to + 9 < 64 and file(to) < 7
            if file < 7 && to_u16 + 9 < 64 {
                mask |= 1u64 << (to + 9);
            }
        }
    }

    mask & side_pawns
}

/// Finds the least valuable piece (LVA) of side `side` that attacks the square
/// `to` in the current occupancy `occupied`.
///
/// Returns `Some((lva_square, lva_value))` or `None` if there is no attacker.
///
/// The test order (from least valuable to most valuable) guarantees the LVA:
///   Pawn → Knight → Bishop → Rook → Queen → King
///
/// Note: `occupied` is passed explicitly (and may differ from the initial
/// board) to account for X-rays after each capture.
fn find_lva(board: &Board, to: u8, side: Color, occupied: u64) -> Option<(u8, i32)> {
    let idx = side.index();

    // --- Pawns ---
    let pawns = board.pieces[idx][Piece::Pawn.index()] & occupied;
    let pawn_atk = pawn_attackers_to(to, side, pawns);
    if pawn_atk != 0 {
        let sq = pawn_atk.trailing_zeros() as u8;
        // If the pawn captures on the last rank, it promotes → queen value
        let is_promo = match side {
            Color::White => to / 8 == 7,
            Color::Black => to / 8 == 0,
        };
        let val = if is_promo {
            piece_value(Piece::Queen)
        } else {
            piece_value(Piece::Pawn)
        };
        return Some((sq, val));
    }

    // --- Knights ---
    let knights = board.pieces[idx][Piece::Knight.index()] & occupied;
    let knight_atk = knight_attacks(to) & knights;
    if knight_atk != 0 {
        let sq = knight_atk.trailing_zeros() as u8;
        return Some((sq, piece_value(Piece::Knight)));
    }

    // --- Bishops ---
    // Uses `occupied` for the actual attacks (X-rays).
    let bishops = board.pieces[idx][Piece::Bishop.index()] & occupied;
    let bishop_atk = bishop_attacks(to, occupied) & bishops;
    if bishop_atk != 0 {
        let sq = bishop_atk.trailing_zeros() as u8;
        return Some((sq, piece_value(Piece::Bishop)));
    }

    // --- Rooks ---
    let rooks = board.pieces[idx][Piece::Rook.index()] & occupied;
    let rook_atk = rook_attacks(to, occupied) & rooks;
    if rook_atk != 0 {
        let sq = rook_atk.trailing_zeros() as u8;
        return Some((sq, piece_value(Piece::Rook)));
    }

    // --- Queens ---
    let queens = board.pieces[idx][Piece::Queen.index()] & occupied;
    let queen_atk = queen_attacks(to, occupied) & queens;
    if queen_atk != 0 {
        let sq = queen_atk.trailing_zeros() as u8;
        return Some((sq, piece_value(Piece::Queen)));
    }

    // --- King (last: very high value to avoid sacrificing it) ---
    let kings = board.pieces[idx][Piece::King.index()] & occupied;
    let king_atk = king_attacks(to) & kings;
    if king_atk != 0 {
        let sq = king_atk.trailing_zeros() as u8;
        return Some((sq, piece_value(Piece::King)));
    }

    None
}

// =============================================================================
// Public entry point — iterative SEE
// =============================================================================

/// Statically evaluates the exchange of captures triggered by the move `mv`.
///
/// Returns the net gain (in centipawns) for the side making `mv`:
///   - Positive → winning capture (net material gain)
///   - Zero     → neutral capture (exact exchange)
///   - Negative → losing capture (net material loss)
///
/// Iterative algorithm with a gains array on the stack (`gains[32]`).
/// Replaces the old recursive version (max depth ~8) — zero overhead
/// from function calls, zero recursive stack frames.
///
/// Principle:
///   1. Forward pass: each successive recapture is simulated by finding the LVA
///      of each side, in turn. `gains[d]` stores the value of
///      the piece the current side could capture at depth d.
///   2. Backward pass: propagate from the maximum depth down to 0 by applying
///      `result = max(0, gains[d] - result)` — each side can refuse to
///      capture if the exchange is unfavorable to it (stand-pat).
///   3. Final result = `captured_value - result`.
///
/// Handling X-rays:
///   Each LVA is removed from the occupancy before the next iteration.
///   Sliding pieces placed behind it are thus naturally revealed
///   on the next call to bishop_attacks / rook_attacks / queen_attacks.
///
/// Special cases:
///   - En passant          → returns 0 (pawn ×× pawn, assumed neutral)
///   - Non-capture         → returns 0 (SEE not applicable)
///   - Promoting pawn      → treated as a queen (standard assumption)
pub fn see(board: &Board, mv: Move) -> i32 {
    // En passant: the square `to` is empty (the captured pawn is on a different square).
    // The exchange is assumed to be fair (pawn for pawn).
    if mv.flags == MoveFlags::EnPassant {
        return 0;
    }

    // SEE applies only to captures (Capture or PromotionCapture)
    if !mv.flags.is_capture() {
        return 0;
    }

    let to   = mv.to;
    let from = mv.from;

    // Value of the captured piece (on square `to`)
    let captured_value = match board.piece_at(to) {
        Some((p, _)) => piece_value(p),
        None         => return 0,
    };

    // Value of the capturing piece (on square `from`)
    // If it's a promotion-capture, the pawn promotes → it is treated as a queen
    let attacker_value = match board.piece_at(from) {
        Some((Piece::Pawn, _)) if mv.flags == MoveFlags::PromotionCapture => {
            piece_value(Piece::Queen)
        }
        Some((p, _)) => piece_value(p),
        None         => return 0,
    };

    // Remove the capturing piece from the initial occupancy.
    // (It has moved to `to`; X-rays behind it will be revealed.)
    let mut occ  = (board.occupancy[0] | board.occupancy[1]) & !(1u64 << from);
    let mut side = board.side_to_move.opposite(); // First to recapture = opponent

    // --- Forward pass: simulation of the exchange sequence ---
    //
    // gains[d] = value of the piece that the current side COULD capture at
    //            depth d (i.e. the value of the opponent's piece that has just
    //            captured, and that this side can now take).
    //
    // Invariant: `next_target` is the value of the piece occupying `to` after
    //             the previous capture, which is the new target.
    let mut gains       = [0i32; 32];
    let mut depth       = 0usize;
    let mut next_target = attacker_value; // The first piece to recapture = our attacker

    while depth < 32 {
        match find_lva(board, to, side, occ) {
            None => break, // No more attackers for this side → exchange finished
            Some((lva_sq, lva_val)) => {
                // This side can capture `next_target` with its LVA.
                gains[depth] = next_target;

                // The LVA leaves its square → removed from the occupancy (reveals X-rays).
                occ         &= !(1u64 << lva_sq);
                // The next target will be this LVA (the opponent will be able to take it).
                next_target  = lva_val;
                side         = side.opposite();
                depth       += 1;
            }
        }
    }

    // --- Backward pass: minimax with stand-pat ---
    //
    // We go back up from the maximum depth to 0.
    // `result` represents the net gain the current side can hope for if it captures.
    // Each side chooses: capture (`gains[d] - result`) or pass (0).
    //
    // max(0, gains[d] - result) = stand-pat: we don't capture if it's a losing move.
    let mut result = 0i32;
    for d in (0..depth).rev() {
        result = (gains[d] - result).max(0);
    }

    // Final result for the player making `mv`:
    //   we take `captured_value`, the opponent can recapture (cost = `result`).
    //
    // Note: we do NOT do max(0, ...) here.
    // A negative value means a losing capture → useful for
    // move ordering and pruning in quiescence.
    captured_value - result
}
