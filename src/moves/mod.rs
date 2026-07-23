// =============================================================================
// Vendetta Chess Engine — src/moves/mod.rs
//
// Role: Coordinator of move generation. This module is the main entry
//        point for obtaining the list of legal moves of a
//        position.
//
// Contents:
//   - is_square_attacked() : detects if a square is attacked by a color
//   - is_in_check()        : detects if a king is in check
//   - generate_legal_moves() : generates all LEGAL moves (zero illegal move)
//   - generate_pseudo_moves() : generates pseudo-legal moves (internal use)
//   - perft()              : test function to validate move generation
//
// Legal generation principle:
//   1. We generate all pseudo-moves (may leave the king in check)
//   2. For each pseudo-move, we play it on the board
//   3. If the king is not in check after the move, it is a legal move
//   4. We undo the move
//
//   For castling, we additionally check that the king does not pass through a
//   square that is attacked and is not in check at the start.
//
// Philosophy: absolute correctness. Zero illegal move possible.
// =============================================================================

pub mod pawn;
pub mod knight;
pub mod bishop;
pub mod rook;
pub mod queen;
pub mod king;

use crate::utils::types::{Color, Piece, Move, MoveFlags};
use crate::board::state::Board;
use crate::board::bitboard::{
    Bitboard, pop_lsb,
    knight_attacks, king_attacks,
    rook_attacks, bishop_attacks, queen_attacks,
    white_pawn_attacks, black_pawn_attacks,
    FILE_A, FILE_H, RANK_1, RANK_8,
};

// =============================================================================
// MoveList — fixed-capacity move list, allocated on the STACK
// =============================================================================

/// Maximum capacity of a MoveList.
///
/// The record for LEGAL moves in a chess position is 218; the
/// pseudo-legal moves generated before filtering remain bounded well below. 256
/// offers a comfortable margin while remaining a power of 2.
pub const MAX_MOVE_LIST: usize = 256;

/// Fixed-capacity move list (`[Move; 256]`), allocated on the STACK — therefore
/// NO heap allocation per node, unlike `Vec<Move>`. This is the most
/// significant memory gain on the hot path: quiescence represents
/// 80-90% of nodes, each generating at least one move list.
///
/// Zero external dependency (no `arrayvec`) — consistent with the project's
/// philosophy. Behaves like a `[Move]` slice via Deref/DerefMut: `len()`,
/// `iter()`, indexing, `swap()`, slicing, `to_vec()`, `is_empty()` work
/// directly, without a dedicated method.
pub struct MoveList {
    moves: [Move; MAX_MOVE_LIST],
    len:   usize,
}

impl MoveList {
    /// Creates an empty list. The buffer is pre-filled with `Move::NULL` (never
    /// read beyond `len` thanks to the Deref which slices to `..len`).
    #[inline]
    pub fn new() -> MoveList {
        MoveList { moves: [Move::NULL; MAX_MOVE_LIST], len: 0 }
    }

    /// Adds a move at the end of the list.
    ///
    /// The generator never produces more moves than `MAX_MOVE_LIST` (the
    /// theoretical legal maximum is 218). In development, a `debug_assert!`
    /// detects any overflow; in release, an overflow is silently ignored
    /// (impossible in practice) rather than panicking or writing out
    /// of bounds — consistent with the "never panic in production" policy.
    #[inline]
    pub fn push(&mut self, mv: Move) {
        debug_assert!(self.len < MAX_MOVE_LIST,
            "MoveList::push : capacité {} dépassée", MAX_MOVE_LIST);
        if self.len < MAX_MOVE_LIST {
            self.moves[self.len] = mv;
            self.len += 1;
        }
    }

    /// Keeps only the moves satisfying the predicate (equivalent to
    /// `Vec::retain`, in-place compaction in O(n)).
    #[inline]
    pub fn retain<F: FnMut(&Move) -> bool>(&mut self, mut keep: F) {
        let mut write = 0usize;
        for read in 0..self.len {
            if keep(&self.moves[read]) {
                self.moves[write] = self.moves[read];
                write += 1;
            }
        }
        self.len = write;
    }

    /// Empties the list (length reset to zero; the buffer is not cleared).
    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl Default for MoveList {
    #[inline]
    fn default() -> Self { Self::new() }
}

impl std::ops::Deref for MoveList {
    type Target = [Move];
    #[inline]
    fn deref(&self) -> &[Move] {
        &self.moves[..self.len]
    }
}

impl std::ops::DerefMut for MoveList {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Move] {
        &mut self.moves[..self.len]
    }
}

