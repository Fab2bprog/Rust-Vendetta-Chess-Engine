// =============================================================================
// Vendetta Chess Engine — src/search/transposition.rs
//
// Role: Thread-safe, lock-free transposition table (TT).
//        Cache of already-analyzed positions. Shared across all threads
//        Lazy SMP via Arc<TranspositionTable>.
//
// Multi-thread architecture:
//   Each entry is stored as two AtomicU64:
//     - key  : Zobrist hash of the position
//     - data : compressed data (score, depth, flag, move)
//
//   Reads/writes use Ordering::Relaxed for performance.
//   Benign "races" (reading an entry currently being written by
//   another thread) simply result in a cache-miss — never a logic
//   error.
//
// Encoding of the `data` field (64 bits):
//   bits  0-20 : score + 1_000_000 (21 bits, values [0, 2_000_000])
//   bits 21-27 : depth (7 bits, values [0, 127])
//   bits 28-29 : TTFlag (2 bits: 0=Exact, 1=LowerBound, 2=UpperBound)
//   bits 30-35 : move's from square (6 bits)
//   bits 36-41 : move's to square (6 bits)
//   bits 42-44 : MoveFlags enum (3 bits, values 0-7)
//   bits 45-47 : promotion piece (3 bits, values 0-7)
//   bits 48-63 : unused
//
// Replacement policy with generation number (per "go" command):
//   - Stale entry (different generation) → always replaced.
//   - Same generation → replaced if depth >= old depth.
// The generation is encoded in bits 48-55 of the data field (8 bits, 256 values).
// =============================================================================

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use crate::utils::types::{Move, MoveFlags};

/// Entry type in the transposition table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TTFlag {
    /// Exact score (full alpha-beta window traversed).
    Exact      = 0,
    /// Lower bound (fail high — score >= beta).
    LowerBound = 1,
    /// Upper bound (fail low — score <= alpha).
    UpperBound = 2,
}

/// An entry decoded from the transposition table.
/// Used only as a return value — not stored directly.
#[derive(Clone, Copy, Debug)]
pub struct TTEntry {
    /// Zobrist hash of the position (to detect collisions).
    pub hash:      u64,
    /// Score of the position.
    pub score:     i32,
    /// Depth at which this score was computed.
    pub depth:     i32,
    /// Type of the score (exact, lower bound, or upper bound).
    pub flag:      TTFlag,
    /// Best move found for this position (for move ordering).
    pub best_move: Move,
}

// =============================================================================
// Atomic slot (key/data pair)
// =============================================================================

/// An atomic slot in the transposition table.
/// key and data are each an AtomicU64 for lock-free access.
struct TtSlot {
    /// Zobrist hash — used to verify we're reading the right position.
    key:  AtomicU64,
    /// Compressed data (see encoding in the file header).
    data: AtomicU64,
}

// =============================================================================
// Encoding / decoding functions
// =============================================================================

/// Compresses score, depth, flag, move AND generation into a u64.
///
/// Encoding (64 bits):
///   bits  0-20 : score + 1_000_000 (21 bits, [0, 2_000_000])
///   bits 21-27 : depth             (7 bits, [0, 127])
///   bits 28-29 : TTFlag            (2 bits: 0=Exact, 1=Lower, 2=Upper)
///   bits 30-35 : from square       (6 bits)
///   bits 36-41 : to square         (6 bits)
///   bits 42-44 : MoveFlags         (3 bits)
///   bits 45-47 : promotion piece   (3 bits)
///   bits 48-55 : generation        (8 bits) ← NEW
///   bits 56-63 : unused
fn pack_data(score: i32, depth: i32, flag: TTFlag, mv: Move, gen: u8) -> u64 {
    let s  = (score + 1_000_000) as u64;   // 21 bits
    let d  = depth as u64;                  //  7 bits
    let f  = flag as u64;                   //  2 bits
    let fr = mv.from as u64;               //  6 bits
    let to = mv.to as u64;                 //  6 bits
    let mf = mv.flags as u64;             //  3 bits
    let p  = mv.promotion as u64;          //  3 bits
    let g  = gen as u64;                   //  8 bits

    s | (d << 21) | (f << 28) | (fr << 30) | (to << 36) | (mf << 42) | (p << 45) | (g << 48)
}

