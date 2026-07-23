// =============================================================================
// Vendetta Chess Engine — src/eval/threats.rs
//
// Role: Static threat detection — penalizes a piece attacked by an
//        opposing piece of lesser value, or an undefended piece attacked
//        ("hanging piece").
//
//        Independent of tactical search: helps the evaluation recognize
//        a real vulnerability even when quiescence search has not yet (or
//        will never, at the current depth) explored the capture that would
//        punish it. Reduces positions where the engine underestimates a
//        piece in danger simply because the exact tactical line is
//        beyond the reach of the search at that moment.
//
// Principle (two distinct signals, stackable):
//   1. "Threatened by a cheaper piece" — a Knight/Bishop attacked by a
//      pawn, a Rook attacked by a pawn or a minor piece, a Queen
//      attacked by anything cheaper. In these cases, the SEE
//      almost always favors the attacker: regardless of the defense in
//      terms of square control, the exchange remains bad for the threatened side.
//   2. "Hanging" — a piece (other than pawn and king) attacked and that NO
//      friendly piece can recapture on its square. A more general signal, which
//      captures isolated pieces even when the attacker is not cheaper.
//
// Deliberate simplification — NOT a full SEE here:
//   A full SEE per piece at EVERY call to evaluate() would be far too
//   costly (evaluate() is called at every leaf node). We settle
//   for a cheap square-control check (attack bitboards already needed
//   elsewhere), consistent with the level of simplicity of mobility.rs,
//   king_safety.rs and center.rs. The penalties are deliberately modest
//   (an evaluative "nudge", not a re-estimation of material): the tactical
//   search (SEE, quiescence) remains responsible for the actual accuracy
//   when it can see the line.
// =============================================================================

use crate::board::state::Board;
use crate::board::bitboard::{
    knight_attacks, bishop_attacks, rook_attacks, queen_attacks, king_attacks,
    white_pawn_attacks, black_pawn_attacks,
};
use crate::utils::types::{Color, Piece};

/// Penalty for a piece attacked by an opposing piece of lesser value
/// (regardless of any defense). Reasonable initial value — like the
/// rest of the positional evaluation, to be refined via Texel Tuning (possible
/// v5, see CLAUDE.md "Future work").
const THREATENED_BY_LESSER_PENALTY: i32 = 25;

/// Additional penalty for a piece attacked and totally un
/// defended ("hanging"). Stackable with the penalty above if both
/// conditions are met (e.g.: Rook attacked by a Knight AND not
/// defended otherwise).
const HANGING_PENALTY: i32 = 20;

/// Bitboard of squares attacked by ALL pieces of `color` (pawns
/// included). Serves as a cheap proxy for "defense": if `color` attacks a
/// square, it can recapture there if one of its pieces is captured there.
///
/// Deliberately WITHOUT masking squares occupied by `color`'s own
/// pieces (unlike mobility.rs, which excludes `!own_pieces` to only
/// count squares that are actually accessible): here, a square occupied by
/// a friendly piece AND attacked by another friendly piece is precisely what
/// defines a "defended" square.
fn own_attack_bitboard(board: &Board, color: Color) -> u64 {
    let occupied = board.all_pieces;
    let mut attacks = 0u64;

    let pawns = board.pieces[color.index()][Piece::Pawn.index()];
    attacks |= if color == Color::White {
        white_pawn_attacks(pawns)
    } else {
        black_pawn_attacks(pawns)
    };

    let mut bb = board.pieces[color.index()][Piece::Knight.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        attacks |= knight_attacks(sq);
    }

    let mut bb = board.pieces[color.index()][Piece::Bishop.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        attacks |= bishop_attacks(sq, occupied);
    }

    let mut bb = board.pieces[color.index()][Piece::Rook.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        attacks |= rook_attacks(sq, occupied);
    }

    let mut bb = board.pieces[color.index()][Piece::Queen.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        attacks |= queen_attacks(sq, occupied);
    }

    let king_bb = board.pieces[color.index()][Piece::King.index()];
    if king_bb != 0 {
        attacks |= king_attacks(king_bb.trailing_zeros() as u8);
    }

    attacks
}