// =============================================================================
// Attack detection
// =============================================================================

/// Returns true if the square `sq` is attacked by the color `attacker`.
/// Checks all pieces of the attacking color.
pub fn is_square_attacked(board: &Board, sq: u8, attacker: Color) -> bool {
    let occupied = board.all_pieces;
    let sq_bb: Bitboard = 1u64 << sq;

    // --- Knight attack ---
    let knights = board.pieces[attacker.index()][Piece::Knight.index()];
    if knight_attacks(sq) & knights != 0 {
        return true;
    }

    // --- King attack ---
    let king = board.pieces[attacker.index()][Piece::King.index()];
    if king_attacks(sq) & king != 0 {
        return true;
    }

    // --- Pawn attack ---
    // Pawns attack diagonally. We check if an enemy pawn
    // can reach `sq` from its attack squares.
    let pawns = board.pieces[attacker.index()][Piece::Pawn.index()];
    let pawn_attacks = match attacker {
        Color::White => white_pawn_attacks(pawns),
        Color::Black => black_pawn_attacks(pawns),
    };
    if pawn_attacks & sq_bb != 0 {
        return true;
    }

    // --- Rook or queen attack (horizontal/vertical lines) ---
    let rooks_queens = board.pieces[attacker.index()][Piece::Rook.index()]
                     | board.pieces[attacker.index()][Piece::Queen.index()];
    if rook_attacks(sq, occupied) & rooks_queens != 0 {
        return true;
    }

    // --- Bishop or queen attack (diagonals) ---
    let bishops_queens = board.pieces[attacker.index()][Piece::Bishop.index()]
                       | board.pieces[attacker.index()][Piece::Queen.index()];
    if bishop_attacks(sq, occupied) & bishops_queens != 0 {
        return true;
    }

    false
}

/// Returns true if the king of the color `color` is in check.
pub fn is_in_check(board: &Board, color: Color) -> bool {
    let king_sq = board.king_square(color);
    is_square_attacked(board, king_sq, color.opposite())
}

// =============================================================================
// Move generation
// =============================================================================

/// Generates all pseudo-moves (may leave the king in check).
/// Called internally by generate_legal_moves().
fn generate_pseudo_moves(board: &Board, moves: &mut crate::moves::MoveList) {
    let color = board.side_to_move;
    pawn::generate_pawn_moves(board, color, moves);
    knight::generate_knight_moves(board, color, moves);
    bishop::generate_bishop_moves(board, color, moves);
    rook::generate_rook_moves(board, color, moves);
    queen::generate_queen_moves(board, color, moves);
    king::generate_king_moves(board, color, moves);
}

/// Generates all LEGAL moves of the current position.
/// Guarantee: no returned move leaves the king in check.
/// Guarantee: castling moves are validated (king not in check, squares crossed are safe).
pub fn generate_legal_moves(board: &mut Board) -> Vec<Move> {
    // Wrapper kept for callers OUTSIDE the hot path (perft, tuner,
    // extract_positions, benchmark, tests, UCI parsing) — it allocates a Vec, but
    // these usages are not critical for NPS. The engine (alpha_beta /
    // quiescence) uses generate_legal_moves_into() which allocates nothing.
    let mut list = MoveList::new();
    generate_legal_moves_into(board, &mut list);
    list.to_vec()
}

/// Zero-allocation version of generate_legal_moves(): fills the `MoveList`
/// provided by the caller (typically stack-allocated in the search).
/// The list is cleared at the start — its previous content is ignored.
pub fn generate_legal_moves_into(board: &mut Board, out: &mut MoveList) {
    out.clear();

    // Pseudo-moves generated on the stack (no heap allocation).
    let mut pseudo_moves = MoveList::new();
    generate_pseudo_moves(board, &mut pseudo_moves);

    // Legal filtering: fast path (pins) for the common case, make/unmake
    // for the tricky cases. See filter_legal_into().
    filter_legal_into(board, &pseudo_moves, out);
}