/// Decompresses a u64 into (score, depth, flag, move, generation).
fn unpack_data(data: u64) -> (i32, i32, TTFlag, Move, u8) {
    let score = (data & 0x1F_FFFF) as i32 - 1_000_000;
    let depth = ((data >> 21) & 0x7F) as i32;
    let flag  = match (data >> 28) & 0x3 {
        0 => TTFlag::Exact,
        1 => TTFlag::LowerBound,
        _ => TTFlag::UpperBound,
    };
    let from  = ((data >> 30) & 0x3F) as u8;
    let to    = ((data >> 36) & 0x3F) as u8;
    let mf    = match (data >> 42) & 0x7 {
        0 => MoveFlags::Quiet,
        1 => MoveFlags::DoublePush,
        2 => MoveFlags::CastleKingside,
        3 => MoveFlags::CastleQueenside,
        4 => MoveFlags::Capture,
        5 => MoveFlags::EnPassant,
        6 => MoveFlags::Promotion,
        _ => MoveFlags::PromotionCapture,
    };
    let promo = ((data >> 45) & 0x7) as u8;
    let gen   = ((data >> 48) & 0xFF) as u8;
    let mv    = Move { from, to, flags: mf, promotion: promo };

    (score, depth, flag, mv, gen)
}

// =============================================================================
// Transposition table
// =============================================================================

/// Lock-free transposition table, shareable between threads via Arc.
///
/// Uses AtomicU64 pairs (key, data) for each entry.
/// Benign races (reading an entry currently being written) result
/// in a cache-miss — never a logic error.
pub struct TranspositionTable {
    /// Array of atomic slots.
    slots:      Vec<TtSlot>,
    /// Indexing mask (slots.len() - 1, always a power of 2).
    mask:       u64,
    /// Current generation number (incremented on each "go" command).
    /// Allows distinguishing fresh entries from stale ones.
    generation: AtomicU8,
}

// SAFETY: TtSlot only contains AtomicU64, which are Sync.
// TranspositionTable is therefore Sync, and thus Arc<TranspositionTable> is Send+Sync.
unsafe impl Send for TranspositionTable {}
unsafe impl Sync for TranspositionTable {}

impl TranspositionTable {
    /// Attempts to create a transposition table of `size_mb` MB, WITHOUT ever
    /// aborting the program. The actual size is rounded down to the nearest
    /// power of 2. Returns `None` if the allocation fails (insufficient memory).
    ///
    /// Robustness: the allocation uses `Vec::try_reserve_exact`, which returns
    /// an error instead of calling `handle_alloc_error` (process abort)
    /// when the allocator cannot supply the memory. This is the basis of the
    /// graceful fallback on the UCI side: an overly ambitious `Hash` setting no longer kills the engine.
    ///
    /// Honest limitation: `try_reserve` catches OUTRIGHT REFUSALS from the
    /// allocator (the most common crash vector). On systems with memory
    /// overcommit, a reservation may "succeed" virtually and then pressure
    /// RAM while filling it — that case would require querying physical
    /// RAM (outside the standard library). The fallback remains a clear
    /// improvement: no more aborts on allocation refusal.
    pub fn try_new(size_mb: usize) -> Option<TranspositionTable> {
        // 2 AtomicU64 per slot = 16 bytes per slot
        let bytes_per_slot = 16usize;
        let total_bytes    = size_mb.saturating_mul(1024 * 1024);
        let num_slots_raw  = total_bytes / bytes_per_slot;

        // Power of 2 less than or equal
        let mut num_slots = 1usize;
        while num_slots * 2 <= num_slots_raw {
            num_slots *= 2;
        }
        if num_slots == 0 { num_slots = 1; }

        let mask = (num_slots - 1) as u64;

        // FALLIBLE allocation: try_reserve_exact returns Err instead of aborting.
        let mut slots: Vec<TtSlot> = Vec::new();
        if slots.try_reserve_exact(num_slots).is_err() {
            return None;
        }
        // The capacity is now guaranteed → these pushes never reallocate
        // (so they cannot fail).
        for _ in 0..num_slots {
            slots.push(TtSlot { key: AtomicU64::new(0), data: AtomicU64::new(0) });
        }

        Some(TranspositionTable { slots, mask, generation: AtomicU8::new(0) })
    }

