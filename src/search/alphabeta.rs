// =============================================================================
// Vendetta Chess Engine — src/search/alphabeta.rs
//
// Role: Alpha-beta search algorithm with all its heuristics.
//        This is the heart of the engine's intelligence.
//
// Contents:
//   - alpha_beta(): main search with alpha-beta pruning
//   - quiescence(): quiescence search (captures; correct handling of
//     positions where the side to move is in check — see the function's doc)
//   - order_moves(): move ordering to maximize cutoffs
//   - Principal Variation Search (PVS)
//   - Null move pruning
//   - Late Move Reduction (LMR)
//   - Late Move Pruning (LMP)
//   - Internal Iterative Reduction (IIR)
//   - Mate Distance Pruning
//   - Reverse Futility Pruning (Static Null Move)
//   - Razoring (formerly named "Futility Pruning" in this file —
//     renamed to match standard terminology)
//   - Delta Pruning (futility in quiescence)
//   - Check Extension
//   - Singular Extension (SE)
//   - Killer moves, history heuristic and countermove heuristic integrated
//
// Alpha-beta principle:
//   We maintain a window [alpha, beta].
//   - alpha: best score the active player is guaranteed to obtain
//   - beta : best score the opponent is guaranteed to obtain
//   If a move exceeds beta, the opponent would avoid it → we cut (beta cutoff).
//   If a move improves alpha, we update the best move.
//
// Principal Variation Search (PVS) — detailed principle:
//   Good ordering places the best move first in the vast
//   majority of nodes. We exploit this property:
//     - Move #1 (move_index == 0): searched with a full window [alpha, beta].
//       We need its exact score to establish the true reference.
//     - Subsequent moves: first probed with a NULL window [-alpha-1, -alpha].
//       A null window only asks a boolean question ("does it exceed
//       alpha?"), which generates far more internal beta cutoffs
//       than a full window → a significantly smaller search tree.
//       If the probe exceeds alpha, the move is potentially better than
//       expected: it is re-searched with a full window to get its exact score.
//   Typical gain measured in the literature: 10-20% fewer nodes
//   compared to a "naive" alpha-beta that would search all moves with a full
//   window. Combines naturally with LMR (the null-window probe is
//   also where the depth reduction is applied).
//
// Singular Extension — detailed principle:
//   A TT move is "singular" if, when excluded from the search, no other
//   move can reach a score close to its own. To check this, we launch
//   a search at reduced depth (depth/2) with the TT move excluded and a
//   null window just below its score. If this search fails (fail-low),
//   the move is singular and we explore it one level deeper.
//   Precaution: the SE search must never be recursive (we check
//   excluded_move.is_null() before launching SE).
//
// Philosophy: clarity and correctness before optimization.
//   Each heuristic is clearly separated and documented.
// =============================================================================

use std::sync::OnceLock;

use crate::utils::types::{Color, Piece, Move, MoveFlags, SCORE_INF, SCORE_MATE, SCORE_DRAW};
use crate::board::state::Board;
use crate::moves::{generate_legal_moves_into, generate_legal_captures_into, is_in_check, MoveList, MAX_MOVE_LIST};
use crate::eval::{evaluate_opt, is_insufficient_material};
use crate::eval::material::piece_value;
use super::transposition::{TranspositionTable, TTFlag};
use super::see::see;
use super::EVAL_HISTORY_NONE;
use super::killers::KillerMoves;
use super::history::HistoryTable;
use super::countermove::CountermoveTable;
use super::continuation_history::ContinuationHistoryTable;
use super::SearchInfo;
use super::CorrKeys;
// EVAL_HISTORY_NONE (defined in search/mod.rs) serves as a sentinel "no
// eval recorded at this ply" for the eval_history stack of the "improving" flag
// — see the detailed "improving" block in alpha_beta().

// Theoretical maximum number of LEGAL moves in a chess position.
// (record position: "R6R/3Q4/1Q4Q1/4Q3/2Q4Q/Q4Q2/pp1Q4/kBNN1KB1 w - - 0 1" → 218 moves)
// Used to size the stack arrays indexed by legal move
// (`scores`, `lmp_pruned`) in the move loop.
//
// Not to be confused with `moves::MAX_MOVE_LIST` (= 256): the latter is the
// CAPACITY of a MoveList (buffer of pseudo-moves before legal filtering), deliberately
// larger and rounded to a power of 2 for margin. Here we index moves
// already filtered to LEGAL, whose count is bounded by this exact maximum of 218.
const MAX_MOVES: usize = 218;

// Absolute maximum search depth in plies from the root.
//
// Critical safety bound for the Check Extension.
//
// Problem: the Check Extension adds +1 to the child's depth when a move
// gives check and depth ≤ 4:
//   child_depth = depth - 1 + 1 = depth   (depth DOES NOT DECREASE)
// If every move in the branch gives check, the recursion never terminates.
//
// Fix: the extension condition includes `ply + 1 < MAX_PLY`.
// Beyond this threshold, all extension is disabled → depth decreases normally → termination guaranteed.
//
// Chosen value: 128 (max_depth in infinite mode) + 64 extension levels = 192.
// In practice, perpetual checks are detected well before this (position repetition).
// This bound is an absolute safety net, never reached in real games.
//
// pub(crate) visibility: killers.rs reuses THIS constant (instead of
// defining a second one) to size its table — fixes an inconsistency
// discovered during an audit (killers.rs had its own copy fixed at 128,
// which is less than the 192 plies the search can theoretically reach
// with extensions; beyond 128, killer moves were silently
// disabled). A single source of truth prevents the two values from diverging
// again in the future.
pub(crate) const MAX_PLY: usize = 128 + 64;

// Maximum depth of the quiescence search.
// In practice, SEE filters out losing captures and stand-pat cuts off the recursion,
// but a pathological position with many winning captures chained together could
// overflow the stack without this explicit guard. 64 levels = amply sufficient for any
// conceivable exchange, at no noticeable cost (extremely rare case).
const MAX_QUIESCENCE_PLY: usize = MAX_PLY + 64; // MAX_PLY + 64 quiescence levels

// =============================================================================
// Move ordering
// =============================================================================

/// Ordering score for a move (higher = tried first).
/// Good ordering is crucial to alpha-beta efficiency.
// Deliberately large number of arguments: the ordering combines several
// heuristics (TT, killers, history, countermove, continuation history), each
// in its own structure. Grouping them would obscure the nature of the dependencies.
#[allow(clippy::too_many_arguments)]
fn move_score(
    board:        &Board,
    mv:           Move,
    tt_move:      Move,
    killers:      &KillerMoves,
    history:      &HistoryTable,
    countermoves: &CountermoveTable,
    cont_history: &ContinuationHistoryTable,
    prev_key:     Option<(Piece, u8)>,
    ply:          usize,
) -> i32 {
    // 1. Transposition table move (best known move) → absolute priority
    if mv == tt_move && !tt_move.is_null() {
        return 2_000_000;
    }

    // 2. Captures: evaluated by SEE (Static Exchange Evaluation).
    //    SEE simulates the entire sequence of captures on the target square and returns
    //    the net gain for the capturing side.
    //    - SEE ≥ 0: winning or neutral capture → high priority (1_000_000 + see)
    //    - SEE < 0: losing capture → very low priority (negative)
    //      Explored after quiet moves, avoided in quiescence.
    if mv.flags.is_capture() {
        let see_score = see(board, mv);
        return if see_score >= 0 {
            1_000_000 + see_score   // Winning capture: before quiet moves
        } else {
            see_score               // Losing capture: after quiet moves
        };
    }

    // 3. Queen promotions (without capture)
    if mv.flags == MoveFlags::Promotion && mv.promotion == 4 {
        return 900_000;
    }

    // 4. Killer moves (quiet moves that recently caused cutoffs).
    //    Killer 1 (the most recently recorded, see KillerMoves::store) is
    //    slightly preferred over killer 2: with equal recording, the most recent
    //    is statistically the most relevant candidate in this branch.
    //    The gap (10 points) is minimal — just enough to break ties without
    //    ever making them fall below the queen promotions threshold (900_000).
    let km = killers.get(ply);
    if mv == km[0] {
        return 810_000;
    }
    if mv == km[1] {
        return 800_000;
    }

    // 5. Countermove: move that has already refuted the last move played by the
    //    opponent (same piece + same destination square) elsewhere in the tree. Placed
    //    just below killers: a more specific signal than generic
    //    history, but less established than a killer tested AT this exact level.
    if let Some((prev_piece, prev_to)) = prev_key {
        if mv == countermoves.get(prev_piece, prev_to) {
            return 750_000;
        }
    }

    // 6. Quiet moves ordered by the history heuristic, enriched
    //    by continuation history (context of the opponent's last move) —
    //    a simple addition, not a separate tier: continuation history
    //    refines the existing "history" score rather than creating a new
    //    priority category.
    if let Some((piece, _)) = board.piece_at(mv.from) {
        let base = history.get(piece, mv.to);
        let cont = match prev_key {
            Some((prev_piece, prev_to)) => cont_history.get(prev_piece, prev_to, piece, mv.to),
            None => 0,
        };
        return base + cont;
    }

    0
}