/// Checks if a castling move is legal:
/// - The king must not be in check at the start
/// - The squares crossed by the king must not be attacked
fn is_castling_legal(board: &Board, mv: &Move, color: Color) -> bool {
    let enemy = color.opposite();
    let king_sq = mv.from;

    // The king must not be in check at the start
    if is_square_attacked(board, king_sq, enemy) {
        return false;
    }

    // Check the squares crossed by the king
    match mv.flags {
        MoveFlags::CastleKingside => {
            // The king passes through f1/f8 (king_sq + 1) and arrives at g1/g8 (king_sq + 2)
            if is_square_attacked(board, king_sq + 1, enemy) { return false; }
            if is_square_attacked(board, king_sq + 2, enemy) { return false; }
        }
        MoveFlags::CastleQueenside => {
            // The king passes through d1/d8 (king_sq - 1) and arrives at c1/c8 (king_sq - 2)
            if is_square_attacked(board, king_sq - 1, enemy) { return false; }
            if is_square_attacked(board, king_sq - 2, enemy) { return false; }
        }
        _ => {}
    }

    true
}

// =============================================================================
// Legal filtering accelerated by pin detection
// =============================================================================

/// Bitboard of the pieces of `us` ABSOLUTELY pinned to their king (can only
/// move along the pin line without exposing the king).
///
/// PRECONDITION: the king of `us` is NOT in check (the fast path of
/// filter_legal_into() only calls this function in that case). Under this
/// precondition, the result is EXACT — neither false positive nor false negative:
///   - We start from the pieces of `us` that block a king line FIRST
///     (`rook_attacks`/`bishop_attacks` from the king stop at the 1st blocker).
///   - We remove each blocker and check whether an enemy sliding piece of the
///     right type (rook/queen on a line, bishop/queen on a diagonal) then attacks the king.
///     Since the king was NOT in check, no sliding piece was attacking before:
///     an attacker revealed by the removal can only come from that
///     blocker's line → the blocker is genuinely pinned.
///
/// The cost is a few magic lookups per node (one per piece blocking a
/// king line, typically 0 to 4), much cheaper than ~35 make/unmake.
fn pinned_pieces(board: &Board, us: Color) -> Bitboard {
    let king_sq = board.king_square(us);
    let them    = us.opposite();
    let occ     = board.all_pieces;
    let own     = board.occupancy[us.index()];

    let rook_sliders   = board.pieces[them.index()][Piece::Rook.index()]
                       | board.pieces[them.index()][Piece::Queen.index()];
    let bishop_sliders = board.pieces[them.index()][Piece::Bishop.index()]
                       | board.pieces[them.index()][Piece::Queen.index()];

    let mut pinned: Bitboard = 0;

    // Pins along straight lines (rooks / queens).
    let mut blockers = rook_attacks(king_sq, occ) & own;
    while blockers != 0 {
        let sq = pop_lsb(&mut blockers);
        if rook_attacks(king_sq, occ ^ (1u64 << sq)) & rook_sliders != 0 {
            pinned |= 1u64 << sq;
        }
    }

    // Pins along diagonals (bishops / queens).
    let mut blockers = bishop_attacks(king_sq, occ) & own;
    while blockers != 0 {
        let sq = pop_lsb(&mut blockers);
        if bishop_attacks(king_sq, occ ^ (1u64 << sq)) & bishop_sliders != 0 {
            pinned |= 1u64 << sq;
        }
    }

    pinned
}

