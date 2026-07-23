// =============================================================================
// Vendetta Chess Engine — src/board/magic.rs
//
// Role: Magic Bitboards for ultra-fast computation of sliding piece attacks
//        (rook and bishop). Replaces the loops in bitboard.rs with a
//        simple constant-time table lookup.
//
// Principle:
//   For a piece on square `sq` with occupancy `occ`:
//     1. Mask the relevant occupancy: occ & mask[sq]
//     2. Multiply by the magic number: masked × magic[sq]
//     3. Shift right: >> shift[sq]
//     4. Look up the table: table[sq × SIZE + index]
//
//   This formula, in a single multiplication, compresses the relevant bits
//   of the occupancy toward the high-order bits, producing a compact index.
//
// Magic numbers:
//   Found at startup by random trial (sparse xorshift64).
//   Convergence guaranteed: a good magic number exists for every square.
//   Typical duration: < 10 ms for the 128 squares (64 rooks + 64 bishops).
//
// Storage (flat tables, offset = sq × MAX_SIZE):
//   Rooks  : 64 × 4096 × 8 bytes = 2 MB  (max 12 mask bits → 2^12 = 4096)
//   Bishops: 64 × 512  × 8 bytes = 256 KB (max  9 mask bits → 2^9  =  512)
//   Total  : ~2.25 MB allocated on the heap, read-only afterward.
//
// Thread-safety:
//   OnceLock guarantees that initialization runs only once,
//   even if several threads call init_magic_tables() simultaneously.
// =============================================================================

use std::sync::OnceLock;

// =============================================================================
// Data structure
// =============================================================================

struct MagicTables {
    /// Relevant occupancy mask for each square (rook).
    /// The edges of the board are excluded because they do not change mobility.
    rook_masks:    [u64; 64],
    /// Relevant occupancy mask for each square (bishop).
    bishop_masks:  [u64; 64],
    /// Magic number per square (rook).
    rook_magics:   [u64; 64],
    /// Magic number per square (bishop).
    bishop_magics: [u64; 64],
    /// Shift = 64 − popcount(mask) per square (rook).
    rook_shifts:   [u32; 64],
    /// Shift = 64 − popcount(mask) per square (bishop).
    bishop_shifts: [u32; 64],
    /// Flat tables of rook attacks: index = sq × 4096 + magic_index.
    rook_table:    Vec<u64>,
    /// Flat tables of bishop attacks: index = sq × 512  + magic_index.
    bishop_table:  Vec<u64>,
}

/// Global tables, initialized only once at startup.
static MAGIC_TABLES: OnceLock<MagicTables> = OnceLock::new();

// =============================================================================
// Mask computation
// =============================================================================

/// Occupancy mask for a rook on `sq`.
///
/// Contains all squares on the same rank and the same file,
/// excluding edge squares (they do not block the rook's mobility).
/// The square `sq` itself is also excluded.
fn rook_mask(sq: u8) -> u64 {
    let rank = (sq / 8) as i32;
    let file = (sq % 8) as i32;
    let mut mask = 0u64;

    // Same rank: columns b–g only (a and h excluded as edges)
    for f in 1..7_i32 {
        if f != file {
            mask |= 1u64 << (rank * 8 + f);
        }
    }
    // Same file: ranks 2–7 only (1 and 8 excluded as edges)
    for r in 1..7_i32 {
        if r != rank {
            mask |= 1u64 << (r * 8 + file);
        }
    }
    mask
}

/// Occupancy mask for a bishop on `sq`.
///
/// Contains all squares on the 4 diagonals, excluding the edges.
fn bishop_mask(sq: u8) -> u64 {
    let rank = (sq / 8) as i32;
    let file = (sq % 8) as i32;
    let mut mask = 0u64;

    // 4 diagonal directions, strictly excluding the edges (r ∈ ]0,7[, f ∈ ]0,7[)
    for (dr, df) in [(1_i32, 1_i32), (1, -1), (-1, 1), (-1, -1)] {
        let (mut r, mut f) = (rank + dr, file + df);
        while r > 0 && r < 7 && f > 0 && f < 7 {
            mask |= 1u64 << (r * 8 + f);
            r += dr;
            f += df;
        }
    }
    mask
}

// =============================================================================
// Slow attack computation (used only during initialization)
// =============================================================================

/// Attacks of a rook on `sq` with occupancy `occ` — classic slow version.
/// Used only to populate the magic tables at startup.
fn slow_rook_attacks(sq: u8, occ: u64) -> u64 {
    let rank = (sq / 8) as i32;
    let file = (sq % 8) as i32;
    let mut attacks = 0u64;

    for (dr, df) in [(1_i32, 0_i32), (-1, 0), (0, 1), (0, -1)] {
        let (mut r, mut f) = (rank + dr, file + df);
        while (0..8).contains(&r) && (0..8).contains(&f) {
            let s = (r * 8 + f) as u8;
            attacks |= 1u64 << s;
            if occ & (1u64 << s) != 0 { break; }
            r += dr;
            f += df;
        }
    }
    attacks
}

