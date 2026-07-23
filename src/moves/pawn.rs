// =============================================================================
// Vendetta Chess Engine — src/moves/pawn.rs
//
// Role: Generates all pseudo-legal pawn moves for a given color.
//        Pseudo-legal moves may leave the king in check — they will be
//        filtered in moves/mod.rs.
//
// Contents:
//   - Single pushes (advance by one square)
//   - Double pushes (advance by two squares from the initial position)
//   - Diagonal captures
//   - En passant captures
//   - Promotions (push and capture to the promotion rank)
//
// Important rules:
//   - A white pawn advances toward increasing ranks (+8 per rank)
//   - A black pawn advances toward decreasing ranks (-8 per rank)
//   - The double push is only possible from the initial rank
//     (rank 1 for White, rank 6 for Black)
//   - En passant capture is only possible if the en passant target square
//     is defined in the board state
//   - Promotions occur on rank 7 (White) or rank 0 (Black)
// =============================================================================

use crate::utils::types::{Color, Piece, Move};
use crate::board::state::Board;
use crate::board::bitboard::{
    Bitboard, pop_lsb,
    FILE_A, FILE_H, RANK_2, RANK_7,
};

/// Generates all pseudo-moves for the pawns of the `color` color.
/// The moves are added to the `moves` vector.
pub fn generate_pawn_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    match color {
        Color::White => generate_white_pawn_moves(board, moves),
        Color::Black => generate_black_pawn_moves(board, moves),
    }
}

// =============================================================================
// White Pawns
// =============================================================================

/// Generates all pseudo-moves for the white pawns.
fn generate_white_pawn_moves(board: &Board, moves: &mut crate::moves::MoveList) {
    let pawns    = board.pieces[Color::White.index()][Piece::Pawn.index()];
    let enemy    = board.occupancy[Color::Black.index()];
    let empty    = !board.all_pieces;

    // --- Single pushes ---
    // A white pawn advances one square north (+8) if the square is empty.
    let push1 = (pawns << 8) & empty;

    // Separate promotions (rank 8 = squares 56-63) from normal pushes.
    const RANK_8: u64 = 0xFF00_0000_0000_0000u64;
    let push1_promo  = push1 & RANK_8;
    let push1_normal = push1 & !RANK_8;

    // Normal single pushes
    let mut bb = push1_normal;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to - 8;
        moves.push(Move::quiet(from, to));
    }

    // Promotions by single push
    let mut bb = push1_promo;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to - 8;
        // The 4 promotion pieces: Queen, Rook, Bishop, Knight
        moves.push(Move::promotion(from, to, 4)); // Queen
        moves.push(Move::promotion(from, to, 3)); // Rook
        moves.push(Move::promotion(from, to, 2)); // Bishop
        moves.push(Move::promotion(from, to, 1)); // Knight
    }

    // --- Double pushes ---
    // From rank 2 (squares 8-15), if both squares ahead are free.
    let push2 = ((pawns & RANK_2) << 8) & empty;
    let push2 = (push2 << 8) & empty;

    let mut bb = push2;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to - 16;
        moves.push(Move::double_push(from, to));
    }

    // --- Captures to the North-East ---
    let captures_ne = ((pawns & !FILE_H) << 9) & enemy;

    let cap_ne_rank8 = captures_ne & 0xFF00_0000_0000_0000u64;
    let cap_ne_normal = captures_ne & !cap_ne_rank8;

    let mut bb = cap_ne_normal;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to - 9;
        moves.push(Move::capture(from, to));
    }

    let mut bb = cap_ne_rank8;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to - 9;
        moves.push(Move::promotion_capture(from, to, 4));
        moves.push(Move::promotion_capture(from, to, 3));
        moves.push(Move::promotion_capture(from, to, 2));
        moves.push(Move::promotion_capture(from, to, 1));
    }

    // --- Captures to the North-West ---
    let captures_nw = ((pawns & !FILE_A) << 7) & enemy;

    let cap_nw_rank8 = captures_nw & 0xFF00_0000_0000_0000u64;
    let cap_nw_normal = captures_nw & !cap_nw_rank8;

    let mut bb = cap_nw_normal;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to - 7;
        moves.push(Move::capture(from, to));
    }

    let mut bb = cap_nw_rank8;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to - 7;
        moves.push(Move::promotion_capture(from, to, 4));
        moves.push(Move::promotion_capture(from, to, 3));
        moves.push(Move::promotion_capture(from, to, 2));
        moves.push(Move::promotion_capture(from, to, 1));
    }

    // --- En passant capture ---
    if let Some(ep_sq) = board.en_passant {
        let ep_bb: Bitboard = 1u64 << ep_sq;
        // We work in reverse: from ep_sq, we look for the white pawns
        // that can reach it.
        // White pawn coming from the SW (ep_sq-9): shift >>9 then mask FILE_H (wrap file A→H)
        // White pawn coming from the SE (ep_sq-7): shift >>7 then mask FILE_A (wrap file H→A)
        let ep_attackers = (((ep_bb >> 9) & !FILE_H) | ((ep_bb >> 7) & !FILE_A)) & pawns;
        let mut bb = ep_attackers;
        while bb != 0 {
            let from = pop_lsb(&mut bb);
            moves.push(Move::en_passant(from, ep_sq));
        }
    }
}