/// Hash key of the NON-PAWN pieces (Knight..King) of a color, for
/// indexing a Correction History table. Mixing of the relevant bitboards.
/// An index collision has no consequence on the result: at worst an
/// approximate correction (the eval remains the eval, only a margin shifts).
#[inline]
fn nonpawn_key(board: &Board, color: Color) -> u64 {
    // Distinct odd multipliers (splitmix-style mixing constants).
    const MULT: [u64; 6] = [
        0,                      // Pawn (excluded)
        0xFF51_AFD7_ED55_8CCD,  // Knight
        0xC4CE_B9FE_1A85_EC53,  // Bishop
        0x9E37_79B9_7F4A_7C15,  // Rook
        0x2545_F491_4F6C_DD1D,  // Queen
        0x1656_67B1_9E37_79F9,  // King
    ];
    let c = color.index();
    let mut h = 0u64;
    // Each NON-PAWN bitboard mixed by its multiplier. `.skip(1)` discards
    // pawns (index 0); this covers Knight..King (index 1 to 5).
    for (bb, &mult) in board.pieces[c].iter().zip(MULT.iter()).skip(1) {
        h ^= bb.wrapping_mul(mult);
    }
    h ^ (h >> 31)
}

/// Builds the Correction History keys for a node (computed ONCE, then
/// reused at the read site AND the learning site). `prev_move` is
/// the move that led to this node (for the continuation table).
#[inline]
fn corr_keys(board: &Board, prev_move: Move) -> CorrKeys {
    // Pawn structure key (the two pawn bitboards mixed together).
    let wp = board.pieces[Color::White.index()][Piece::Pawn.index()];
    let bp = board.pieces[Color::Black.index()][Piece::Pawn.index()];
    let mut hp = wp.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    hp ^= bp.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    let pawn = hp ^ (hp >> 29);

    // Continuation index: (piece type, destination square) of the last move.
    // After make_move(prev_move), the piece that moved is ON prev_move.to.
    let cont = if prev_move.is_null() {
        None
    } else if let Some((piece, _)) = board.piece_at(prev_move.to) {
        Some(piece.index() * 64 + prev_move.to as usize)
    } else {
        None
    };

    CorrKeys {
        stm:       board.side_to_move.index(),
        pawn,
        nonpawn_w: nonpawn_key(board, Color::White),
        nonpawn_b: nonpawn_key(board, Color::Black),
        cont,
    }
}

// order_moves() has been replaced by a lazy sort inlined in alpha_beta().
// See the "Lazy selection sort" comment in alpha_beta().

// =============================================================================
// Late Move Reduction — precomputed table (zero floating point at runtime)
// =============================================================================

/// Returns the LMR reduction for (depth, move_index) via a precomputed table.
///
/// Standard logarithmic formula (Stockfish-style), computed ONCE:
///   reduction = max(1, floor(1 + ln(depth) × ln(move_index) / 2))
///
/// The table is a [64][64] array of i32 initialized via OnceLock on first call.
/// Out-of-bounds indices (depth > 63 or move_index > 63) are clamped to 63.
/// Row/column 0 is 0 (ln(0) = -∞ → no reduction at zero depth/index).
///
/// Example values:
///   depth= 3, move_index= 3 → 1   (not very late, low depth)
///   depth= 6, move_index= 6 → 2
///   depth=10, move_index=15 → 4
///   depth=15, move_index=20 → 5
///
/// Gain: eliminates two f64::ln() calls + cast per internal node (millions of nodes/s).
#[inline]
fn lmr_reduction(depth: i32, move_index: usize) -> i32 {
    static LMR_TABLE: OnceLock<[[i32; 64]; 64]> = OnceLock::new();

    let table = LMR_TABLE.get_or_init(|| {
        let mut t = [[0i32; 64]; 64];
        // d=0 or m=0: ln(0) = -∞ → reduction 0 (not applied in practice)
        // Deliberate indexed loop: d AND m serve both as indices AND as
        // values in the ln(d)·ln(m) formula — an iterator would be less clear.
        #[allow(clippy::needless_range_loop)]
        for d in 1usize..64 {
            for m in 1usize..64 {
                let r = 1.0_f64 + (d as f64).ln() * (m as f64).ln() / 2.0;
                t[d][m] = (r as i32).max(1);
            }
        }
        t
    });

    let d = (depth as usize).min(63);
    let m = move_index.min(63);
    table[d][m]
}

// =============================================================================
// Late Move Pruning — move threshold per depth
// =============================================================================

/// Maximum depth beyond which Late Move Pruning no longer ever
/// applies: beyond that, the number of legal moves rarely exceeds
/// the threshold below anyway, and the risk (pruning a genuinely good move)
/// outweighs the marginal gain.
const LMP_MAX_DEPTH: i32 = 8;

/// Number of moves (across all categories, but in practice almost
/// always exceeded by poorly-ranked quiet moves, since captures and
/// killers are ordered much earlier) beyond which a remaining quiet
/// move is pruned without search at this depth.
///
/// Quadratic growth: at depth 1, a very tight threshold (little margin
/// for error acceptable); at depth LMP_MAX_DEPTH, the threshold exceeds the
/// number of legal moves in the vast majority of real positions
/// (the theoretical record is 218, but the real average is around
/// 30-40) — LMP then becomes a natural no-op, with no special case to code.
///
/// "Improving" adjustment (RE-ENABLED — the original bug on `eval_history`
/// has been fixed, see the "improving" block in alpha_beta()): the quadratic
/// coefficient is 2 if the position is improving (we grant MORE moves
/// before pruning — an improving position deserves a broader look),
/// 1 otherwise (we prune earlier, having no reason to believe a late move
/// saves a position that isn't improving). Same logic as the
/// `(2 - improving)` divisor of modern engines.
#[inline]
fn lmp_threshold(depth: i32, improving: bool) -> usize {
    let coeff = if improving { 2 } else { 1 };
    (4 + coeff * depth * depth) as usize
}

// =============================================================================
// Delta Pruning (futility pruning in quiescence)
// =============================================================================

/// Safety margin added to the estimate of material gain for delta
/// pruning. Covers the gap between the raw material value of a capture and
/// the full evaluation of the resulting position (mobility, pawn
/// structure, king safety, etc., not counted here). Standard value used
/// by most engines: 150 to 200 cp. We keep 200 to stay
/// cautious (better to explore a useless move than miss a useful one).
const DELTA_MARGIN: i32 = 200;

/// Estimates the maximum material gain of a capture, for delta pruning.
///
/// Value of the captured piece + any promotion gain (value of the
/// new piece minus that of the pawn). This is an UPPER bound on the
/// actual gain: SEE will account for opponent recaptures, but here we only
/// want to know whether the BEST possible case can reach alpha — an
/// upper bound is enough, and it is much cheaper to compute than
/// the full SEE (no simulation of the exchange chain).
///
/// Special cases:
///   - En passant: the captured piece is not on `to` but on the
///     adjacent square; we know it's always an enemy pawn.
///   - Promotion (with or without capture): we add the net gain of the
///     promotion (Queen − Pawn by default, cf. promotion_piece()).
#[inline]
fn capture_gain_estimate(board: &Board, mv: Move) -> i32 {
    let captured_value = if mv.flags == MoveFlags::EnPassant {
        piece_value(Piece::Pawn)
    } else {
        match board.piece_at(mv.to) {
            Some((p, _)) => piece_value(p),
            None => 0, // Should never happen for a legal capture.
        }
    };

    let promotion_gain = match mv.promotion_piece() {
        Some(p) => piece_value(p) - piece_value(Piece::Pawn),
        None => 0,
    };

    captured_value + promotion_gain
}

// =============================================================================
// Quiescence search
// =============================================================================