/// Attacks of a bishop on `sq` with occupancy `occ` — classic slow version.
/// Used only to populate the magic tables at startup.
fn slow_bishop_attacks(sq: u8, occ: u64) -> u64 {
    let rank = (sq / 8) as i32;
    let file = (sq % 8) as i32;
    let mut attacks = 0u64;

    for (dr, df) in [(1_i32, 1_i32), (1, -1), (-1, 1), (-1, -1)] {
        let (mut r, mut f) = (rank + dr, file + df);
        while (0..8).contains(&r) && (0..8).contains(&f) {
            let s = (r * 8 + f) as u8;
            attacks |= 1u64 << s;
            if occ & (1u64 << s) != 0 { break; }
            r += dr;
            f += df;
        }
    }
    attacks
}

// =============================================================================
// Magic number search
// =============================================================================

/// xorshift64 pseudo-random number generator.
/// Fast, sufficient for magic number search.
#[inline]
fn xorshift64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Generates a sparse random number (few bits set to 1).
/// Good magic numbers tend to be sparse — this heuristic
/// speeds up convergence by a factor of 5 to 10 on average.
#[inline]
fn sparse_random(state: &mut u64) -> u64 {
    xorshift64(state) & xorshift64(state) & xorshift64(state)
}

/// Finds a valid magic number for square `sq`.
///
/// Algorithm:
///   1. Enumerate all 2^N subsets of the mask (carry-rippler).
///   2. For each magic candidate, check for the absence of collisions:
///      two different occupancies must produce different indices
///      (or the same index if their attacks are identical — constructive).
///   3. Start over with a new candidate until a valid magic number is found.
///
/// The seed is unique per square to diversify the search spaces.
fn find_magic(sq: u8, mask: u64, is_rook: bool) -> u64 {
    let bits  = mask.count_ones() as usize;
    let shift = (64 - bits) as u32;
    let n     = 1usize << bits;

    // Precompute all subsets of the mask and their corresponding attacks.
    // The carry-rippler enumerates the 2^bits subsets in descending order.
    let mut occs    = vec![0u64; n];
    let mut attacks = vec![0u64; n];

    let mut subset = mask;
    let mut i = 0usize;
    loop {
        occs[i] = subset;
        attacks[i] = if is_rook {
            slow_rook_attacks(sq, subset)
        } else {
            slow_bishop_attacks(sq, subset)
        };
        i += 1;
        if subset == 0 { break; }
        subset = (subset - 1) & mask; // Next subset (carry-rippler)
    }

    // Temporary table for collision checking.
    // An entry of 0 means "never visited" (attacks are always > 0).
    let mut used = vec![0u64; n];

    // Unique seed per square to diversify the search
    let mut rng = 0xDEADBEEFCAFEBABEu64
        ^ ((sq as u64).wrapping_mul(0x9E3779B97F4A7C15));

    // Safety counter: in theory, a valid magic number always exists
    // for the squares of an 8×8 board. In practice, convergence is < 10 ms
    // for the 128 squares (rooks + bishops). If this bound is reached, it indicates
    // a bug in the mask generation (mask == 0, incorrect bits, etc.).
    // Chosen value: 100 million >> far greater than the worst cases observed
    // (~10,000 attempts for difficult squares), with no risk of a false positive.
    const MAX_MAGIC_ATTEMPTS: u64 = 100_000_000;
    let mut attempts: u64 = 0;

    'outer: loop {
        attempts += 1;
        if attempts > MAX_MAGIC_ATTEMPTS {
            panic!(
                "find_magic : impossible de trouver un nombre magique valide pour la case {} \
                 après {} tentatives. mask=0x{:016X}, is_rook={}. \
                 Cela indique un bug dans la génération du masque.",
                sq, MAX_MAGIC_ATTEMPTS, mask, is_rook
            );
        }

        let magic = sparse_random(&mut rng);

        // Quick filter: a good magic number must "disperse" the mask's bits.
        // Classic heuristic: at least 6 bits set to 1 in the 8 high-order bits
        // of the product (mask × magic). Rejects ~80% of bad candidates upfront.
        if (mask.wrapping_mul(magic) >> 56).count_ones() < 6 {
            continue;
        }

        // Reset the verification table
        used.fill(0);

        // Check for the absence of collisions across all subsets
        for j in 0..n {
            let idx = ((occs[j].wrapping_mul(magic)) >> shift) as usize;

            if used[idx] == 0 {
                // First time this index is used: record the attack
                used[idx] = attacks[j];
            } else if used[idx] != attacks[j] {
                // Destructive collision: two different attacks on the same index
                continue 'outer;
            }
            // Constructive collision (used[idx] == attacks[j]): acceptable
        }

        // No destructive collision → valid magic number found
        return magic;
    }
}

