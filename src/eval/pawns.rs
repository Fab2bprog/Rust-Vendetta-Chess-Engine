// =============================================================================
// Vendetta Chess Motor — src/eval/pawns.rs
//
// Role: Pawn structure evaluation.
//        Pawn structure is fundamental in chess: it determines
//        strategic plans and permanent strengths/weaknesses.
//
// Contents:
//   - Detection of doubled pawns (two pawns on the same file)
//   - Detection of isolated pawns (no allied pawn on adjacent files)
//   - Detection of passed pawns (no enemy pawn can block them)
//   - Overall pawn structure score
//
// Penalties and bonuses (in centipawns):
//   - Doubled pawn : -20 (two pawns on the same file = weakness)
//   - Isolated pawn : -20 (no lateral protection = weakness)
//   - Passed pawn   : +20 to +50 depending on advancement (major strategic strength)
// =============================================================================

use std::sync::OnceLock;
use std::cell::RefCell;
use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::{Bitboard, file_mask, rank_mask};

/// Penalty for a doubled pawn (per extra pawn on the file).
/// Calibrated by Texel Tuning v3 (was -20) — see material.rs::PIECE_VALUE.
const DOUBLED_PAWN_PENALTY: i32 = -24;

/// Penalty for an isolated pawn (no friendly pawn on neighboring files).
/// Calibrated by Texel Tuning v3 (was -20).
const ISOLATED_PAWN_PENALTY: i32 = -19;

/// Bonus for a passed pawn, by rank of advancement (index = rank 0-7).
/// The more advanced the pawn, the more dangerous it is.
/// Calibrated by Texel Tuning v3 (was [0, 5, 10, 20, 35, 60, 100, 0]) —
/// strictly positive and increasing, unlike previous tuning attempts
/// without K calibration that had produced an inconsistent sign
/// at the first ranks (see material.rs::PIECE_VALUE for the full context).
const PASSED_PAWN_BONUS: [i32; 8] = [0, 7, 8, 33, 75, 138, 218, 0];

// =============================================================================
// Precomputed table of passed pawn masks
// =============================================================================

/// Returns the table `PASSED_PAWN_MASK[color][sq]`.
///
/// For each square `sq` and each color, the mask covers all ranks
/// "in front of" the pawn (depending on its color) on the pawn's file and the two
/// adjacent files. A pawn is passed if `enemy_pawns & mask == 0`.
///
/// Precomputed once via OnceLock (thread-safe, zero overhead afterward).
/// Replaces the O(8) loop `for r in (rank+1)..8 { mask |= rank_mask(r) & files; }`
/// with an O(1) lookup: a single array access instead of 8 bitboard OR + AND operations.
///
/// Indices:
///   [0][sq] = mask for a White pawn on square sq (higher ranks)
///   [1][sq] = mask for a Black pawn on square sq (lower ranks)
#[inline]
fn get_passed_pawn_mask() -> &'static [[Bitboard; 64]; 2] {
    static TABLE: OnceLock<[[Bitboard; 64]; 2]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [[0u64; 64]; 2];
        for sq in 0u8..64 {
            let file = sq % 8;
            let rank = sq / 8;
            let left   = if file > 0 { file_mask(file - 1) } else { 0 };
            let center = file_mask(file);
            let right  = if file < 7 { file_mask(file + 1) } else { 0 };
            let cols   = left | center | right;

            // White: ranks strictly above the pawn
            let mut wm = 0u64;
            for r in (rank + 1)..8 {
                wm |= rank_mask(r) & cols;
            }
            t[0][sq as usize] = wm;

            // Black: ranks strictly below the pawn
            let mut bm = 0u64;
            for r in 0..rank {
                bm |= rank_mask(r) & cols;
            }
            t[1][sq as usize] = bm;
        }
        t
    })
}

/// Evaluates the pawn structure for a given color.
/// Returns a score (positive = good for this color).
pub fn pawn_structure_score(board: &Board, color: Color) -> i32 {
    let pawns = board.pieces[color.index()][Piece::Pawn.index()];
    let enemy_pawns = board.pieces[color.opposite().index()][Piece::Pawn.index()];
    let mut score = 0i32;

    // For each file, count and analyze the pawns of this color
    for file in 0u8..8 {
        let col_mask = file_mask(file);
        let pawns_on_file = pawns & col_mask;
        let count = pawns_on_file.count_ones() as i32;

        if count == 0 { continue; }

        // --- Doubled pawns ---
        // If more than one pawn on the file, penalty for the extra pawns.
        if count > 1 {
            score += DOUBLED_PAWN_PENALTY * (count - 1);
        }

        // --- Isolated pawns ---
        // A pawn is isolated if there is no friendly pawn on neighboring files.
        let left_file  = if file > 0 { file_mask(file - 1) } else { 0 };
        let right_file = if file < 7 { file_mask(file + 1) } else { 0 };
        let adjacent   = left_file | right_file;

        if pawns & adjacent == 0 {
            // All pawns on this file are isolated
            score += ISOLATED_PAWN_PENALTY * count;
        }

        // --- Passed pawns ---
        // A pawn is passed if no enemy pawn is in front of it
        // on the same file or adjacent files.
        //
        // Optimization: O(1) lookup in PASSED_PAWN_MASK instead of the loop
        // O(8) `for r in (rank+1)..8 { mask |= rank_mask(r) & blocking_files; }`.
        // The table is initialized once (OnceLock), then a simple
        // array access replaces 8 bitboard operations per node.
        let ppm = get_passed_pawn_mask();
        let color_idx = color.index();
        let mut bb = pawns_on_file;
        while bb != 0 {
            let sq = bb.trailing_zeros() as u8;
            bb    &= bb - 1;

            // Precomputed mask: O(1), zero loop.
            if enemy_pawns & ppm[color_idx][sq as usize] == 0 {
                let rank = sq / 8;
                let advancement = match color {
                    Color::White => rank as usize,
                    Color::Black => (7 - rank) as usize,
                };
                score += PASSED_PAWN_BONUS[advancement];
            }
        }
    }

    score
}