    /// Creates a transposition table of `size_mb` MB with GUARANTEED GRACEFUL
    /// FALLBACK: if the allocation fails, the size is halved until it
    /// succeeds. NEVER panics — consistent with the engine's robustness
    /// priority. Used at startup (where a small size succeeds anyway).
    /// To finely control the fallback (UCI message, actual size retained), the
    /// UCI layer instead calls `try_new()` directly.
    pub fn new(size_mb: usize) -> TranspositionTable {
        let mut try_size = size_mb.max(1);
        loop {
            if let Some(tt) = Self::try_new(try_size) {
                return tt;
            }
            if try_size <= 1 {
                break; // even 1 MB fails: minimal fallback below
            }
            try_size /= 2;
        }

        // Absolute last resort (system with virtually no memory): a table of a
        // single slot (mask = 0 → everything indexes slot 0). Inefficient but VALID
        // and crash-free — always better than aborting.
        TranspositionTable {
            slots: vec![TtSlot { key: AtomicU64::new(0), data: AtomicU64::new(0) }],
            mask: 0,
            generation: AtomicU8::new(0),
        }
    }

    /// Computes the index of a hash in the table.
    #[inline]
    fn index(&self, hash: u64) -> usize {
        (hash & self.mask) as usize
    }

    /// Prefetches into the cache the line containing the slot associated with
    /// `hash`, without reading it or returning anything. To be called as soon as
    /// the CHILD position's hash is known (right after make_move), BEFORE the
    /// recursive descent: the latency of accessing the TT (often 64 MiB,
    /// frequently outside the cache) is then masked by the work that follows
    /// (gives_check, extensions, LMP…), so that the slot is already warm when the child calls probe().
    ///
    /// SAFETY of the `unsafe` blocks: the prefetch instructions (`prfm` on
    /// aarch64, `_mm_prefetch` on x86-64) are simple hardware HINTS.
    /// They NEVER fault — even on an invalid address — and modify no
    /// observable architectural state. Moreover the pointer comes from a slot
    /// indexed within bounds (index() applies `& mask`), so it is always valid.
    /// On other architectures: no-op (prefetching is only an optimization,
    /// never a correctness requirement).
    #[inline(always)]
    pub fn prefetch(&self, hash: u64) {
        let slot_ptr = &self.slots[self.index(hash)] as *const TtSlot;

        #[cfg(target_arch = "aarch64")]
        // SAFETY: prfm is a prefetch hint that never faults
        // and modifies no observable state (see the doc above).
        unsafe {
            core::arch::asm!(
                "prfm pldl1keep, [{ptr}]",
                ptr = in(reg) slot_ptr,
                options(nostack, readonly, preserves_flags),
            );
        }

        #[cfg(target_arch = "x86_64")]
        // SAFETY: _mm_prefetch is a prefetch hint that never faults
        // and modifies no observable state (see the doc above).
        unsafe {
            core::arch::x86_64::_mm_prefetch(
                slot_ptr as *const i8,
                core::arch::x86_64::_MM_HINT_T0,
            );
        }

        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        {
            // Other architectures: no-op. `let _` avoids an unused warning.
            let _ = slot_ptr;
        }
    }