/// Filters the `pseudo` pseudo-moves into LEGAL moves, added to `out`.
///
/// FAST PATH (zero make/unmake) for the common case — not in check, piece
/// neither pinned nor king, neither castling nor en passant capture: the move is legal by
/// construction. Justification: if the king is not in check, moving a
/// NON-pinned piece (other than the king) cannot expose its own king (only
/// removing a pinner could). The destination square is irrelevant
/// for a piece other than the king.
///
/// SAFE PATH (make / is_in_check / unmake) for all other cases: in check
/// (the king must respond), king move (may enter check), castling (additionally validated
/// by is_castling_legal — squares crossed), en passant capture (horizontal discovered
/// check after removal of the captured pawn, which a simple pin test on
/// the moving pawn does not detect), or pinned piece (legal only
/// along the line — checked by make/unmake).
///
/// SAFETY NET: in DEBUG build only, each fast-path decision is
/// re-checked against make/unmake. Any divergence (a pin that would
/// have been missed) makes perft / `cargo test` fail immediately, BEFORE
/// any real game. In release, the fast path performs no make/unmake.
fn filter_legal_into(board: &mut Board, pseudo: &MoveList, out: &mut MoveList) {
    let us       = board.side_to_move;
    let in_check = is_in_check(board, us);
    let king_sq  = board.king_square(us);
    let pinned   = if in_check { 0 } else { pinned_pieces(board, us) };

    for &mv in pseudo.iter() {
        let needs_full_check = in_check
            || mv.from == king_sq
            || mv.flags == MoveFlags::EnPassant
            || (pinned & (1u64 << mv.from)) != 0;

        if needs_full_check {
            // Castling: additional validation (king not in check, squares
            // crossed not attacked). The `from` of a castling move IS the king's square,
            // so this case is indeed captured by `mv.from == king_sq` above.
            if (mv.flags == MoveFlags::CastleKingside
                || mv.flags == MoveFlags::CastleQueenside)
                && !is_castling_legal(board, &mv, us)
            {
                continue;
            }
            board.make_move(mv);
            let legal = !is_in_check(board, us);
            board.unmake_move(mv);
            if legal {
                out.push(mv);
            }
        } else {
            // Fast path: legal move guaranteed by construction.
            #[cfg(debug_assertions)]
            {
                // Safety net (debug only): re-check that the move
                // is indeed legal. If this debug_assert triggers, pinned_pieces
                // missed a pin — bug to fix before any real game.
                board.make_move(mv);
                let really_legal = !is_in_check(board, us);
                board.unmake_move(mv);
                debug_assert!(
                    really_legal,
                    "FAST-PATH clouage manqué : coup {:?} jugé légal sans \
                     vérification mais laisse le roi en échec",
                    mv
                );
            }
            out.push(mv);
        }
    }
}

// =============================================================================
// Fast generation of captures (for quiescence search)
//
// Principle: generate ONLY pseudo-captures (captures, en passant,
// promotion-captures), then filter legality via make/unmake.
//
// Why this is critical:
//   The old implementation called generate_legal_moves() (make/unmake for
//   ~35 pseudo-moves on average) then silently discarded ~30 moves.
//   The new one only does make/unmake on ~5 pseudo-captures on average.
//   Since quiescence represents 80-90% of total nodes, this is the largest
//   possible gain on NPS.
//
// Captures generated (identical to the old implementation):
//   ✓ Normal captures of all pieces
//   ✓ En passant captures
//   ✓ Promotion-captures (×4 promotions)
//   ✗ Silent promotions (not included, as before)
//   ✗ Castling (never captures)
// =============================================================================