// =============================================================================
// Initialization (called only once at startup)
// =============================================================================

/// Initializes the magic tables for the 64 squares, rooks and bishops.
///
/// This function is called from `init_attack_tables()` in bitboard.rs.
/// It is idempotent and thread-safe (OnceLock).
/// Typical duration: < 10 ms on a modern CPU.
pub fn init_magic_tables() {
    MAGIC_TABLES.get_or_init(|| {
        let mut rook_masks    = [0u64; 64];
        let mut bishop_masks  = [0u64; 64];
        let mut rook_magics   = [0u64; 64];
        let mut bishop_magics = [0u64; 64];
        let mut rook_shifts   = [0u32; 64];
        let mut bishop_shifts = [0u32; 64];

        // Flat tables allocated on the heap to avoid any stack overflow.
        // Access offset: sq × MAX_SIZE + magic_index
        let mut rook_table   = vec![0u64; 64 * 4096]; // 2 MB
        let mut bishop_table = vec![0u64; 64 * 512];  // 256 KB

        for sq in 0u8..64 {
            // ----------------------------------------------------------------
            // Rook
            // ----------------------------------------------------------------
            let r_mask  = rook_mask(sq);
            let r_magic = find_magic(sq, r_mask, true);
            let r_shift = 64 - r_mask.count_ones();

            rook_masks[sq as usize]  = r_mask;
            rook_magics[sq as usize] = r_magic;
            rook_shifts[sq as usize] = r_shift;

            // Populate the table: for each subset of r_mask, compute
            // the magic index and store the corresponding attacks.
            let base = sq as usize * 4096;
            let mut subset = r_mask;
            loop {
                let idx = ((subset.wrapping_mul(r_magic)) >> r_shift) as usize;
                rook_table[base + idx] = slow_rook_attacks(sq, subset);
                if subset == 0 { break; }
                subset = (subset - 1) & r_mask;
            }

            // ----------------------------------------------------------------
            // Bishop
            // ----------------------------------------------------------------
            let b_mask  = bishop_mask(sq);
            let b_magic = find_magic(sq, b_mask, false);
            let b_shift = 64 - b_mask.count_ones();

            bishop_masks[sq as usize]  = b_mask;
            bishop_magics[sq as usize] = b_magic;
            bishop_shifts[sq as usize] = b_shift;

            let base = sq as usize * 512;
            let mut subset = b_mask;
            loop {
                let idx = ((subset.wrapping_mul(b_magic)) >> b_shift) as usize;
                bishop_table[base + idx] = slow_bishop_attacks(sq, subset);
                if subset == 0 { break; }
                subset = (subset - 1) & b_mask;
            }
        }

        MagicTables {
            rook_masks,
            bishop_masks,
            rook_magics,
            bishop_magics,
            rook_shifts,
            bishop_shifts,
            rook_table,
            bishop_table,
        }
    });
}

// =============================================================================
// Public attack functions
// =============================================================================

/// Returns the squares attacked by a rook on `sq` with occupancy `occ`.
///
/// Magic Bitboards version: O(1) — one multiplication, one shift, one lookup.
#[inline]
pub fn rook_attacks_magic(sq: u8, occ: u64) -> u64 {
    let t     = MAGIC_TABLES.get()
        .expect("init_magic_tables() non appelée avant rook_attacks_magic()");
    let mask  = t.rook_masks[sq as usize];
    let magic = t.rook_magics[sq as usize];
    let shift = t.rook_shifts[sq as usize];
    let idx   = ((occ & mask).wrapping_mul(magic) >> shift) as usize;
    t.rook_table[sq as usize * 4096 + idx]
}

/// Returns the squares attacked by a bishop on `sq` with occupancy `occ`.
///
/// Magic Bitboards version: O(1) — one multiplication, one shift, one lookup.
#[inline]
pub fn bishop_attacks_magic(sq: u8, occ: u64) -> u64 {
    let t     = MAGIC_TABLES.get()
        .expect("init_magic_tables() non appelée avant bishop_attacks_magic()");
    let mask  = t.bishop_masks[sq as usize];
    let magic = t.bishop_magics[sq as usize];
    let shift = t.bishop_shifts[sq as usize];
    let idx   = ((occ & mask).wrapping_mul(magic) >> shift) as usize;
    t.bishop_table[sq as usize * 512 + idx]
}
