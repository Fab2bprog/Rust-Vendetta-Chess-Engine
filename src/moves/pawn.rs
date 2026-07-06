// =============================================================================
// Vendetta Chess Motor — src/moves/pawn.rs
//
// Rôle : Génère tous les pseudo-coups légaux des pions pour une couleur donnée.
//        Les coups pseudo-légaux peuvent laisser le roi en échec — ils seront
//        filtrés dans moves/mod.rs.
//
// Contenu :
//   - Poussées simples (avance d'une case)
//   - Poussées doubles (avance de deux cases depuis la position initiale)
//   - Captures diagonales
//   - Prises en passant
//   - Promotions (poussée et capture vers le rang de promotion)
//
// Règles importantes :
//   - Un pion blanc avance vers les rangs croissants (+8 par rang)
//   - Un pion noir avance vers les rangs décroissants (-8 par rang)
//   - La poussée double n'est possible que depuis le rang initial
//     (rang 1 pour Blanc, rang 6 pour Noir)
//   - La prise en passant n'est possible que si la case cible en passant
//     est définie dans l'état du plateau
//   - Les promotions ont lieu sur le rang 7 (Blanc) ou rang 0 (Noir)
// =============================================================================

use crate::utils::types::{Color, Piece, Move};
use crate::board::state::Board;
use crate::board::bitboard::{
    Bitboard, pop_lsb,
    FILE_A, FILE_H, RANK_2, RANK_7,
};

/// Génère tous les pseudo-coups des pions de la couleur `color`.
/// Les coups sont ajoutés au vecteur `moves`.
pub fn generate_pawn_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    match color {
        Color::White => generate_white_pawn_moves(board, moves),
        Color::Black => generate_black_pawn_moves(board, moves),
    }
}

// =============================================================================
// Pions Blancs
// =============================================================================

/// Génère tous les pseudo-coups des pions blancs.
fn generate_white_pawn_moves(board: &Board, moves: &mut crate::moves::MoveList) {
    let pawns    = board.pieces[Color::White.index()][Piece::Pawn.index()];
    let enemy    = board.occupancy[Color::Black.index()];
    let empty    = !board.all_pieces;

    // --- Poussées simples ---
    // Un pion blanc avance d'une case vers le nord (+8) si la case est vide.
    let push1 = (pawns << 8) & empty;

    // Séparer les promotions (rang 8 = cases 56-63) des poussées normales.
    const RANK_8: u64 = 0xFF00_0000_0000_0000u64;
    let push1_promo  = push1 & RANK_8;
    let push1_normal = push1 & !RANK_8;

    // Poussées simples normales
    let mut bb = push1_normal;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to - 8;
        moves.push(Move::quiet(from, to));
    }

    // Promotions par poussée simple
    let mut bb = push1_promo;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to - 8;
        // Les 4 pièces de promotion : Dame, Tour, Fou, Cavalier
        moves.push(Move::promotion(from, to, 4)); // Dame
        moves.push(Move::promotion(from, to, 3)); // Tour
        moves.push(Move::promotion(from, to, 2)); // Fou
        moves.push(Move::promotion(from, to, 1)); // Cavalier
    }

    // --- Poussées doubles ---
    // Depuis le rang 2 (cases 8-15), si les deux cases devant sont libres.
    let push2 = ((pawns & RANK_2) << 8) & empty;
    let push2 = (push2 << 8) & empty;

    let mut bb = push2;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to - 16;
        moves.push(Move::double_push(from, to));
    }

    // --- Captures vers le Nord-Est ---
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

    // --- Captures vers le Nord-Ouest ---
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

    // --- Prise en passant ---
    if let Some(ep_sq) = board.en_passant {
        let ep_bb: Bitboard = 1u64 << ep_sq;
        // On travaille en sens inverse : depuis ep_sq, on cherche les pions blancs
        // qui peuvent y arriver.
        // Pion blanc venant du SW (ep_sq-9) : shift >>9 puis masquer FILE_H (wrap file A→H)
        // Pion blanc venant du SE (ep_sq-7) : shift >>7 puis masquer FILE_A (wrap file H→A)
        let ep_attackers = (((ep_bb >> 9) & !FILE_H) | ((ep_bb >> 7) & !FILE_A)) & pawns;
        let mut bb = ep_attackers;
        while bb != 0 {
            let from = pop_lsb(&mut bb);
            moves.push(Move::en_passant(from, ep_sq));
        }
    }
}

// =============================================================================
// Pions Noirs
// =============================================================================

/// Génère tous les pseudo-coups des pions noirs.
fn generate_black_pawn_moves(board: &Board, moves: &mut crate::moves::MoveList) {
    let pawns  = board.pieces[Color::Black.index()][Piece::Pawn.index()];
    let enemy  = board.occupancy[Color::White.index()];
    let empty  = !board.all_pieces;

    // --- Poussées simples ---
    // Un pion noir avance d'une case vers le sud (-8) si la case est vide.
    let push1       = (pawns >> 8) & empty;
    let push1_rank1 = push1 & 0x0000_0000_0000_00FFu64; // Rang 1 = promotion
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

    // --- Poussées doubles ---
    // Depuis le rang 7 (cases 48-55), si les deux cases devant (vers le bas) sont libres.
    let push2 = ((pawns & RANK_7) >> 8) & empty;
    let push2 = (push2 >> 8) & empty;

    let mut bb = push2;
    while bb != 0 {
        let to   = pop_lsb(&mut bb);
        let from = to + 16;
        moves.push(Move::double_push(from, to));
    }

    // --- Captures vers le Sud-Est ---
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

    // --- Captures vers le Sud-Ouest ---
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

    // --- Prise en passant ---
    if let Some(ep_sq) = board.en_passant {
        let ep_bb: Bitboard = 1u64 << ep_sq;
        // Pion noir venant du NW (ep_sq+9) : shift <<9 puis masquer FILE_A (wrap file H→A)
        // Pion noir venant du NE (ep_sq+7) : shift <<7 puis masquer FILE_H (wrap file A→H)
        let ep_attackers = (((ep_bb << 9) & !FILE_A) | ((ep_bb << 7) & !FILE_H)) & pawns;
        let mut bb = ep_attackers;
        while bb != 0 {
            let from = pop_lsb(&mut bb);
            moves.push(Move::en_passant(from, ep_sq));
        }
    }
}
