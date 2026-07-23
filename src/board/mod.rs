// =============================================================================
// Vendetta Chess Motor — src/board/mod.rs
//
// Role: Entry point of the board module. Re-exports the types and functions
//        essential for other modules to easily access them.
//
// Submodules:
//   - bitboard : Bitboard type (u64) and all associated operations
//   - state    : Board structure, make/unmake move, FEN, Zobrist hashing
// =============================================================================

pub mod bitboard;
pub mod magic;
pub mod state;

// Convenient re-exports to simplify imports in other modules.
pub use state::{Board, BoardState, CastlingRights, ZOBRIST};
pub use bitboard::{
    Bitboard, set_bit, clear_bit, get_bit, count_bits, lsb, pop_lsb,
    knight_attacks, king_attacks, rook_attacks, bishop_attacks, queen_attacks,
    white_pawn_attacks, black_pawn_attacks,
    init_attack_tables, FILE_A, FILE_H, RANK_1, RANK_2, RANK_7, RANK_8,
    file_mask, rank_mask,
};