/// Generates the pseudo-captures for pawns of color `color`.
///
/// Includes: normal diagonal captures, promotion-captures, en passant captures.
/// Does NOT include single pushes, double pushes, or silent promotions.
///
/// The bitboard logic is a direct extraction from pawn.rs — only the
/// "capture" branches are kept, the "push" branches are removed.
fn generate_pawn_captures(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let pawns = board.pieces[color.index()][Piece::Pawn.index()];
    let enemy = board.occupancy[color.opposite().index()];

    match color {
        // -----------------------------------------------------------------------
        // WHITE pawns — advance toward increasing ranks (+8 per rank)
        // -----------------------------------------------------------------------
        Color::White => {
            // --- Captures toward the North-East (right diagonal) ---
            // A white pawn on sq captures on sq+9 if sq is not on the H file.
            let cap_ne       = ((pawns & !FILE_H) << 9) & enemy;
            let cap_ne_promo = cap_ne & RANK_8;   // Capture on the last rank → promotion
            let cap_ne_norm  = cap_ne & !RANK_8;

            let mut bb = cap_ne_norm;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to - 9;
                moves.push(Move::capture(from, to));
            }
            let mut bb = cap_ne_promo;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to - 9;
                // Always emit all 4 pieces — the GUI will choose
                moves.push(Move::promotion_capture(from, to, 4)); // Queen
                moves.push(Move::promotion_capture(from, to, 3)); // Rook
                moves.push(Move::promotion_capture(from, to, 2)); // Bishop
                moves.push(Move::promotion_capture(from, to, 1)); // Knight
            }

            // --- Captures toward the North-West (left diagonal) ---
            // A white pawn on sq captures on sq+7 if sq is not on the A file.
            let cap_nw       = ((pawns & !FILE_A) << 7) & enemy;
            let cap_nw_promo = cap_nw & RANK_8;
            let cap_nw_norm  = cap_nw & !RANK_8;

            let mut bb = cap_nw_norm;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to - 7;
                moves.push(Move::capture(from, to));
            }
            let mut bb = cap_nw_promo;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to - 7;
                moves.push(Move::promotion_capture(from, to, 4));
                moves.push(Move::promotion_capture(from, to, 3));
                moves.push(Move::promotion_capture(from, to, 2));
                moves.push(Move::promotion_capture(from, to, 1));
            }

            // --- White en passant capture ---
            // Exact copy of generate_white_pawn_moves() — same bitboard logic.
            if let Some(ep_sq) = board.en_passant {
                let ep_bb: Bitboard = 1u64 << ep_sq;
                // Look for white pawns able to reach ep_sq diagonally
                let ep_attackers =
                    (((ep_bb >> 9) & !FILE_H) | ((ep_bb >> 7) & !FILE_A)) & pawns;
                let mut bb = ep_attackers;
                while bb != 0 {
                    let from = pop_lsb(&mut bb);
                    moves.push(Move::en_passant(from, ep_sq));
                }
            }
        }

        // -----------------------------------------------------------------------
        // BLACK pawns — advance toward decreasing ranks (-8 per rank)
        // -----------------------------------------------------------------------
        Color::Black => {
            // --- Captures toward the South-East (right diagonal for black) ---
            // A black pawn on sq captures on sq-7 if sq is not on the H file.
            let cap_se       = ((pawns & !FILE_H) >> 7) & enemy;
            let cap_se_promo = cap_se & RANK_1;   // Capture on rank 1 → promotion
            let cap_se_norm  = cap_se & !RANK_1;

            let mut bb = cap_se_norm;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to + 7;
                moves.push(Move::capture(from, to));
            }
            let mut bb = cap_se_promo;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to + 7;
                moves.push(Move::promotion_capture(from, to, 4));
                moves.push(Move::promotion_capture(from, to, 3));
                moves.push(Move::promotion_capture(from, to, 2));
                moves.push(Move::promotion_capture(from, to, 1));
            }

            // --- Captures toward the South-West (left diagonal for black) ---
            // A black pawn on sq captures on sq-9 if sq is not on the A file.
            let cap_sw       = ((pawns & !FILE_A) >> 9) & enemy;
            let cap_sw_promo = cap_sw & RANK_1;
            let cap_sw_norm  = cap_sw & !RANK_1;

            let mut bb = cap_sw_norm;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to + 9;
                moves.push(Move::capture(from, to));
            }
            let mut bb = cap_sw_promo;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to + 9;
                moves.push(Move::promotion_capture(from, to, 4));
                moves.push(Move::promotion_capture(from, to, 3));
                moves.push(Move::promotion_capture(from, to, 2));
                moves.push(Move::promotion_capture(from, to, 1));
            }

            // --- Black en passant capture ---
            // Exact copy of generate_black_pawn_moves() — same bitboard logic.
            if let Some(ep_sq) = board.en_passant {
                let ep_bb: Bitboard = 1u64 << ep_sq;
                let ep_attackers =
                    (((ep_bb << 9) & !FILE_A) | ((ep_bb << 7) & !FILE_H)) & pawns;
                let mut bb = ep_attackers;
                while bb != 0 {
                    let from = pop_lsb(&mut bb);
                    moves.push(Move::en_passant(from, ep_sq));
                }
            }
        }
    }
}