/// Quiescence search: continues the search on "noisy" moves
/// (captures), plus full evasions when the side to move is in check
/// (see "Check handling" below). Avoids the horizon effect (stopping
/// on an unstable position) — for example, not evaluating a position where the
/// queen has just been captured but could be recaptured on the next move.
///
/// Check handling: if the side to move is IN CHECK on entry, the
/// function does NOT do a stand-pat (illegal — passing is not allowed) and
/// searches ALL legal evasions (not just captures),
/// also detecting mate. See the `in_check` block at the start of the function.
/// Generating quiet moves that GIVE check is, however, not done
/// (too costly — see the note at the end of the function).
///
/// The depth from the root (`ply`) is used for the MAX_QUIESCENCE_PLY bound and
/// for mate scores. The recursion is naturally bounded (stand-pat + SEE filter
/// on captures, limited evasions when in check); no depth counter
/// specific to quiescence is necessary.
pub fn quiescence(
    board:     &mut Board,
    mut alpha: i32,
    beta:      i32,
    ply:       usize,
    info:      &mut SearchInfo,
) -> i32 {
    if info.should_stop() {
        return evaluate_opt(board, !info.toggles.disable_king_attack);
    }

    info.nodes += 1;

    // Update the maximum selective depth reached (UCI seldepth).
    if ply as i32 > info.seldepth {
        info.seldepth = ply as i32;
    }

    // Safety depth bound.
    // Placed BEFORE everything else to ALSO bound the "in check" branch
    // below: a sequence of checks and evasions does not progress via
    // captures and is not detected as drawn by quiescence (no
    // repetition detection here) — this guard guarantees termination and
    // protects the stack in the rare positions with near-perpetual checks.
    if ply >= MAX_QUIESCENCE_PLY {
        return evaluate_opt(board, !info.toggles.disable_king_attack);
    }

    // --- Special case: the side to move is IN CHECK ---
    //
    // RE-ENABLED — a professional and safe form of "checks in quiescence".
    // The old attempt generated ALL legal moves at EVERY leaf
    // to look for quiet check-giving moves among them: prohibitive (the
    // hottest path of the engine), hence its deactivation. Here we implement
    // the genuinely important and MUCH LESS costly part — correctly
    // handling the position where the side to move is ITSELF in check.
    //
    // When in check, "stand-pat" is illegal: you cannot "pass", you must
    // respond to the check. Evaluating such a position as calm (and cutting on
    // stand_pat >= beta, or only looking at captures) is a CORRECTNESS
    // ERROR: the true value can be radically different, up to
    // mate. So we generate ALL legal evasions (king flight,
    // interpositions, capturing the checking piece — not just captures)
    // and search them. Controlled cost: this block triggers ONLY on
    // quiescence nodes actually in check (a small fraction of the total),
    // unlike the old version active at every leaf.
    let in_check = is_in_check(board, board.side_to_move);
    if in_check {
        // Evasions generated on the stack (zero heap allocation).
        let mut evasions = MoveList::new();
        generate_legal_moves_into(board, &mut evasions);
        if evasions.is_empty() {
            // No legal evasion → checkmate. Mate score adjusted to the
            // distance (faster mates preferred), consistent with alpha_beta().
            return -(SCORE_MATE - ply as i32);
        }
        for &mv in evasions.iter() {
            board.make_move(mv);
            let score = -quiescence(board, -beta, -alpha, ply + 1, info);
            board.unmake_move(mv);

            if score >= beta {
                return beta;        // beta cutoff (fail-hard, like the rest of the file)
            }
            if score > alpha {
                alpha = score;
            }
        }
        return alpha;
    }

    // Score of the position without playing a move (stand pat).
    // (From here on, the side to move is NOT in check.)
    // If this score exceeds beta, the opponent would avoid this branch.
    let stand_pat = evaluate_opt(board, !info.toggles.disable_king_attack);

    if stand_pat >= beta {
        return beta;
    }

    if stand_pat > alpha {
        alpha = stand_pat;
    }

    // Generate captures, evaluate them with SEE, and sort them (best first).
    // SEE pruning: losing captures (SEE < 0) are ignored in quiescence.
    // Rationale: in quiescence we seek to stabilize the position; a
    // losing capture worsens the situation and isn't worth exploring.
    let mut raw_captures = MoveList::new();
    generate_legal_captures_into(board, &mut raw_captures);

    // Precompute SEE scores to avoid redundant calls (sort + filter).
    //
    // Delta Pruning (futility in quiescence) — applied BEFORE computing SEE:
    //   If stand_pat + max_possible_gain + margin ≤ alpha, this move cannot
    //   structurally improve the position, even in the best case
    //   (successful capture without any opponent recapture at all). We discard it without
    //   even computing its SEE — which simulates the entire exchange chain and costs
    //   noticeably more than a simple piece_value() lookup.
    //   Expected gain: fewer quiescence nodes explored in the endgame
    //   or in highly unbalanced positions, without loss of accuracy
    //   (the discarded move could not have changed the result anyway).
    // Retained captures + their SEE score, stored on the STACK (fixed array,
    // parallel to the moves) instead of a Vec<(Move,i32)> allocated on the heap
    // at each quiescence node — the hottest path of the engine.
    // `n` ≤ raw_captures.len() ≤ MAX_MOVE_LIST: no overflow possible.
    let mut scored: [(Move, i32); MAX_MOVE_LIST] = [(Move::NULL, 0); MAX_MOVE_LIST];
    let mut n = 0usize;
    for &mv in raw_captures.iter() {
        let gain_estimate = capture_gain_estimate(board, mv);
        if stand_pat + gain_estimate + DELTA_MARGIN <= alpha {
            continue;
        }
        scored[n] = (mv, see(board, mv));
        n += 1;
    }

    // Sort by descending SEE score: best captures first.
    scored[..n].sort_unstable_by_key(|entry| std::cmp::Reverse(entry.1));

    for &(mv, see_score) in scored[..n].iter() {
        // Skip losing captures (SEE < 0).
        // Since the slice is sorted, as soon as we see SEE < 0 we can stop.
        if see_score < 0 {
            break;
        }

        board.make_move(mv);
        let score = -quiescence(board, -beta, -alpha, ply + 1, info);
        board.unmake_move(mv);

        if score >= beta {
            return beta;
        }
        if score > alpha {
            alpha = score;
        }
    }

    // Note: the GENERATION of quiet moves giving check (the side to move
    // GIVES check without capturing) is deliberately NOT done here — this is what
    // made the old version prohibitive (generate_legal_moves() at
    // every leaf). Handling of positions where the side to move IS IN
    // check, on the other hand, is handled at the top of the function (see the `in_check` block),
    // for a much lower cost and a real correctness gain. Reintroducing the
    // generation of quiet counter-checks would require cheap check
    // detection (without regenerating all moves) AND an NPS/Elo benchmark —
    // to be left for a later, measured iteration.

    alpha
}

// =============================================================================
// Contempt factor
// =============================================================================

/// Returns the score of a drawn position (50-move rule, repetition, insufficient
/// material, stalemate — all causes combined), adjusted by the contempt
/// configured via the UCI option "Contempt" (`info.contempt`, 0 by default =
/// unchanged behavior, exactly SCORE_DRAW).
///
/// Principle: contempt expresses "a draw is slightly UNFAVORABLE
/// from the point of view of the side the engine is currently playing" (useful against
/// a weaker opponent — no point settling for a
/// split of the points). This side is ALWAYS the one to move at the root of the
/// current search (ply == 0), since "go" is only called when
/// it is the engine's turn to play.
///
/// Derivation of parity: each search level returns a score from the
/// point of view of the side to move AT THIS NODE, then negamax flips it at
/// each ply unwind. The side alternates at each ply, so the number
/// of flips between this node and the root is exactly `ply`. For
/// the root to always perceive `-contempt` (slightly unfavorable, regardless
/// of where in the tree the draw is detected):
///   - even ply   → this node IS the root side → return `-contempt` directly
///     (an even number of flips does not change the sign)
///   - odd ply → this node is the OPPONENT of the root side → return
///     `+contempt`, which becomes `-contempt` after the odd flip
///
/// IMPORTANT — multi-thread consistency: the transposition table is
/// SHARED among all Lazy SMP threads. If one thread applied
/// contempt and another did not, the cached draw scores would be
/// inconsistent depending on which thread computed them. `info.contempt` must
/// therefore be identical on ALL threads of a given search — see
/// SearchEngine::search() which copies it to the main thread AND to
/// each secondary thread from the same SearchConfig.
#[inline]
fn draw_score(contempt: i32, ply: usize) -> i32 {
    if contempt == 0 {
        return SCORE_DRAW;
    }
    if ply.is_multiple_of(2) { -contempt } else { contempt }
}