/// Evaluates the threats faced by `color` (always a penalty, never a
/// bonus — being threatened is never positive). Returns a negative or
/// zero score, from `color`'s point of view.
pub fn threats_score(board: &Board, color: Color) -> i32 {
    let enemy    = color.opposite();
    let occupied = board.all_pieces;

    // --- Opposing attacks, BY piece TYPE (to compare values) ---
    let enemy_pawns = board.pieces[enemy.index()][Piece::Pawn.index()];
    let enemy_pawn_attacks = if enemy == Color::White {
        white_pawn_attacks(enemy_pawns)
    } else {
        black_pawn_attacks(enemy_pawns)
    };

    let mut enemy_minor_attacks = 0u64; // Opposing Knights + Bishops combined
    let mut bb = board.pieces[enemy.index()][Piece::Knight.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        enemy_minor_attacks |= knight_attacks(sq);
    }
    let mut bb = board.pieces[enemy.index()][Piece::Bishop.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        enemy_minor_attacks |= bishop_attacks(sq, occupied);
    }

    let mut enemy_rook_attacks = 0u64;
    let mut bb = board.pieces[enemy.index()][Piece::Rook.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        enemy_rook_attacks |= rook_attacks(sq, occupied);
    }

    let mut enemy_queen_attacks = 0u64;
    let mut bb = board.pieces[enemy.index()][Piece::Queen.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        enemy_queen_attacks |= queen_attacks(sq, occupied);
    }

    let enemy_king_bb = board.pieces[enemy.index()][Piece::King.index()];
    let enemy_king_attacks = if enemy_king_bb != 0 {
        king_attacks(enemy_king_bb.trailing_zeros() as u8)
    } else {
        0
    };

    let enemy_all_attacks = enemy_pawn_attacks | enemy_minor_attacks
        | enemy_rook_attacks | enemy_queen_attacks | enemy_king_attacks;

    // --- Squares defended by MY own side ---
    let my_defended = own_attack_bitboard(board, color);

    let mut score = 0i32;

    // --- Signal 1: threatened by a piece of lesser value ---

    // Knights/Bishops threatened by an opposing pawn.
    let my_minors = board.pieces[color.index()][Piece::Knight.index()]
        | board.pieces[color.index()][Piece::Bishop.index()];
    score -= (my_minors & enemy_pawn_attacks).count_ones() as i32 * THREATENED_BY_LESSER_PENALTY;

    // Rooks threatened by an opposing pawn or minor piece.
    let my_rooks = board.pieces[color.index()][Piece::Rook.index()];
    let rook_threats = enemy_pawn_attacks | enemy_minor_attacks;
    score -= (my_rooks & rook_threats).count_ones() as i32 * THREATENED_BY_LESSER_PENALTY;

    // Queen threatened by any piece cheaper than it.
    let my_queens = board.pieces[color.index()][Piece::Queen.index()];
    let queen_threats = enemy_pawn_attacks | enemy_minor_attacks | enemy_rook_attacks;
    score -= (my_queens & queen_threats).count_ones() as i32 * THREATENED_BY_LESSER_PENALTY;

    // --- Signal 2: hanging piece (attacked AND undefended), excluding pawn/king ---
    let my_pieces_no_king_pawn = board.occupancy[color.index()]
        & !board.pieces[color.index()][Piece::Pawn.index()]
        & !board.pieces[color.index()][Piece::King.index()];
    let hanging = my_pieces_no_king_pawn & enemy_all_attacks & !my_defended;
    score -= hanging.count_ones() as i32 * HANGING_PENALTY;

    score
}

/// Computes the threat differential from the active player's point of view.
/// Positive score = advantage for the active player (so here, almost always
/// "the opponent is more threatened than I am" — both components are
/// penalties, never bonuses).
pub fn threats_eval(board: &Board) -> i32 {
    let white_score = threats_score(board, Color::White);
    let black_score = threats_score(board, Color::Black);
    let diff = white_score - black_score;

    if board.side_to_move == Color::White { diff } else { -diff }
}
