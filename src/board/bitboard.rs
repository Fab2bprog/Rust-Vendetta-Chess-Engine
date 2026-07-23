// =============================================================================
// Vendetta Chess Engine — src/board/bitboard.rs
//
// Role: Defines the Bitboard type (u64) and all associated operations.
//        A bitboard is a 64-bit integer where each bit represents a square
//        of the chessboard. Bit i corresponds to square i (0=a1, 63=h8).
//
// Contents:
//   - Bitboard type and alias
//   - Bit manipulation functions (set, clear, get, pop, count, lsb)
//   - Precomputed file and rank masks
//   - Attack functions for sliding pieces (bishop, rook, queen)
//     via Magic Bitboards (O(1): one multiplication + one shift + one lookup)
//   - Precomputed attack tables for knight and king
//
// Technical choice: attacks for sliding pieces (rook, bishop, queen)
// use Magic Bitboards (magic.rs module). Tables precomputed at
// startup in < 10 ms, O(1) access during search.
// =============================================================================

use std::sync::OnceLock;
use super::magic::{init_magic_tables, rook_attacks_magic, bishop_attacks_magic};

/// Bitboard type: 64-bit integer representing a set of squares.
/// Bit i = 1 means that square i is in the set.
pub type Bitboard = u64;

// =============================================================================
// Basic bit operations
// =============================================================================

/// Sets the bit corresponding to square `sq`.
#[inline]
pub fn set_bit(bb: &mut Bitboard, sq: u8) {
    *bb |= 1u64 << sq;
}

/// Clears the bit corresponding to square `sq`.
#[inline]
pub fn clear_bit(bb: &mut Bitboard, sq: u8) {
    *bb &= !(1u64 << sq);
}

/// Returns true if the bit for square `sq` is set.
#[inline]
pub fn get_bit(bb: Bitboard, sq: u8) -> bool {
    (bb >> sq) & 1 == 1
}

/// Returns the number of set bits (popcount).
#[inline]
pub fn count_bits(bb: Bitboard) -> u32 {
    bb.count_ones()
}

/// Returns the index of the least significant bit (LSB).
/// Precondition: bb != 0.
#[inline]
pub fn lsb(bb: Bitboard) -> u8 {
    bb.trailing_zeros() as u8
}

/// Returns the index of the LSB and clears it in the bitboard.
/// Precondition: bb != 0.
#[inline]
pub fn pop_lsb(bb: &mut Bitboard) -> u8 {
    let sq = lsb(*bb);
    *bb &= *bb - 1;
    sq
}

// =============================================================================
// File and rank masks
// =============================================================================

/// Mask for file a (file 0).
pub const FILE_A: Bitboard = 0x0101_0101_0101_0101;
/// Mask for file b (file 1).
pub const FILE_B: Bitboard = 0x0202_0202_0202_0202;
/// Mask for file g (file 6).
pub const FILE_G: Bitboard = 0x4040_4040_4040_4040;
/// Mask for file h (file 7).
pub const FILE_H: Bitboard = 0x8080_8080_8080_8080;

/// Mask for rank 1 (rank 0).
pub const RANK_1: Bitboard = 0x0000_0000_0000_00FF;
/// Mask for rank 2 (rank 1).
pub const RANK_2: Bitboard = 0x0000_0000_0000_FF00;
/// Mask for rank 7 (rank 6).
pub const RANK_7: Bitboard = 0x00FF_0000_0000_0000;
/// Mask for rank 8 (rank 7).
pub const RANK_8: Bitboard = 0xFF00_0000_0000_0000;

/// Returns the mask for the given file (0=a, 7=h).
#[inline]
pub fn file_mask(file: u8) -> Bitboard {
    FILE_A << file
}

/// Returns the mask for the given rank (0=rank1, 7=rank8).
#[inline]
pub fn rank_mask(rank: u8) -> Bitboard {
    RANK_1 << (rank * 8)
}

// =============================================================================
// Precomputed attack tables for knight and king
// These tables are computed only once at startup (see init_attack_tables).
// =============================================================================