// =============================================================================
// Black Pawns
// =============================================================================

/// Generates all pseudo-moves for the black pawns.
fn generate_black_pawn_moves(board: &Board, moves: &mut crate::moves::MoveList) {
    let pawns  = board.pieces[Color::Black.index()][Piece::Pawn.index()];
    let enemy  = board.occupancy[Color::White.index()];
    let empty  = !board.all_pieces;

    // --- Single pushes ---
    // A black pawn advances one square south (-8) if the square is empty.
    let push1       = (pawns >> 8) & empty;
    let push1_rank1 = push1 & 0x0000_0000_0000_00FFu64; // Rank 1 = promotion
    let push1_normal = push1 & !push1_rank1;

    let mut bb = push1_normal;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to + 8;
        moves.push(Move::quiet(from, to));
    }

    let mut bb = push1_rank1;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to + 8;
        moves.push(Move::promotion(from, to, 4));
        moves.push(Move::promotion(from, to, 3));
        moves.push(Move::promotion(from, to, 2));
        moves.push(Move::promotion(from, to, 1));
    }

    // --- Double pushes ---
    // From rank 7 (squares 48-55), if both squares ahead (downward) are free.
    let push2 = ((pawns & RANK_7) >> 8) & empty;
    let push2 = (push2 >> 8) & empty;

    let mut bb = push2;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to + 16;
        moves.push(Move::double_push(from, to));
    }

    // --- Captures to the South-East ---
    let captures_se = ((pawns & !FILE_H) >> 7) & enemy;
    let cap_se_rank1  = captures_se & 0x0000_0000_0000_00FFu64;
    let cap_se_normal = captures_se & !cap_se_rank1;

    let mut bb = cap_se_normal;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to + 7;
        moves.push(Move::capture(from, to));
    }

    let mut bb = cap_se_rank1;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to + 7;
        moves.push(Move::promotion_capture(from, to, 4));
        moves.push(Move::promotion_capture(from, to, 3));
        moves.push(Move::promotion_capture(from, to, 2));
        moves.push(Move::promotion_capture(from, to, 1));
    }

    // --- Captures to the South-West ---
    let captures_sw = ((pawns & !FILE_A) >> 9) & enemy;
    let cap_sw_rank1  = captures_sw & 0x0000_0000_0000_00FFu64;
    let cap_sw_normal = captures_sw & !cap_sw_rank1;

    let mut bb = cap_sw_normal;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to + 9;
        moves.push(Move::capture(from, to));
    }

    let mut bb = cap_sw_rank1;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to + 9;
        moves.push(Move::promotion_capture(from, to, 4));
        moves.push(Move::promotion_capture(from, to, 3));
        moves.push(Move::promotion_capture(from, to, 2));
        moves.push(Move::promotion_capture(from, to, 1));
    }

    // --- En passant capture ---
    if let Some(ep_sq) = board.en_passant {
        let ep_bb: Bitboard = 1u64 << ep_sq;
        // Black pawn coming from the NW (ep_sq+9): shift <<9 then mask FILE_A (wrap file H→A)
        // Black pawn coming from the NE (ep_sq+7): shift <<7 then mask FILE_H (wrap file A→H)
        let ep_attackers = (((ep_bb << 9) & !FILE_A) | ((ep_bb << 7) & !FILE_H)) & pawns;
        let mut bb = ep_attackers;
        while bb != 0 {
            let from = pop_lsb(&mut bb);
            moves.push(Move::en_passant(from, ep_sq));
        }
    }
}