// =============================================================================
// Main alpha-beta search
// =============================================================================

/// Alpha-beta search with all the heuristics of Vendetta Chess Engine.
///
/// Parameters:
///   - board         : current position (modified then restored)
///   - depth         : remaining depth to explore
///   - alpha         : lower bound (best score guaranteed for the side to move)
///   - beta          : upper bound (best score guaranteed for the opponent)
///   - ply           : distance from the root (0 = root)
///   - tt            : shared transposition table (interior mutability)
///   - killers       : killer moves heuristic
///   - history       : history heuristic
///   - countermoves  : countermove heuristic (refutation move)
///   - prev_move     : last move played to reach this node (the one that
///                     countermoves may attempt to refute).
///                     Move::NULL at the root or after a null move (Null
///                     Move Pruning) — in this case, no countermove lookup
///                     is performed for the children of this node.
///   - cont_history  : continuation history (cumulative generalization of
///                     countermove) — uses the same prev_key key.
///   - info          : search statistics and stop signal
///   - excluded_move : move to exclude from the search.
///                     Always Move::NULL in normal calls.
///                     Only non-NULL in the Singular Extension
///                     verification search.
///   - root_moves    : pre-filtered moves for the root (UCI searchmoves).
///                     Empty (&[]) in all internal recursive calls.
///                     Non-empty only at ply==0 from search(): eliminates
///                     generate_legal_moves() AND to_uci() allocations at the root.
// Deliberately high number of arguments: this is the central search
// function, which propagates shared state (TT, heuristics, context) throughout
// the whole recursion. Grouping them into a struct would harm the readability of the
// recursive calls. The manual alignment of the parameter doc is, likewise,
// deliberate (preferred over automatic reformatting).
#[allow(clippy::too_many_arguments, clippy::doc_overindented_list_items)]
pub fn alpha_beta(
    board:         &mut Board,
    mut depth:     i32,
    mut alpha:     i32,
    mut beta:      i32,
    ply:           usize,
    tt:            &TranspositionTable,
    killers:       &mut KillerMoves,
    history:       &mut HistoryTable,
    countermoves:  &mut CountermoveTable,
    cont_history:  &mut ContinuationHistoryTable,
    prev_move:     Move,
    info:          &mut SearchInfo,
    excluded_move: Move,
    root_moves:    &[Move],
) -> i32 {
    // --- Early stop ---
    info.check_time();
    if info.should_stop() {
        return 0;
    }

    info.nodes += 1;

    // --- Draw position detection ---

    // 50-move rule
    if board.halfmove_clock >= 100 {
        return draw_score(info.contempt, ply);
    }

    // Draw by repetition (via Zobrist hash)
    //
    // Fix — adapter order: .skip(1).step_by(2), not the reverse.
    //   The old version (.step_by(2).skip(1)) checked positions at ranks
    //   3, 5, 7… half-moves back, i.e. the positions of the OPPONENT'S turn.
    //   The Zobrist hash encodes the side to move → they can never match
    //   the current hash → the detection was silently non-functional.
    //
    //   Correct sequence:
    //     .skip(1)    — skip the position 1 half-move back (opponent's turn)
    //     .step_by(2) — every 2: positions 2, 4, 6… half-moves back
    //                   (same side to move as the current position, guaranteed by the Zobrist)
    //
    // Optimization — .any() replaces .count() >= 2.
    //   In alpha-beta search, a single prior repetition is enough to declare
    //   a draw (the opponent can always force the repetition on the next move).
    //   .any() exits immediately on the first match: O(1) in case
    //   of repetition, instead of continuing up to halfmove_clock/2 entries.
    if ply > 0 && board.history.len() >= 2 {
        let current_hash  = board.hash;
        let is_repetition = board.history.iter().rev()
            .skip(1)                                     // skip the pos. 1 ply back (opponent's turn)
            .step_by(2)                                  // same side to move as the current pos.
            .take(board.halfmove_clock as usize / 2)     // bounded by the 50-move rule
            .any(|s| s.hash == current_hash);            // exit on the 1st occurrence
        if is_repetition {
            return draw_score(info.contempt, ply);
        }
    }

    // Insufficient material to mate
    if is_insufficient_material(board) {
        return draw_score(info.contempt, ply);
    }

    // --- Transposition table probe ---
    //
    // Important: if a move is excluded (SE verification search), we do NOT do
    // a TT cutoff. The score stored in the TT was computed without exclusion: it
    // accounts for the excluded move and would be incorrect here.
    // However, we still retrieve tt_move for move ordering.
    let tt_entry_opt = tt.probe(board.hash);
    let tt_move = match tt_entry_opt {
        Some(ref entry) => {
            if entry.depth >= depth && excluded_move.is_null() {
                let score = TranspositionTable::adjust_score_from_tt(
                    entry.score, ply as i32,
                );
                match entry.flag {
                    TTFlag::Exact => {
                        // Exact score: we can return directly (except at the root)
                        if ply > 0 { return score; }
                    }
                    TTFlag::LowerBound => {
                        if score >= beta { return beta; }
                    }
                    TTFlag::UpperBound => {
                        if score <= alpha { return alpha; }
                    }
                }
            }
            entry.best_move
        }
        None => Move::NULL,
    };

    // --- Mate Distance Pruning ---
    //
    // Directly tighten [alpha, beta] to the mate scores reachable
    // from THIS node, given its depth (ply) — without relying on
    // any heuristic or approximate margin, just the exact
    // arithmetic of mate scores. A free technique with no
    // tactical risk (unlike RFP/Razoring/LMP): if it cuts, it's
    // a logical certainty, not a gamble.
    //
    //   - Best case: mate the opponent on the next move (ply+1, one move
    //     more than this node) → SCORE_MATE - (ply+1). If beta already exceeds
    //     this bound, we lower it: no move can do better than an
    //     immediate mate.
    //   - Worst case: being mated AT this very node (ply) → -SCORE_MATE + ply. If
    //     alpha is already below this bound, we raise it: nothing worse
    //     can happen to us here than an immediate mate against us.
    //   - If the window collapses (alpha >= beta) after tightening, the
    //     position is already entirely determined by mate distance
    //     alone: we return without generating a single move.
    //
    // Placed after the TT probe (which can already cut earlier in the
    // general case) and before dispatching to quiescence — it thus applies
    // uniformly to ALL nodes, including those about to dive
    // into depth 0.
    let mate_score_for_us     = SCORE_MATE - (ply as i32 + 1);
    let mate_score_against_us = -SCORE_MATE + ply as i32;
    if beta  > mate_score_for_us     { beta  = mate_score_for_us; }
    if alpha < mate_score_against_us { alpha = mate_score_against_us; }
    if alpha >= beta {
        return alpha;
    }

    // --- Leaf node: quiescence search ---
    if depth <= 0 {
        return quiescence(board, alpha, beta, ply, info);
    }

    let in_check = is_in_check(board, board.side_to_move);

    // Countermove key of the current node: (piece, destination square) of the last
    // move played to REACH this node (already applied on `board`). None if
    // no previous move is tracked (root, or child of a null move).
    // board.piece_at(prev_move.to) reads the CURRENT state of the board, which already
    // reflects this move — no additional information to propagate besides
    // the Move itself.
    //
    // FIXED (robustness audit, point 2): for a promotion, piece_at()
    // would read the piece AFTER promotion (e.g. Queen) rather than the piece that
    // actually made the move (a Pawn — by definition, no other piece
    // can promote). Explicit special case, without needing to
    // re-read the board or propagate extra information.
    let prev_key: Option<(Piece, u8)> = if !prev_move.is_null() {
        if prev_move.flags.is_promotion() {
            Some((Piece::Pawn, prev_move.to))
        } else {
            board.piece_at(prev_move.to).map(|(piece, _)| (piece, prev_move.to))
        }
    } else {
        None
    };

    // Static evaluation computed once and shared between the
    // Reverse Futility Pruning and Razoring below (previously, evaluate()
    // was called a second time in the Razoring block — redundant).
    //
    // Disabled (skip = None) if the conditions common to both techniques
    // are not met: in check, at the root, or in an SE search
    // (Singular Extension — excluded_move non-null). These three cases make
    // the static evaluation irrelevant or the cuts dangerous.
    // Node's Correction History keys, computed ONCE here and then
    // reused at the learning site (at the end of the function). None if the
    // correction does not apply (same conditions as static_eval_opt), or
    // if the runtime switch disables it (SPRT tests).
    let corr_keys_opt = if !in_check && ply > 0 && excluded_move.is_null() && !info.toggles.disable_correction {
        Some(corr_keys(board, prev_move))
    } else {
        None
    };

    let static_eval_opt = if !in_check && ply > 0 && excluded_move.is_null() {
        // Static eval CORRECTED by the Correction History (⚠️ to be validated via SPRT):
        // we add the learned correction (weighted average of several tables:
        // pawns, non-pawn pieces by color, continuation). ALL downstream
        // pruning (RFP, Razoring, NMP, futility) and the `improving` flag use
        // this corrected eval — a better-calibrated eval refines the cut margins.
        let raw_eval = evaluate_opt(board, !info.toggles.disable_king_attack);
        // corr_keys_opt = None ⇒ correction disabled (toggle) or not applicable:
        // the eval remains RAW.
        let correction = match &corr_keys_opt {
            Some(keys) => info.correction_history.value(keys),
            None => 0,
        };
        Some(raw_eval + correction)
    } else {
        None
    };

    // --- Stack of static evaluations per ply + "improving" flag ---
    //
    // (RE-ENABLED — the bug that had caused this feature to be disabled is fixed,
    // see below.)
    //
    // "improving" answers: is the position of the side to move BETTER
    // than 2 plies ago? (The side to move alternates every ply; ply and ply-2 are
    // therefore always the SAME side to move, a direct and valid
    // eval comparison.) If so, we trust the scores more: RFP cuts a
    // bit more easily, NMP reduces a bit more, LMP prunes a bit less aggressively.
    //
    // FIX FOR THE ORIGINAL BUG — the old version only wrote
    // eval_history[ply] when a static eval was available (i.e.
    // NOT in check). A node in check would then leave at this index the value
    // of ANOTHER branch explored earlier at the same ply; a descendant at
    // ply+2 would read it as if it were its own grandparent's value →
    // a pruning decision based on an unrelated position.
    //
    // The fix: write eval_history[ply] on EVERY actual visit to the node,
    // UNCONDITIONALLY — the real eval if available, otherwise the sentinel
    // EVAL_HISTORY_NONE (in check, root). The invariant is then guaranteed: during
    // the exploration of a node's subtree at ply P, eval_history[P] contains
    // always exactly what THIS node wrote there. (Its descendants only write
    // at indices >= P+1; its siblings already explored have returned, and their
    // writes at indices >= P+1 never alter index P.) So reading
    // eval_history[ply-2] ALWAYS returns the eval of the ancestor located 2 plies
    // higher up on THE current path — never that of another branch.
    // When this ancestor was in check (sentinel), improving = false (the
    // most cautious setting). This is the technique used by modern engines (the
    // ss->staticEval stack in Stockfish).
    //
    // EXCEPTION — Singular Extension search: it calls alpha_beta() again
    // at the SAME ply (no move played) with excluded_move non-null. If it wrote
    // eval_history[ply], it would overwrite the value that the ENCLOSING node
    // legitimately placed there, corrupting the index for the rest of the
    // ACTUAL exploration of this node. We therefore only write when excluded_move is null
    // (actual visit). The SE search is on the same
    // position anyway: its descendants reading eval_history[ply] find the correct
    // value already in place.
    if excluded_move.is_null() && ply < MAX_PLY {
        info.eval_history[ply] = static_eval_opt.unwrap_or(EVAL_HISTORY_NONE);
    }

    let improving = if info.toggles.disable_improving {
        // Runtime switch (SPRT tests, see bin/selfplay.rs): `improving`
        // forced to false. No effect in normal play (disable_improving = false).
        false
    } else {
        match static_eval_opt {
            Some(current_eval) if (2..MAX_PLY).contains(&ply) => {
                let prev = info.eval_history[ply - 2];
                prev != EVAL_HISTORY_NONE && current_eval > prev
            }
            _ => false,
        }
    };

    // --- Reverse Futility Pruning (Static Null Move) ---
    // If the static evaluation already exceeds beta by a wide margin, the opponent would
    // never let the game reach this position: we cut without
    // even trying the null move. This is the mirror of Razoring below —
    // this one cuts on the beta side (position too good), Razoring cuts on the
    // alpha side (position too bad).
    //
    // Not disabled in a null window here, unlike Razoring:
    // RFP actually benefits the most from non-PV nodes (null window), which
    // are the most numerous in the PVS tree.
    //
    // Margin: 120 centipawns per ply of remaining depth — a position
    // evaluated at +120*depth above beta has very little chance of being
    // returned by a deeper search at this depth.
    //
    // "improving" adjustment: if the position is improving (see above),
    // the margin is reduced by 120 cp (depth - 1 instead of depth) — we
    // trust an already good score more when the trend confirms it,
    // so we cut more easily. If the position is NOT improving,
    // the full margin remains in effect (more cautious, cuts less often).
    const RFP_MAX_DEPTH:        i32 = 6;
    const RFP_MARGIN_PER_DEPTH: i32 = 120;

    if let Some(static_eval) = static_eval_opt {
        let rfp_margin = RFP_MARGIN_PER_DEPTH * (depth - improving as i32);
        if depth <= RFP_MAX_DEPTH
            && static_eval - rfp_margin >= beta
            && static_eval.abs() < SCORE_MATE - 200
        {
            // Fail-hard: we return `beta`, not the raw score, for consistency
            // with all other cutoffs in this file (Null Move Pruning,
            // stand-pat in quiescence, etc.).
            return beta;
        }
    }

    // --- Razoring ---
    // (previously named "Futility Pruning" in this file — fixed:
    // standard terminology calls "Razoring" the cut at the NODE
    // LEVEL based on alpha, and reserves "Futility Pruning" for a PER-MOVE
    // cut inside the quiet-move loop. This file
    // only implements the "node" version — Razoring is therefore the correct name.)
    //
    // At depths 1-2, if the static evaluation + a safety margin
    // is still below alpha, quiet moves cannot save the
    // position: we drop directly into quiescence.
    //
    // Disabled if:
    //   - In check, at the root, or in an SE search (cf. static_eval_opt above)
    //   - Null window (alpha == beta - 1): extra safety,
    //     unlike RFP, which benefits from null windows
    //   - Score close to a mate
    if let Some(static_eval) = static_eval_opt {
        if depth <= 2
            && alpha != beta - 1
        {
            let razoring_margin = 150 * depth;

            if static_eval + razoring_margin <= alpha
                && static_eval.abs() < SCORE_MATE - 200
            {
                return quiescence(board, alpha, beta, ply, info);
            }
        }
    }

    // --- Null Move Pruning ---
    // We pass our turn: if the score still exceeds beta, the position is
    // too good for the opponent → cutoff without exploring.
    //
    // "improving" adjustment: reduction R=4 if the position is improving (more
    // aggressive — the recent dynamics make a zugzwang trap less
    // likely), R=3 otherwise (original setting, more cautious). `.max(0)` in
    // case R=4 would bring the child's depth below zero (possible at
    // depth==3 with R=4: 3-4=-1) — the existing guard `depth <= 0 →
    // quiescence` at the top of the function handles 0 without issue, `.max(0)`
    // simply avoids needlessly passing a negative depth.
    //
    // Disabled if:
    //   - In check
    //   - Depth < 3
    //   - Root
    //   - SE search: the null move interacts poorly with the exclusion
    //   - King + pawns only (zugzwang possible)
    if !in_check
        && depth >= 3
        && ply > 0
        && excluded_move.is_null()
    {
        let side     = board.side_to_move;
        let non_pawn = board.occupancy[side.index()]
            & !board.pieces[side.index()][Piece::Pawn.index()]
            & !board.pieces[side.index()][Piece::King.index()];

        if non_pawn != 0 {
            let nmp_reduction = if improving { 4 } else { 3 };
            let null_depth    = (depth - nmp_reduction).max(0);
            let prev_ep    = board.make_null_move();
            let null_score = -alpha_beta(
                board,
                null_depth,
                -beta,
                -beta + 1,
                ply + 1,
                tt, killers, history, countermoves, cont_history,
                Move::NULL, // no real move to trace after a null move
                info,
                Move::NULL,
                &[],
            );
            board.unmake_null_move(prev_ep);

            if null_score >= beta {
                return beta;
            }
        }
    }

    // --- Singular Extension ---
    //
    // A move is "singular" if it is the only one that maintains the score.
    // Mechanism:
    //   1. We take the TT move (the best known move for this position).
    //   2. We run a reduced-depth search EXCLUDING it.
    //   3. If all other moves fail below (tt_score - margin):
    //      → The TT move is singular, we extend it by +1 in the main loop.
    //   4. If even without the TT move the score exceeds beta (multi-cut):
    //      → There are several good moves, we can cut directly.
    //
    // Activation conditions (all required):
    //   depth >= 6   : SE is expensive (~50% more nodes), useless near the bottom
    //   ply > 0      : never at the root
    //   excluded_move.is_null() : never in a nested SE search
    //   !in_check    : the Check Extension already handles positions in check
    //   tt_move non-null + reliable TT entry (depth and flag)
    //   TT score far from a mate
    let mut singular_extension = 0i32;

    if depth >= 6
        && ply > 0
        && excluded_move.is_null()
        && !in_check
        && !tt_move.is_null()
    {
        if let Some(ref entry) = tt_entry_opt {
            // The TT entry must be deep enough to be reliable.
            // UpperBound → TT score may be overestimated → unreliable for SE.
            if entry.depth >= depth - 3 && entry.flag != TTFlag::UpperBound {
                let tt_score = TranspositionTable::adjust_score_from_tt(
                    entry.score, ply as i32,
                );

                // Do not apply SE near a mate score
                if tt_score.abs() < SCORE_MATE - 200 {
                    // Conservative margin: 2 cp × depth.
                    // Too small → too many extensions → time explosion.
                    // Too large → too few extensions → SE useless.
                    let se_margin = 2 * depth;

                    // se_beta: floor that other moves must clear
                    // to disprove the singularity of the TT move.
                    let se_beta  = (tt_score - se_margin).max(-SCORE_MATE + 1);

                    // Verification depth: about half.
                    // Sufficient to detect singularity without excessive cost.
                    let se_depth = (depth - 1) / 2;

                    // Verification search.
                    // The position on the board is NOT modified (no make_move).
                    // The TT move is passed as excluded_move → it will be skipped.
                    let se_score = alpha_beta(
                        board,
                        se_depth,
                        se_beta - 1,  // Null window: [se_beta-1, se_beta]
                        se_beta,
                        ply,          // Same ply: no move has been played
                        tt, killers, history, countermoves, cont_history,
                        prev_move,    // No move played here: same context as the current node
                        info,
                        tt_move,      // ← Exclusion of the TT move
                        &[],
                    );

                    if !info.should_stop() {
                        if se_score < se_beta {
                            // Singular confirmed: the TT move is the only good move
                            // in this position → it will be extended by +1 in the loop.
                            singular_extension = 1;
                        } else if se_score >= beta {
                            // Multi-cut: even without the TT move, the score exceeds beta.
                            // There are therefore several good moves → we can cut.
                            return beta;
                        }
                    }
                }
            }
        }
    }

    // --- Internal Iterative Reduction (IIR) ---
    //
    // If the TT has NO move for this node (never visited, or visited at a
    // depth insufficient to have stored a useful move), it is the
    // sign that this branch is little explored — we reduce the depth
    // by 1 before continuing, rather than treating it with the same
    // confidence as a node already well documented by the TT.
    //
    // Replaces the old Internal Iterative Deepening (IID) — which launched
    // a reduced-depth search ONLY to guess a good move
    // before continuing (cost: an additional recursive call). IIR
    // performs NO recursive call: a simple conditional subtraction
    // on `depth`, which then propagates naturally to the rest of the
    // processing of this node (move loop, child depths,
    // TT storage at the end of the function). This is the version used by
    // modern engines (including Stockfish), much less costly than the classic
    // IID for a comparable effect.
    //
    // Disabled if:
    //   - TT move present (tt_move non-null): nothing to correct, the node is
    //     already well documented
    //   - depth < IIR_MIN_DEPTH: the reduction is no longer useful at very
    //     low depth (quiescence or other pruning already handle that)
    //   - SE search (excluded_move non-null): do not disturb the
    //     singularity verification with a depth already reduced by
    //     something other than itself
    const IIR_MIN_DEPTH: i32 = 4;
    if tt_move.is_null() && depth >= IIR_MIN_DEPTH && excluded_move.is_null() {
        depth -= 1;
    }

    // --- Legal move generation ---
    //
    // Root with pre-filtered searchmoves (root_moves non-empty):
    //   We use directly the list prepared by search().
    //   Advantages vs the old internal filter:
    //     - Zero generate_legal_moves() call at the root.
    //     - Zero String allocation (to_uci() is a thing of the past).
    //     - The filter is applied ONLY ONCE before the depth iteration.
    //     - SearchInfo no longer has a heap-allocated Vec<String> propagated to each node.
    //
    //   excluded_move is always NULL at ply==0 (SE is only active at ply>0)
    //   → no retain() needed in this case.
    //
    // Recursive calls (root_moves empty): normal generation + SE exclusion.
    // Move list allocated on the STACK (MoveList) — no heap allocation per
    // node, unlike the old Vec<Move>. Indexing, len(), iter(),
    // swap() and slicing work via Deref to [Move].
    let mut moves = MoveList::new();
    if ply == 0 && !root_moves.is_empty() {
        for &mv in root_moves {
            moves.push(mv);
        }
    } else {
        generate_legal_moves_into(board, &mut moves);
        if !excluded_move.is_null() {
            moves.retain(|mv| *mv != excluded_move);
        }
    }

    // --- End of game ---
    if moves.is_empty() {
        if in_check {
            // Checkmate: prefer fast mates (small ply)
            return -(SCORE_MATE - ply as i32);
        } else {
            // Stalemate
            return draw_score(info.contempt, ply);
        }
    }

    // --- Lazy selection sort ---
    //
    // Principle: instead of fully sorting N moves in O(N log N),
    //   we compute all scores in one O(N) pass and then select
    //   the best remaining move at each iteration via a linear scan.
    //
    //   Total cost: O(N) scores + O(k × N) selections for k moves examined.
    //
    //   Gain vs full sort O(N log N + N × processing):
    //     If the beta cutoff occurs at the k-th move (k ≪ N), we save
    //     O(N log N − k × N) operations. With a high cutoff rate
    //     (TT move / killer at the top), k is typically 1–3 for most
    //     internal nodes → substantial savings across the whole tree.
    //
    //   Unfavorable case: k = N (no cutoff) → O(N²) instead of O(N log N).
    //   In practice extremely rare at high depths thanks to the TT move.
    debug_assert!(moves.len() <= MAX_MOVES,
        "alpha_beta: {} coups dépasse MAX_MOVES={}", moves.len(), MAX_MOVES);
    let move_count = moves.len();
    let mut scores = [0i32; MAX_MOVES];
    for (i, &mv) in moves.iter().enumerate() {
        scores[i] = move_score(board, mv, tt_move, killers, history, countermoves, cont_history, prev_key, ply);
    }

    // BUG AVOIDED (post-LMP robustness audit): a move pruned by Late Move
    // Pruning is NEVER actually searched — it does not "lose", it is
    // simply not examined. Without this tracking, history.update_bad() below
    // (triggered by a beta cutoff on a later move) would also penalize
    // the pruned moves as if they had been tried and had
    // failed, even though they were never submitted to a search. Silently
    // degrades the quality of the history heuristic without ever crashing
    // — so never detected by the existing perft/benchmark tests.
    let mut lmp_pruned = [false; MAX_MOVES];

    // --- Move exploration ---
    let mut best_score = -SCORE_INF;
    let mut best_move  = Move::NULL; // updated as soon as the first move is examined
    let mut tt_flag    = TTFlag::UpperBound;

    for move_index in 0..move_count {
        // Selection of the best remaining move: O(N − move_index) scan.
        // We bring the move with the maximum score to position `move_index` by swapping,
        // so that moves[0..=move_index] always contains the moves examined
        // in decreasing score order (useful for history.update_bad below).
        let best_idx = {
            let mut b = move_index;
            for j in (move_index + 1)..move_count {
                if scores[j] > scores[b] { b = j; }
            }
            b
        };
        moves.swap(move_index, best_idx);
        scores.swap(move_index, best_idx);

        // mv is a copy (Move implements Copy) — no dereferencing required.
        let mv = moves[move_index];

        // --- info currmove / currmovenumber (UCI, root only) ---
        // Pure visual feedback for the GUI ("currently analyzing: such move, nth
        // of N") — no effect on the search itself. Emitted only at
        // ply == 0 (never in the hot loop of internal nodes) AND
        // only if info.show_currmove is active.
        //
        // BUG FIXED: the first version printed unconditionally as soon as
        // ply == 0, which polluted the output of ANY caller of alpha_beta()
        // — including src/bin/benchmark.rs, which calls alpha_beta() directly
        // (outside the UCI layer) to measure raw NPS. The safeguard
        // show_currmove (false by default, enabled only by
        // SearchEngine::search(), the real search driven by the UCI) cleanly
        // isolates this UCI-specific behavior from the rest of the uses
        // of alpha_beta() in the project.
        if ply == 0 && info.show_currmove {
            println!("info currmove {} currmovenumber {}", mv.to_uci(), move_index + 1);
        }

        board.make_move(mv);

        // TT prefetch: board.hash is now the hash of the
        // CHILD position, which the recursive descent will probe first. We
        // launch the loading of the cache line NOW so that it is hot
        // by the time of the child's probe() — the memory latency is masked by
        // the extension / LMP calculation that follows. Pure speed, zero side effect.
        tt.prefetch(board.hash);

        // --- Extension calculation for this move ---
        //
        // Check Extension: the move puts the opponent in check → critical position.
        //   Cumulative conditions:
        //     1. gives_check      : the move actually gives check
        //     2. depth <= 4       : useful only at low remaining depth
        //     3. ply + 1 < MAX_PLY : CRITICAL safety bound against infinite recursion.
        //
        //   Without condition (3), if depth == 4 and extension == 1:
        //     depth_child = 4 - 1 + 1 = 4  → the depth DOES NOT DECREASE.
        //   Any sequence of moves each giving check creates infinite recursion.
        //   Beyond MAX_PLY plies, the extension is disabled; depth goes to 3,
        //   then 2, then 1, then 0 → quiescence → guaranteed termination.
        //
        // Singular Extension: this move is the only good one in the position.
        //   Applies ONLY to the TT move (the one whose singularity was verified).
        //   The two extensions are mutually exclusive by construction
        //   (Check Extension takes priority if the move also gives check).
        //
        //   BUG FIXED (post-session audit): this extension suffered from the same
        //   flaw as the Check Extension before its fix — it added +1
        //   to the child's depth (depth_child = depth-1+1 = depth) WITHOUT
        //   the `ply + 1 < MAX_PLY` bound. Since the Singular Extension triggers
        //   at depth >= 6, the child remained at depth >= 6 and could, in theory,
        //   in turn be judged singular at the next node — repeating the phenomenon
        //   without the depth ever decreasing. Unlike perpetual checks
        //   (which eventually repeat a position, detected by the
        //   draw rule), a chain of "singular" positions has no
        //   reason to repeat the board: nothing else would have stopped the
        //   recursion, with a risk of stack overflow. Same guard as the
        //   Check Extension, applied here for consistency and safety.
        let gives_check = is_in_check(board, board.side_to_move);
        let extension   = if gives_check && depth <= 4 && ply + 1 < MAX_PLY {
            1  // Check Extension
        } else if mv == tt_move && singular_extension > 0 && ply + 1 < MAX_PLY {
            singular_extension  // Singular Extension (+1)
        } else {
            0
        };

        // --- Late Move Pruning (LMP) ---
        //
        // At shallow depths, a quiet move that arrives very late
        // in the ordering (many better-ranked moves have already preceded it)
        // has such a low probability of improving alpha that searching its
        // entire subtree is almost never worthwhile. Unlike LMR,
        // which only reduces the depth of the probe, here we do NOT
        // search this move AT ALL at this depth — maximum gain, but also
        // the most aggressive pruning in this file.
        //
        // Safe ONLY because the Countermove Heuristic is now in
        // place: move ordering must be reliable for a "late" move
        // to truly be a bad candidate rather than a good move that was poorly ranked.
        // Implemented AFTER the Countermove Heuristic in this project, not before,
        // precisely for this reason.
        //
        // Disabled if:
        //   - Current node in check (in_check): few legal moves, often
        //     all tactical — never LMP in these positions
        //   - The move gives check (gives_check): critical position
        //   - The move received an extension (Check or Singular): we just
        //     decided it deserved ONE MORE PLY, pruning
        //     here would be contradictory
        //   - Capture or promotion: already well ordered by SEE, never pruned
        //   - Killer move or countermove (FIXED — robustness audit): these
        //     are precisely the moves for which we have PROOF that they were
        //     effective elsewhere in the tree (killer) or against this type of
        //     opponent move (countermove). Without this exemption, a move with
        //     a track record of success could be skipped simply because
        //     several winning captures preceded it in the ordering —
        //     contrary to standard practice, which always protects these
        //     two categories from Late Move Pruning.
        //   - Depth > LMP_MAX_DEPTH: safety margin, naturally becomes a
        //     no-op beyond that (see the lmp_threshold comment)
        let is_killer_move = killers.is_killer(mv, ply);
        let is_countermove = prev_key.is_some_and(|(p, t)| countermoves.get(p, t) == mv);

        if !in_check
            && !gives_check
            && extension == 0
            && depth <= LMP_MAX_DEPTH
            && !mv.flags.is_capture()
            && !mv.flags.is_promotion()
            && !is_killer_move
            && !is_countermove
            && move_index >= lmp_threshold(depth, improving)
        {
            lmp_pruned[move_index] = true;
            board.unmake_move(mv);
            continue;
        }

        // --- Futility Pruning (per move) ---
        //
        // ⚠️ HEURISTIC TO BE VALIDATED BY SPRT MATCH before being considered
        // settled (the margins below are a CONSERVATIVE starting point, to
        // be tuned by A/B testing — see the Elo testing method). Can be disabled
        // at runtime via `info.toggles.disable_futility` (used by the
        // selfplay binary for SPRT matches, keys futility_a / futility_b).
        //
        // Idea: near the leaves, a QUIET move that does not give check can
        // barely raise alpha if the node's static evaluation,
        // increased by a margin, is already below alpha. We skip it without
        // searching it. Complementary to the two other prunings already present:
        //   - Razoring : cuts at the NODE LEVEL (before the move loop);
        //   - LMP      : cuts on the NUMBER of moves already examined;
        //   - Futility : cuts MOVE BY MOVE, on the static SCORE vs alpha.
        //
        // Conditions (all required, modeled on LMP for safety):
        //   - static_eval available (so outside check, outside root, outside SE);
        //   - move_index > 0: we ALWAYS keep the main move;
        //   - extension == 0: never prune a move we just extended;
        //   - low depth: the margin only covers the risk at low depth;
        //   - quiet move not giving check;
        //   - neither killer nor countermove (effectiveness already proven elsewhere);
        //   - alpha outside the mate zone (for safety);
        //   - static_eval + margin <= alpha.
        const FUTILITY_MAX_DEPTH:        i32 = 6;
        const FUTILITY_MARGIN_PER_DEPTH: i32 = 100;

        if let Some(static_eval) = static_eval_opt {
            if !info.toggles.disable_futility
                && move_index > 0
                && extension == 0
                && depth <= FUTILITY_MAX_DEPTH
                && !gives_check
                && !mv.flags.is_capture()
                && !mv.flags.is_promotion()
                && !is_killer_move
                && !is_countermove
                && alpha < SCORE_MATE - 200
                && static_eval + FUTILITY_MARGIN_PER_DEPTH * depth <= alpha
            {
                // Not searched → excluded from history.update_bad (like LMP),
                // via the same lmp_pruned[] marking.
                lmp_pruned[move_index] = true;
                board.unmake_move(mv);
                continue;
            }
        }

        // --- Principal Variation Search (PVS) + Late Move Reduction (LMR) ---
        //
        // Move #1 (move_index == 0): this is the move ranked best by
        // the ordering (TT move, or best capture/killer/history
        // otherwise). We search it directly with the FULL WINDOW [alpha, beta]
        // to obtain an exact score that will serve as a reference for the
        // following moves.
        //
        // Following moves (move_index > 0): 3-step PVS search.
        //   1. Probe with a NULL WINDOW [-alpha-1, -alpha] — possibly at
        //      reduced depth if the LMR criteria are met (move
        //      late, quiet, sufficient depth). This probe only
        //      answers the question "does this move exceed alpha?" — it
        //      cuts off much earlier than a full window and this is what
        //      makes the whole gain of PVS (10-20% fewer nodes).
        //   2. If an LMR reduction was applied AND the probe exceeds
        //      alpha: we do not yet know if this is a genuinely good move or a
        //      false positive caused by the reduction. We reconfirm at FULL
        //      DEPTH but still with a null window, before considering
        //      the most costly search (step 3).
        //   3. If the move actually exceeds alpha (and stays below beta):
        //      it potentially belongs to the principal variation. Only
        //      this situation justifies a re-search with FULL WINDOW and
        //      full depth, to obtain its exact score.
        //
        // In the vast majority of nodes, step 1 is sufficient (the move does
        // not exceed alpha): this is precisely where the PVS gain lies.
        let score = if move_index == 0 {
            // Main move: always full window, never reduced.
            -alpha_beta(
                board,
                depth - 1 + extension,
                -beta,
                -alpha,
                ply + 1,
                tt, killers, history, countermoves, cont_history,
                mv, // mv has just been played: it is the child's prev_move
                info,
                Move::NULL,
                &[],
            )
        } else {
            // LMR criteria: late move, quiet, sufficient depth,
            // neither in check nor giving check (these positions are too critical
            // to be reduced).
            let do_lmr = depth >= 3
                && move_index >= 3
                && !in_check
                && !gives_check
                && !mv.flags.is_capture()
                && !mv.flags.is_promotion();

            // "Normal" depth for this move — STRICTLY identical to the one
            // used by move #1 (move_index == 0). This is the reference for
            // comparison: all moves of the same node must be measured
            // at the same depth for their scores to be comparable.
            let full_depth = depth - 1 + extension;

            // BUG FIXED: the previous version applied `.max(1)` to ALL
            // non-PV moves, even when do_lmr == false (reduction == 0).
            // However when full_depth == 0 (very frequent: this occurs at
            // EVERY node of the entire tree where exactly 1 ply remains to
            // be explored, at every iteration of iterative deepening), the `.max(1)`
            // forced these moves to be searched at depth 1 — that is,
            // ONE PLY MORE than move #1, which itself dove directly into
            // quiescence at depth 0. This systematic inconsistency made
            // score comparisons invalid throughout the whole tree:
            // move #1 (often a generic move at the start of the list, for
            // example an edge pawn push) appeared artificially
            // safe because it was evaluated less deeply, while genuinely
            // relevant moves were penalized by a deeper analysis revealing
            // their real drawbacks. Observed result: the engine systematically
            // played uninteresting moves (edge pawns) at the start
            // of the game, a sign that the search was no longer correctly comparing
            // moves against each other.
            //
            // Fix: the `.max(1)` (and the reduction itself) now
            // applies ONLY if do_lmr is true. If do_lmr is
            // false, probe_depth == full_depth, EXACTLY like move #1.
            let (probe_depth, reduced) = if do_lmr {
                // Base reduction: logarithmic table (depth × move_index).
                let mut r = lmr_reduction(depth, move_index);

                // --- Enriched LMR (adjustments ⚠️ TO BE VALIDATED BY SPRT) ---
                // Signals not already captured by the move's rank
                // (move_index already reflects the ordering by history):
                //   - position that is NOT improving → reduce a bit MORE
                //     (a late move has even less chance of helping);
                //   - PV node (large, non-null window [alpha,beta]) → reduce
                //     a bit LESS (more precision on the principal variation);
                //   - killer or countermove → reduce a bit LESS (quiet move
                //     whose effectiveness has already been proven elsewhere).
                // Deliberately small adjustments (±1) and bounded (r ≥ 1).
                // Can be disabled at runtime via `info.toggles.disable_lmr_tweaks`
                // (selfplay binary, keys lmr_a / lmr_b for SPRT matches):
                // when active, we keep only the BASE reduction, without the
                // enrichment adjustments — this is exactly what we are testing.
                if !info.toggles.disable_lmr_tweaks {
                    let is_pv = beta - alpha > 1;
                    if !improving                       { r += 1; }
                    if is_pv                            { r -= 1; }
                    if is_killer_move || is_countermove { r -= 1; }
                }
                r = r.max(1); // at least 1 of reduction when LMR applies

                ((full_depth - r).max(1), true)
            } else {
                (full_depth, false)
            };

            // --- Step 1: null-window probe (reduced depth if LMR) ---
            let mut s = -alpha_beta(
                board,
                probe_depth,
                -alpha - 1,
                -alpha,
                ply + 1,
                tt, killers, history, countermoves, cont_history,
                mv,
                info,
                Move::NULL,
                &[],
            );

            // --- Step 2: reconfirmation at full depth (null window) ---
            // Only if a reduction was applied AND the probe
            // exceeded alpha — otherwise this step is unnecessary (without a reduction,
            // step 1 was already at full_depth, identical to move #1).
            if reduced && s > alpha {
                s = -alpha_beta(
                    board,
                    full_depth,
                    -alpha - 1,
                    -alpha,
                    ply + 1,
                    tt, killers, history, countermoves, cont_history,
                    mv,
                    info,
                    Move::NULL,
                    &[],
                );
            }

            // --- Step 3: re-search with full window (true PV) ---
            // The move genuinely exceeds alpha and stays below beta: it is
            // part of the principal variation, we need its exact score.
            if s > alpha && s < beta {
                s = -alpha_beta(
                    board,
                    full_depth,
                    -beta,
                    -alpha,
                    ply + 1,
                    tt, killers, history, countermoves, cont_history,
                    mv,
                    info,
                    Move::NULL,
                    &[],
                );
            }

            s
        };

        board.unmake_move(mv);

        if info.should_stop() {
            return 0;
        }

        if score > best_score {
            best_score = score;
            best_move  = mv;

            if score > alpha {
                alpha   = score;
                tt_flag = TTFlag::Exact;
                info.update_best_move(mv, score, ply);
            }
        }

        // --- Beta cutoff ---
        if score >= beta {
            // Remember the killer move and update the history.
            // moves[..move_index] contains the moves examined before mv,
            // in lazy sort order (from best score to worst) —
            // EXCEPT those marked lmp_pruned[i]: these moves were never
            // actually searched (Late Move Pruning skipped them), so they
            // must not be treated as failures by
            // history.update_bad() (see the comment above the
            // declaration of lmp_pruned, earlier in this function).
            if !mv.flags.is_capture() {
                killers.store(mv, ply);
                history.update_good(board, mv, depth);
                if let Some((prev_piece, prev_to)) = prev_key {
                    countermoves.store(prev_piece, prev_to, mv);
                    cont_history.update_good(prev_piece, prev_to, board, mv, depth);
                }
                for (i, prev_mv) in moves[..move_index].iter().enumerate() {
                    if lmp_pruned[i] { continue; }
                    if !prev_mv.flags.is_capture() {
                        history.update_bad(board, *prev_mv, depth);
                        if let Some((prev_piece, prev_to)) = prev_key {
                            cont_history.update_bad(prev_piece, prev_to, board, *prev_mv, depth);
                        }
                    }
                }
            }

            // Store in TT as a lower bound
            let tt_score = TranspositionTable::adjust_score_for_tt(beta, ply as i32);
            tt.store(board.hash, tt_score, depth, TTFlag::LowerBound, best_move);

            return beta;
        }
    }

    // --- Update the Correction History (⚠️ to validate with SPRT) ---
    // We learn the gap between the search score and the CORRECTED static eval
    // of the node. `board` has returned to the node's state (all moves undone) → the
    // `corr_keys_opt` keys computed at the top of the function are still valid.
    // The learning is WEIGHTED BY `depth` (deeper search = more reliable).
    // Conditions: correction active (corr_keys_opt is Some, which already covers
    // not-in-check / ply>0 / not-in-SE / toggle), best move quiet (relevant
    // positional eval), score outside mate range. Simplified version: the
    // beta-cutoff nodes (which return earlier) are not updated.
    if let (Some(keys), Some(corrected_eval)) = (&corr_keys_opt, static_eval_opt) {
        let quiet_best = best_move.is_null() || !best_move.flags.is_capture();
        if quiet_best && best_score.abs() < SCORE_MATE - 200 {
            info.correction_history.update(keys, best_score - corrected_eval, depth);
        }
    }

    // --- Store the result in the transposition table ---
    let tt_score = TranspositionTable::adjust_score_for_tt(best_score, ply as i32);
    tt.store(board.hash, tt_score, depth, tt_flag, best_move);

    best_score
}