/// Precomputed attack tables — thread-safe via OnceLock.
/// Once initialized, they are read-only for all threads.
static KNIGHT_ATTACKS_TABLE: OnceLock<[Bitboard; 64]> = OnceLock::new();
static KING_ATTACKS_TABLE:   OnceLock<[Bitboard; 64]> = OnceLock::new();

/// Initializes all precomputed attack tables:
///   - Knight and king (simple OnceLock tables)
///   - Rook and bishop via Magic Bitboards (OnceLock tables in magic.rs)
///
/// Must be called only once at startup, before any threading.
/// All initializations are idempotent and thread-safe (OnceLock).
pub fn init_attack_tables() {
    KNIGHT_ATTACKS_TABLE.get_or_init(|| {
        let mut table = [0u64; 64];
        for sq in 0u8..64 {
            table[sq as usize] = compute_knight_attacks(sq);
        }
        table
    });
    KING_ATTACKS_TABLE.get_or_init(|| {
        let mut table = [0u64; 64];
        for sq in 0u8..64 {
            table[sq as usize] = compute_king_attacks(sq);
        }
        table
    });
    // Magic tables for sliding pieces (rook and bishop)
    init_magic_tables();
}

/// Computes the squares attacked by a knight on square `sq`.
fn compute_knight_attacks(sq: u8) -> Bitboard {
    let bb: Bitboard = 1u64 << sq;
    let mut attacks: Bitboard = 0;

    // The 8 possible knight moves, avoiding edge wraparound.
    // North-North-East: +17, not if file h
    attacks |= (bb << 17) & !FILE_A;
    // North-North-West: +15, not if file a
    attacks |= (bb << 15) & !FILE_H;
    // North-East-East: +10, not if file g or h
    attacks |= (bb << 10) & !(FILE_A | FILE_B);
    // North-West-West: +6, not if file a or b
    attacks |= (bb << 6)  & !(FILE_G | FILE_H);
    // South-South-East: -15, not if file h
    attacks |= (bb >> 15) & !FILE_A;
    // South-South-West: -17, not if file a
    attacks |= (bb >> 17) & !FILE_H;
    // South-East-East: -6, not if file g or h
    attacks |= (bb >> 6)  & !(FILE_A | FILE_B);
    // South-West-West: -10, not if file a or b
    attacks |= (bb >> 10) & !(FILE_G | FILE_H);

    attacks
}

/// Computes the squares attacked by a king on square `sq`.
fn compute_king_attacks(sq: u8) -> Bitboard {
    let bb: Bitboard = 1u64 << sq;
    let mut attacks: Bitboard = 0;

    // The 8 king directions, avoiding edge wraparound.
    attacks |= bb << 8;                      // North
    attacks |= bb >> 8;                      // South
    attacks |= (bb << 1) & !FILE_A;         // East
    attacks |= (bb >> 1) & !FILE_H;         // West
    attacks |= (bb << 9) & !FILE_A;         // North-East
    attacks |= (bb << 7) & !FILE_H;         // North-West
    attacks |= (bb >> 7) & !FILE_A;         // South-East
    attacks |= (bb >> 9) & !FILE_H;         // South-West

    attacks
}

/// Returns the bitboard of squares attacked by a knight on square `sq`.
/// Thread-safe: read-only from OnceLock initialized at startup.
/// Precondition: sq < 64 (guaranteed by pop_lsb / lsb called on a non-zero bitboard).
#[inline]
pub fn knight_attacks(sq: u8) -> Bitboard {
    debug_assert!(sq < 64, "knight_attacks : case invalide sq={} (doit être 0-63)", sq);
    KNIGHT_ATTACKS_TABLE.get()
        .expect("init_attack_tables() non appelée")[sq as usize]
}