/// Generates all pseudo-captures for the color to move.
///
/// For each piece type, the attacked squares are intersected directly
/// with the enemy bitboard (`& enemy`). This avoids generating the
/// silent moves and the `board.all_pieces & (1u64 << to)` test from the full version.
///
/// Castling is excluded: it can never be a capture.
/// Silent promotions are excluded: handled outside of quiescence.
fn generate_pseudo_captures(board: &Board, moves: &mut crate::moves::MoveList) {
    let color    = board.side_to_move;
    let enemy    = board.occupancy[color.opposite().index()];
    let occupied = board.all_pieces;

    // --- Pawns ---
    generate_pawn_captures(board, color, moves);

    // --- Knights ---
    let mut knights = board.pieces[color.index()][Piece::Knight.index()];
    while knights != 0 {
        let from     = pop_lsb(&mut knights);
        // Knight attacks intersected with enemy pieces only
        let mut caps = knight_attacks(from) & enemy;
        while caps != 0 {
            let to = pop_lsb(&mut caps);
            moves.push(Move::capture(from, to));
        }
    }

    // --- Bishops ---
    let mut bishops = board.pieces[color.index()][Piece::Bishop.index()];
    while bishops != 0 {
        let from     = pop_lsb(&mut bishops);
        let mut caps = bishop_attacks(from, occupied) & enemy;
        while caps != 0 {
            let to = pop_lsb(&mut caps);
            moves.push(Move::capture(from, to));
        }
    }

    // --- Rooks ---
    let mut rooks = board.pieces[color.index()][Piece::Rook.index()];
    while rooks != 0 {
        let from     = pop_lsb(&mut rooks);
        let mut caps = rook_attacks(from, occupied) & enemy;
        while caps != 0 {
            let to = pop_lsb(&mut caps);
            moves.push(Move::capture(from, to));
        }
    }

    // --- Queens ---
    let mut queens = board.pieces[color.index()][Piece::Queen.index()];
    while queens != 0 {
        let from     = pop_lsb(&mut queens);
        // The queen combines rook and bishop attacks
        let mut caps = queen_attacks(from, occupied) & enemy;
        while caps != 0 {
            let to = pop_lsb(&mut caps);
            moves.push(Move::capture(from, to));
        }
    }

    // --- King ---
    // The king cannot castle: castling is never a capture.
    let king_sq  = board.king_square(color);
    let mut caps = king_attacks(king_sq) & enemy;
    while caps != 0 {
        let to = pop_lsb(&mut caps);
        moves.push(Move::capture(king_sq, to));
    }
}

/// Generates all LEGAL capture moves (captures, en passant, promotion-captures).
///
/// Guarantees: no returned move leaves the king in check.
///
/// Complexity vs old implementation:
///   Before: generate_legal_moves() → make/unmake for ~35 pseudo-moves → filter
///   After: generate_pseudo_captures() → make/unmake for ~5 pseudo-captures → result
///
/// Since quiescence represents 80-90% of the total nodes in a search, this change
/// has the greatest possible impact on the engine's performance.
pub fn generate_legal_captures(board: &mut Board) -> Vec<Move> {
    // Wrapper allocating a Vec — kept for potential callers outside the hot
    // path. The engine uses generate_legal_captures_into() (zero allocation).
    let mut list = MoveList::new();
    generate_legal_captures_into(board, &mut list);
    list.to_vec()
}

/// Zero-allocation version of generate_legal_captures(): fills the provided
/// `MoveList` (typically on the stack). The list is cleared at the start.
pub fn generate_legal_captures_into(board: &mut Board, out: &mut MoveList) {
    out.clear();

    // Pseudo-captures generated on the stack.
    let mut pseudo = MoveList::new();
    generate_pseudo_captures(board, &mut pseudo);

    // Same accelerated legal filtering as for full moves (fast path
    // via pinned pieces + make/unmake for tricky cases — en passant included).
    filter_legal_into(board, &pseudo, out);
}

/// Returns true if the position is a stalemate (no legal moves, king not in check).
pub fn is_stalemate(board: &mut Board) -> bool {
    let moves = generate_legal_moves(board);
    moves.is_empty() && !is_in_check(board, board.side_to_move)
}

/// Returns true if the position is a checkmate.
pub fn is_checkmate(board: &mut Board) -> bool {
    let moves = generate_legal_moves(board);
    moves.is_empty() && is_in_check(board, board.side_to_move)
}

// =============================================================================
// Perft — Move generation test
//
// Perft (PERFormance Test) counts the number of leaf nodes at a given
// depth. The results are known and can be verified against reference
// tables to validate the correctness of move generation.
// =============================================================================

/// Counts the number of positions reachable at depth `depth`.
/// Reference results for the initial position:
///   depth 1 → 20
///   depth 2 → 400
///   depth 3 → 8902
///   depth 4 → 197281
///   depth 5 → 4865609
pub fn perft(board: &mut Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }

    let moves = generate_legal_moves(board);

    if depth == 1 {
        return moves.len() as u64;
    }

    let mut count = 0u64;
    for mv in moves {
        board.make_move(mv);
        count += perft(board, depth - 1);
        board.unmake_move(mv);
    }

    count
}

