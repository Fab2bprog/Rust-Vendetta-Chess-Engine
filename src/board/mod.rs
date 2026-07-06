// =============================================================================
// Vendetta Chess Motor — src/board/mod.rs
//
// Rôle : Point d'entrée du module board. Réexporte les types et fonctions
//        essentiels pour que les autres modules puissent y accéder facilement.
//
// Sous-modules :
//   - bitboard : type Bitboard (u64) et toutes les opérations associées
//   - state    : structure Board, make/unmake move, FEN, hachage Zobrist
// =============================================================================

pub mod bitboard;
pub mod magic;
pub mod state;

// Réexportations pratiques pour simplifier les imports dans les autres modules.
pub use state::{Board, BoardState, CastlingRights, ZOBRIST};
pub use bitboard::{
    Bitboard, set_bit, clear_bit, get_bit, count_bits, lsb, pop_lsb,
    knight_attacks, king_attacks, rook_attacks, bishop_attacks, queen_attacks,
    white_pawn_attacks, black_pawn_attacks,
    init_attack_tables, FILE_A, FILE_H, RANK_1, RANK_2, RANK_7, RANK_8,
    file_mask, rank_mask,
};