/// Returns the bitboard of squares attacked by a king on square `sq`.
/// Thread-safe: read-only from OnceLock initialized at startup.
/// Precondition: sq < 64 (guaranteed by king_square, itself protected by from_fen).
#[inline]
pub fn king_attacks(sq: u8) -> Bitboard {
    debug_assert!(sq < 64, "king_attacks : case invalide sq={} (doit être 0-63)", sq);
    KING_ATTACKS_TABLE.get()
        .expect("init_attack_tables() non appelée")[sq as usize]
}

// =============================================================================
// Sliding piece attacks (classic loop-based approach)
//
// For each sliding piece, we explore each direction until encountering
// a blocking piece or the edge of the board. The blocking square is included
// (since it can be captured) but we stop there.
// =============================================================================

/// Returns the squares attacked by a rook on square `sq`
/// with the bitboard `occupied` representing all pieces present.
///
/// Magic Bitboards implementation: O(1) — multiplication + shift + lookup.
/// 5 to 20x faster than the classic loop-based version.
#[inline]
pub fn rook_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    rook_attacks_magic(sq, occupied)
}

/// Returns the squares attacked by a bishop on square `sq`
/// with the bitboard `occupied` representing all pieces present.
///
/// Magic Bitboards implementation: O(1) — multiplication + shift + lookup.
/// 5 to 20x faster than the classic loop-based version.
#[inline]
pub fn bishop_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    bishop_attacks_magic(sq, occupied)
}

/// Computes the squares attacked by a queen on square `sq`.
/// The queen combines the attacks of the rook and the bishop.
#[inline]
pub fn queen_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    rook_attacks(sq, occupied) | bishop_attacks(sq, occupied)
}

// =============================================================================
// Pawn attacks
// Pawns do not attack the way they advance, so they are handled separately.
// =============================================================================

/// Computes the squares attacked by white pawns (toward the top of the board).
#[inline]
pub fn white_pawn_attacks(pawns: Bitboard) -> Bitboard {
    ((pawns << 9) & !FILE_A) | ((pawns << 7) & !FILE_H)
}

/// Computes the squares attacked by black pawns (toward the bottom of the board).
#[inline]
pub fn black_pawn_attacks(pawns: Bitboard) -> Bitboard {
    ((pawns >> 7) & !FILE_A) | ((pawns >> 9) & !FILE_H)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_clear_get() {
        let mut bb: Bitboard = 0;
        set_bit(&mut bb, 0);
        assert!(get_bit(bb, 0));
        clear_bit(&mut bb, 0);
        assert!(!get_bit(bb, 0));
    }

    #[test]
    fn test_lsb_pop() {
        let mut bb: Bitboard = 0b1010;
        let sq = pop_lsb(&mut bb);
        assert_eq!(sq, 1);
        assert_eq!(bb, 0b1000);
    }

    #[test]
    fn test_cavalier_centre() {
        init_attack_tables();
        // A knight on e4 (sq=28) attacks 8 squares
        let attacks = knight_attacks(28);
        assert_eq!(count_bits(attacks), 8);
    }

    #[test]
    fn test_cavalier_coin() {
        init_attack_tables();
        // A knight on a1 (sq=0) attacks 2 squares
        let attacks = knight_attacks(0);
        assert_eq!(count_bits(attacks), 2);
    }

    #[test]
    fn test_tour_echiquier_vide() {
        // init MANDATORY: rook_attacks delegates to the magic bitboards, which
        // panic if init_magic_tables() (called by init_attack_tables())
        // has not run. Without this line, the test depended on the parallel
        // execution order of the tests (flaky, panic possible) — made
        // self-contained here, like the knight tests above.
        init_attack_tables();
        // A rook on e4 (sq=28) on an empty board attacks 14 squares
        let attacks = rook_attacks(28, 0);
        assert_eq!(count_bits(attacks), 14);
    }

    #[test]
    fn test_fou_echiquier_vide() {
        // init MANDATORY (same reason as test_tour_echiquier_vide).
        init_attack_tables();
        // A bishop on e4 (sq=28) on an empty board attacks 13 squares
        let attacks = bishop_attacks(28, 0);
        assert_eq!(count_bits(attacks), 13);
    }
}