/// Version of perft that displays the breakdown per move (useful for debugging).
pub fn perft_divide(board: &mut Board, depth: u32) -> u64 {
    let moves = generate_legal_moves(board);
    let mut total = 0u64;

    for mv in moves {
        board.make_move(mv);
        let count = perft(board, depth - 1);
        board.unmake_move(mv);

        println!("{}: {}", mv.to_uci(), count);
        total += count;
    }

    println!("Total : {}", total);
    total
}

// =============================================================================
// Perft Tests — Move generation validation
//
// These tests compare the results of perft() against the reference values from the
// Chess Programming Wiki: https://www.chessprogramming.org/Perft_Results
//
// A discrepancy of 1 node, even at depth 3, reveals a specific bug in
// generation: illegal castling accepted, missed en passant, pin
// ignored, incorrect promotion, etc.
//
// Organization:
//   Fast tests  — depth ≤ 3, < 100 000 nodes → run by `cargo test`
//   Slow tests  — depth 4-5, millions of nodes → #[ignore], run with
//                    `cargo test -- --include-ignored` or via `cargo run --bin perft`
//
// Recommended usage:
//   1. `cargo test`                               : fast tests (a few seconds)
//   2. `cargo test -- --include-ignored`          : full suite (a few minutes in debug)
//   3. `cargo run --release --bin perft`          : optimized suite (< 30 seconds in release)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::state::Board;

    // =========================================================================
    // Position 1 — Initial position
    // rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1
    // Source: https://www.chessprogramming.org/Perft_Results
    // Covers: basic cases, all normal pieces
    // =========================================================================

    #[test]
    fn pos1_initiale_d1() {
        let mut b = Board::start_position();
        assert_eq!(perft(&mut b, 1), 20,
            "Position initiale d1 : attendu 20 coups (8 pions × 2 + 2 cavaliers × 2)");
    }

    #[test]
    fn pos1_initiale_d2() {
        let mut b = Board::start_position();
        assert_eq!(perft(&mut b, 2), 400, "Position initiale d2 : attendu 400");
    }

    #[test]
    fn pos1_initiale_d3() {
        let mut b = Board::start_position();
        assert_eq!(perft(&mut b, 3), 8_902, "Position initiale d3 : attendu 8 902");
    }

    #[test]
    #[ignore = "lent en debug (~200K noeuds) — utiliser --release ou --bin perft"]
    fn pos1_initiale_d4() {
        let mut b = Board::start_position();
        assert_eq!(perft(&mut b, 4), 197_281, "Position initiale d4 : attendu 197 281");
    }

    #[test]
    #[ignore = "lent en debug (~5M noeuds) — utiliser --release ou --bin perft"]
    fn pos1_initiale_d5() {
        let mut b = Board::start_position();
        assert_eq!(perft(&mut b, 5), 4_865_609, "Position initiale d5 : attendu 4 865 609");
    }

    // =========================================================================
    // Position 2 — Kiwipete
    // r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1
    // Covers: castling on both sides, en passant captures, promotions
    //          discovered checks, complex positions
    // =========================================================================

    #[test]
    fn pos2_kiwipete_d1() {
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 1), 48, "Kiwipete d1 : attendu 48");
    }

    #[test]
    fn pos2_kiwipete_d2() {
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 2), 2_039, "Kiwipete d2 : attendu 2 039");
    }

    #[test]
    #[ignore = "lent en debug (~100K noeuds) — utiliser --release ou --bin perft"]
    fn pos2_kiwipete_d3() {
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 3), 97_862, "Kiwipete d3 : attendu 97 862");
    }

    #[test]
    #[ignore = "lent en debug (~4M noeuds) — utiliser --release ou --bin perft"]
    fn pos2_kiwipete_d4() {
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 4), 4_085_603, "Kiwipete d4 : attendu 4 085 603");
    }

    // =========================================================================
    // Position 3 — Endgame with passed pawns
    // 8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1
    // Covers: en passant edge cases, multiple promotions, few pieces
    // =========================================================================

    #[test]
    fn pos3_finale_d1() {
        let fen = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 1), 14, "Pos3 d1 : attendu 14");
    }

    #[test]
    fn pos3_finale_d2() {
        let fen = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 2), 191, "Pos3 d2 : attendu 191");
    }

    #[test]
    fn pos3_finale_d3() {
        let fen = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 3), 2_812, "Pos3 d3 : attendu 2 812");
    }

    #[test]
    #[ignore = "lent en debug (~43K noeuds)"]
    fn pos3_finale_d4() {
        let fen = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 4), 43_238, "Pos3 d4 : attendu 43 238");
    }

    #[test]
    #[ignore = "lent en debug (~675K noeuds)"]
    fn pos3_finale_d5() {
        let fen = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 5), 674_624, "Pos3 d5 : attendu 674 624");
    }

    // =========================================================================
    // Position 4 — Promotions and minority castling
    // r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1
    // Covers: promotions of 7 white pawns in position, limited castling (kq only)
    // =========================================================================

    #[test]
    fn pos4_promotions_d1() {
        let fen = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 1), 6, "Pos4 d1 : attendu 6");
    }

    #[test]
    fn pos4_promotions_d2() {
        let fen = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 2), 264, "Pos4 d2 : attendu 264");
    }

    #[test]
    fn pos4_promotions_d3() {
        let fen = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 3), 9_467, "Pos4 d3 : attendu 9 467");
    }

    #[test]
    #[ignore = "lent en debug (~422K noeuds)"]
    fn pos4_promotions_d4() {
        let fen = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 4), 422_333, "Pos4 d4 : attendu 422 333");
    }

    // =========================================================================
    // Position 5 — En passant and promotion edge cases
    // rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8
    // Covers: double en passant capture, promotions with capture, king without castling
    // =========================================================================

    #[test]
    fn pos5_ep_promos_d1() {
        let fen = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 1), 44, "Pos5 d1 : attendu 44");
    }

    #[test]
    fn pos5_ep_promos_d2() {
        let fen = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 2), 1_486, "Pos5 d2 : attendu 1 486");
    }

    #[test]
    #[ignore = "lent en debug (~62K noeuds)"]
    fn pos5_ep_promos_d3() {
        let fen = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 3), 62_379, "Pos5 d3 : attendu 62 379");
    }

    #[test]
    #[ignore = "lent en debug (~2M noeuds)"]
    fn pos5_ep_promos_d4() {
        let fen = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 4), 2_103_487, "Pos5 d4 : attendu 2 103 487");
    }

    // =========================================================================
    // Position 6 — Balanced middlegame
    // r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10
    // Covers: open positions, fianchettoed bishops, no castling available
    // =========================================================================

    #[test]
    fn pos6_milieu_d1() {
        let fen = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 1), 46, "Pos6 d1 : attendu 46");
    }

    #[test]
    fn pos6_milieu_d2() {
        let fen = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 2), 2_079, "Pos6 d2 : attendu 2 079");
    }

    #[test]
    #[ignore = "lent en debug (~90K noeuds)"]
    fn pos6_milieu_d3() {
        let fen = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 3), 89_890, "Pos6 d3 : attendu 89 890");
    }

    #[test]
    #[ignore = "lent en debug (~4M noeuds)"]
    fn pos6_milieu_d4() {
        let fen = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 4), 3_894_594, "Pos6 d4 : attendu 3 894 594");
    }

    // =========================================================================
    // General legality tests
    // =========================================================================

    #[test]
    fn aucun_coup_legal_ne_laisse_le_roi_en_echec() {
        // Verify that no legal move leaves the king in check — initial position.
        let mut board = Board::start_position();
        let legal_moves = generate_legal_moves(&mut board);
        for mv in legal_moves {
            board.make_move(mv);
            assert!(!is_in_check(&board, Color::White),
                "Le coup {} laisse le roi blanc en échec !", mv.to_uci());
            board.unmake_move(mv);
        }
    }

    #[test]
    fn aucun_coup_legal_ne_laisse_le_roi_en_echec_kiwipete() {
        // Same verification on Kiwipete (more special cases).
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut board = Board::from_fen(fen).unwrap();
        let legal_moves = generate_legal_moves(&mut board);
        assert_eq!(legal_moves.len(), 48,
            "Kiwipete : attendu 48 coups légaux, obtenus {}", legal_moves.len());
        let color = board.side_to_move;
        for mv in legal_moves {
            board.make_move(mv);
            assert!(!is_in_check(&board, color),
                "Le coup {} laisse le roi en échec !", mv.to_uci());
            board.unmake_move(mv);
        }
    }
}