// =============================================================================
// Pawn hash table — cache of the pawn structure evaluation
// =============================================================================
//
// Pawn structure evaluation (doubled/isolated/passed pawns) depends
// ONLY on the position of the pawns of both colors — never on the king or other
// pieces (verified: pawn_structure_score only reads the pawn bitboards).
// Now, pawns rarely move: the same structure reappears in a huge
// proportion of search nodes. The computed value is therefore cached,
// which avoids re-scanning 8 files × 2 colors (+ passed pawn
// detection) on every call to evaluate().
//
// CACHE KEY = the pair of pawn bitboards (white, black) itself,
// checked by EXACT comparison during lookup. Consequence: NO false
// match is possible (unlike a truncated Zobrist hash) — an
// index collision at worst causes a replacement (recomputation), never a
// wrong value. This approach requires NO modification to make_move /
// unmake_move / Board: everything is contained here.
//
// CACHED VALUE = WHITE-RELATIVE score (white − black), independent of
// the side to move. The orientation based on the side to move is applied AFTER the lookup,
// exactly as before — so the result is strictly identical (zero Elo).
//
// THREAD-LOCAL cache: each Lazy SMP thread has its own (no sharing, so
// no synchronization). Entries remain valid indefinitely (a
// given pawn structure always has the same score — the eval constants
// do not change at runtime), so there is never a need to clear the cache.

/// Number of cache entries (power of 2). 8192 × ~24 bytes ≈ 192 KiB per
/// thread — largely sufficient given the small number of distinct pawn structures
/// encountered in a search tree.
const PAWN_CACHE_SIZE: usize = 1 << 13;
const PAWN_CACHE_MASK: usize = PAWN_CACHE_SIZE - 1;

/// A pawn structure cache entry.
#[derive(Clone, Copy)]
struct PawnCacheEntry {
    /// Bitboard of white pawns (part of the exact key).
    white_pawns: u64,
    /// Bitboard of black pawns (part of the exact key).
    black_pawns: u64,
    /// Stored white-relative score (white − black).
    value:       i32,
    /// false = empty slot (never written); distinguishes a free slot from a real
    /// entry whose value is 0 (position with no pawns, for example).
    valid:       bool,
}

const EMPTY_PAWN_ENTRY: PawnCacheEntry = PawnCacheEntry {
    white_pawns: 0,
    black_pawns: 0,
    value:       0,
    valid:       false,
};

thread_local! {
    /// Thread-local cache (Vec allocated on the heap → no large temporary array
    /// on the stack at initialization).
    static PAWN_CACHE: RefCell<Vec<PawnCacheEntry>> =
        RefCell::new(vec![EMPTY_PAWN_ENTRY; PAWN_CACHE_SIZE]);
}

/// Mixes the two pawn bitboards into a well-distributed index (the exact
/// key remains the two bitboards themselves, verified at lookup).
#[inline]
fn pawn_cache_index(white_pawns: u64, black_pawns: u64) -> usize {
    let mut h = white_pawns.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= black_pawns.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    (h as usize) & PAWN_CACHE_MASK
}

/// Computes the pawn structure differential from the active player's point of view.
///
/// First goes through the thread-local cache (key = pawn bitboards). On a
/// miss, computes the white-relative score via pawn_structure_score() and stores it.
/// The result is strictly identical to a direct computation — only the cost changes.
pub fn pawn_eval(board: &Board) -> i32 {
    let white_pawns = board.pieces[Color::White.index()][Piece::Pawn.index()];
    let black_pawns = board.pieces[Color::Black.index()][Piece::Pawn.index()];

    let white_relative = PAWN_CACHE.with(|cache| {
        let mut c   = cache.borrow_mut();
        let idx     = pawn_cache_index(white_pawns, black_pawns);
        let entry   = c[idx];

        // Hit: exact same structure (comparison of the two full bitboards).
        if entry.valid
            && entry.white_pawns == white_pawns
            && entry.black_pawns == black_pawns
        {
            return entry.value;
        }

        // Miss: computation then storage (simple replacement in case of collision).
        let value = pawn_structure_score(board, Color::White)
                  - pawn_structure_score(board, Color::Black);
        c[idx] = PawnCacheEntry { white_pawns, black_pawns, value, valid: true };
        value
    });

    if board.side_to_move == Color::White { white_relative } else { -white_relative }
}