    /// Probes the table for a given hash.
    /// Returns Some(entry) if a valid entry is found, None otherwise.
    ///
    /// Thread-safe: Relaxed reads are consistent for a cache.
    pub fn probe(&self, hash: u64) -> Option<TTEntry> {
        let slot = &self.slots[self.index(hash)];
        let k    = slot.key.load(Ordering::Relaxed);
        let d    = slot.data.load(Ordering::Relaxed);

        // Check the hash and that the slot is not empty
        if k != hash || d == 0 {
            return None;
        }

        let (score, depth, flag, best_move, _gen) = unpack_data(d);
        Some(TTEntry { hash, score, depth, flag, best_move })
    }

    /// Stores an entry in the table.
    ///
    /// Replacement policy by generation + depth:
    ///   - Empty slot → always write.
    ///   - Different generation (stale entry from a previous search)
    ///     → always replace: a fresh entry at depth 1 is worth more
    ///     than a stale entry at depth 8.
    ///   - Same generation → replace only if depth >= old.
    ///
    /// Thread-safe: Relaxed writes are sufficient for a cache.
    pub fn store(
        &self,
        hash:      u64,
        score:     i32,
        depth:     i32,
        flag:      TTFlag,
        best_move: Move,
    ) {
        let slot        = &self.slots[self.index(hash)];
        let old_data    = slot.data.load(Ordering::Relaxed);
        let current_gen = self.generation.load(Ordering::Relaxed);

        if old_data != 0 {
            let (_, old_depth, _, _, old_gen) = unpack_data(old_data);
            // Same generation: keep deeper entries.
            // Different generation: always replace (stale entry).
            if old_gen == current_gen && depth < old_depth {
                return;
            }
        }

        let data = pack_data(score, depth, flag, best_move, current_gen);
        // Write data BEFORE key to minimize benign races
        slot.data.store(data, Ordering::Relaxed);
        slot.key.store(hash,  Ordering::Relaxed);
    }

    /// Increments the generation number at the start of a new search.
    ///
    /// All existing entries will be considered stale on the next
    /// store(), and replaceable even by entries with lower depth.
    /// Call once at the start of each "go" command.
    pub fn new_search(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Clears the entire table (between two games).
    /// Thread-safe: uses atomic stores.
    pub fn clear(&self) {
        for slot in &self.slots {
            slot.key.store(0,  Ordering::Relaxed);
            slot.data.store(0, Ordering::Relaxed);
        }
    }

    /// Estimates the table's fill rate in permille (0–1000).
    ///
    /// Samples the first 1,000 slots (or all if the table is smaller).
    /// Each slot whose `data` field is non-zero is considered occupied.
    /// Used by the UCI protocol ("info hashfull <n>" command).
    pub fn hashfull(&self) -> u32 {
        let sample = self.slots.len().min(1000);
        if sample == 0 { return 0; }
        let filled = self.slots[..sample]
            .iter()
            .filter(|s| s.data.load(Ordering::Relaxed) != 0)
            .count();
        (filled * 1000 / sample) as u32
    }

    /// Adjusts a mate score from the table for the current depth.
    /// Mate scores are stored relative to the root.
    pub fn adjust_score_from_tt(score: i32, ply: i32) -> i32 {
        use crate::utils::types::SCORE_MATE;
        if score > SCORE_MATE - 200 {
            score - ply
        } else if score < -SCORE_MATE + 200 {
            score + ply
        } else {
            score
        }
    }

    /// Adjusts a mate score for storage in the table.
    pub fn adjust_score_for_tt(score: i32, ply: i32) -> i32 {
        use crate::utils::types::SCORE_MATE;
        if score > SCORE_MATE - 200 {
            score + ply
        } else if score < -SCORE_MATE + 200 {
            score - ply
        } else {
            score
        }
    }
}
