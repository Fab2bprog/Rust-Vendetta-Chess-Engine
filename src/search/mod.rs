// =============================================================================
// Vendetta Chess Engine — src/search/mod.rs
//
// Role: Search coordinator. Implements iterative deepening,
//        Lazy SMP multi-threading, time management, difficulty
//        levels, and the interface between the UCI and the alpha-beta algorithm.
//
// Multi-thread architecture (Lazy SMP):
//   - The main thread performs the normal search with iterative deepening.
//   - The secondary threads each perform their own independent search
//     on their own copy of the board (Board::clone()).
//   - All threads share the same transposition table (Arc<TT>).
//   - An Arc<AtomicBool> serves as a shared stop signal:
//     when time runs out (main thread), all threads stop.
//
// Benefit of Lazy SMP:
//   The secondary threads populate the shared TT with evaluations
//   at various depths. The main thread benefits from this via TT hits,
//   which improves move ordering and speeds up the search.
//
// Iterative Deepening:
//   We explore depth 1, then 2, then 3, etc.
//   At each depth, we keep the best move found.
//   If time runs out, we return the best move from the last
//   completed depth. This always guarantees a valid move.
// =============================================================================

pub mod transposition;
pub mod killers;
pub mod history;
pub mod countermove;
pub mod continuation_history;
pub mod see;
pub mod alphabeta;

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use crate::utils::types::{Move, SCORE_MATE};
use crate::board::state::Board;
use crate::moves::generate_legal_moves;
use transposition::TranspositionTable;
use killers::KillerMoves;
use history::HistoryTable;
use countermove::CountermoveTable;
use continuation_history::ContinuationHistoryTable;
use alphabeta::{alpha_beta, MAX_PLY};

/// Sentinel value indicating that no static evaluation has been
/// recorded at this ply for the current branch (node in check, root,
/// or SE check — see alphabeta.rs::static_eval_opt). Used by
/// the "improving" flag (RFP/LMP/NMP): distinguishes "no data" from an
/// actual evaluation, which is always bounded by ±SCORE_MATE, well
/// above i32::MIN.
pub const EVAL_HISTORY_NONE: i32 = i32::MIN;

// =============================================================================
// Correction History — parameters and structure (rework steps A + B)
// =============================================================================
//
// Adjusts a node's static eval based on the historical gap observed between the
// static eval and the actual search score, for positions sharing a
// common sub-structure. A better-calibrated eval → better pruning
// decisions (RFP, NMP, futility, improving). Tables PER THREAD (stored in SearchInfo),
// indexed by the side to move. Can be disabled via SearchInfo.toggles.disable_correction.
//
// Rework compared to v1 (which came out at -3 Elo in SPRT):
//   A) DEPTH-WEIGHTED learning, in FIXED POINT (sub-centipawn
//      resolution): a correction from a deep search (reliable)
//      carries more weight than one from a shallow search (noisy). The
//      v1 used a fixed step (CORRHIST_RATE) that ignored this reliability.
//   B) SEVERAL tables combined via WEIGHTED AVERAGE (the noise of one
//      isolated key gets averaged out, instead of directly perturbing the eval):
//        - pawn structure,
//        - white non-pawn pieces,
//        - black non-pawn pieces,
//        - continuation: (piece type, destination square) of the last move.

/// Number of entries per color for the pawn / non-pawn tables (power of 2).
const CORRHIST_SIZE: usize = 1 << 14; // 16384
const CORRHIST_MASK: usize = CORRHIST_SIZE - 1;
/// Entries per color of the continuation table: (piece type × square).
const CORRHIST_CONT_SIZE: usize = 6 * 64;
/// Fixed-point scale: values are stored in centipawns × GRAIN.
const CORRHIST_GRAIN: i32 = 256;
/// Maximum correction stored/applied PER TABLE (± centipawns).
const CORRHIST_MAX_CP: i32 = 64;
const CORRHIST_LIMIT: i32 = CORRHIST_MAX_CP * CORRHIST_GRAIN;
/// Denominator of the moving average (effective weight ∈ [1, WEIGHT_CAP]).
const CORRHIST_WEIGHT_SCALE: i32 = 256;
/// Maximum weight of an update, reached at great depth.
const CORRHIST_WEIGHT_CAP: i32 = 16;
// Relative weights of each table in the final correction (weighted average).
const CW_PAWN:    i32 = 2;
const CW_NONPAWN: i32 = 1;
const CW_CONT:    i32 = 2;

/// Pre-computed keys (once per node) for indexing the Correction
/// History tables. Computed by `corr_keys()` in alphabeta.rs (which has access to the Board).
pub struct CorrKeys {
    /// Side to move (0 = White, 1 = Black).
    pub stm:       usize,
    /// Pawn structure key (the two pawn bitboards mixed together).
    pub pawn:      u64,
    /// White non-pawn pieces key.
    pub nonpawn_w: u64,
    /// Black non-pawn pieces key.
    pub nonpawn_b: u64,
    /// Continuation index: `piece_type × 64 + square` of the last move, or None
    /// (root, null move, or empty destination square — should not happen).
    pub cont:      Option<usize>,
}

/// Correction History tables (per thread). Values in fixed point (× GRAIN).
pub struct CorrectionHistory {
    pawn:      Vec<i32>, // [stm × CORRHIST_SIZE + (pawn & MASK)]
    nonpawn_w: Vec<i32>,
    nonpawn_b: Vec<i32>,
    cont:      Vec<i32>, // [stm × CORRHIST_CONT_SIZE + (piece_type × 64 + square)]
}

impl Default for CorrectionHistory {
    fn default() -> CorrectionHistory { CorrectionHistory::new() }
}

impl CorrectionHistory {
    pub fn new() -> CorrectionHistory {
        CorrectionHistory {
            pawn:      vec![0; 2 * CORRHIST_SIZE],
            nonpawn_w: vec![0; 2 * CORRHIST_SIZE],
            nonpawn_b: vec![0; 2 * CORRHIST_SIZE],
            cont:      vec![0; 2 * CORRHIST_CONT_SIZE],
        }
    }

    #[inline]
    fn pawn_idx(k: &CorrKeys) -> usize { k.stm * CORRHIST_SIZE + (k.pawn as usize & CORRHIST_MASK) }
    #[inline]
    fn npw_idx(k: &CorrKeys) -> usize { k.stm * CORRHIST_SIZE + (k.nonpawn_w as usize & CORRHIST_MASK) }
    #[inline]
    fn npb_idx(k: &CorrKeys) -> usize { k.stm * CORRHIST_SIZE + (k.nonpawn_b as usize & CORRHIST_MASK) }
    #[inline]
    fn cont_idx(k: &CorrKeys, c: usize) -> usize { k.stm * CORRHIST_CONT_SIZE + c }

    /// Correction (centipawns) to ADD to the static eval: WEIGHTED average of the
    /// tables (computed in fixed point), bounded to ±CORRHIST_MAX_CP. Averaging
    /// several keys reduces the noise from a single isolated table — that was the weakness
    /// of v1 (a single pawn table → direct noise on the eval, -3 Elo).
    #[inline]
    pub fn value(&self, k: &CorrKeys) -> i32 {
        let mut sum = self.pawn[Self::pawn_idx(k)]      * CW_PAWN
                    + self.nonpawn_w[Self::npw_idx(k)]  * CW_NONPAWN
                    + self.nonpawn_b[Self::npb_idx(k)]  * CW_NONPAWN;
        let mut wtot = CW_PAWN + 2 * CW_NONPAWN;
        if let Some(c) = k.cont {
            sum  += self.cont[Self::cont_idx(k, c)] * CW_CONT;
            wtot += CW_CONT;
        }
        // sum is in (cp × GRAIN × weight) → we convert back to cp.
        let cp = sum / (CORRHIST_GRAIN * wtot);
        cp.clamp(-CORRHIST_MAX_CP, CORRHIST_MAX_CP)
    }

    /// DEPTH-WEIGHTED learning: each table slides toward `diff`
    /// (= search score − corrected node eval), by a step proportional to
    /// the depth (a deep search is more reliable, so it carries more weight).
    #[inline]
    pub fn update(&mut self, k: &CorrKeys, diff: i32, depth: i32) {
        let target = diff.clamp(-CORRHIST_MAX_CP, CORRHIST_MAX_CP) * CORRHIST_GRAIN;
        let weight = (depth + 1).clamp(1, CORRHIST_WEIGHT_CAP);
        Self::blend(&mut self.pawn[Self::pawn_idx(k)], target, weight);
        Self::blend(&mut self.nonpawn_w[Self::npw_idx(k)], target, weight);
        Self::blend(&mut self.nonpawn_b[Self::npb_idx(k)], target, weight);
        if let Some(c) = k.cont {
            let idx = Self::cont_idx(k, c);
            Self::blend(&mut self.cont[idx], target, weight);
        }
    }

    /// Fixed-point moving average: `entry += (target − entry) × weight / SCALE`,
    /// then bounded to ±CORRHIST_LIMIT. (entry and target in cp × GRAIN.)
    #[inline]
    fn blend(entry: &mut i32, target: i32, weight: i32) {
        *entry += (target - *entry) * weight / CORRHIST_WEIGHT_SCALE;
        *entry = (*entry).clamp(-CORRHIST_LIMIT, CORRHIST_LIMIT);
    }
}

// =============================================================================
// Search data structures
// =============================================================================

/// RUNTIME switches for search heuristics, grouped here so as not
/// to scatter "test-only" fields across SearchInfo.
///
/// ALL false in normal play → no effect, no overhead (branches always
/// not taken, perfectly predicted). Only the `selfplay` binary toggles
/// them to ISOLATE a feature in an SPRT match (config file's `*_a` / `*_b`
/// keys). Setting a field to true DISABLES the corresponding feature:
///   - disable_improving  : `improving` flag (RFP/NMP/LMP/LMR)
///   - disable_futility   : per-move Futility Pruning
///   - disable_lmr_tweaks : ±1 adjustments of the enriched LMR (base kept)
///   - disable_correction : Correction History (raw eval, nothing read/learned)
///   - disable_king_attack: "king safety via attack" term (eval)
#[derive(Default)]
pub struct FeatureToggles {
    pub disable_improving:  bool,
    pub disable_futility:   bool,
    pub disable_lmr_tweaks: bool,
    pub disable_correction: bool,
    pub disable_king_attack: bool,
}

/// Information shared during a search.
/// The stop signal is an Arc<AtomicBool> shared between all threads.
pub struct SearchInfo {
    /// Search start time.
    pub start_time: Instant,
    /// Time limit allocated for this search.
    pub time_limit: Duration,
    /// Number of nodes explored (by this thread).
    pub nodes: u64,
    /// Best move found so far (by this thread).
    pub best_move: Move,
    /// Score associated with the best move.
    pub best_score: i32,
    /// Depth reached.
    pub depth_reached: i32,
    /// Maximum selective depth reached (including quiescence).
    /// Reset at each new depth in the iteration.
    /// Used for the UCI "seldepth" field.
    pub seldepth: i32,
    /// Node limit for this search (UCI command "go nodes <x>").
    /// None = no limit (default behavior, time-driven).
    /// Checked only on the main thread — under Lazy SMP, the total
    /// number of nodes actually explored (across all threads) can slightly
    /// exceed this limit, just like with existing time limits.
    pub max_nodes: Option<u64>,
    /// Stop signal shared between all Lazy SMP threads.
    /// When the main thread runs out of time, it sets this flag to true,
    /// and all secondary threads stop at their next check.
    pub stop: Arc<AtomicBool>,
    /// Enables emission of "info currmove/currmovenumber" (root only).
    ///
    /// false by default — IMPORTANT: alpha_beta() is called directly by
    /// several tools outside the actual UCI engine (notably src/bin/
    /// benchmark.rs, which builds its own SearchInfo to measure raw NPS
    /// without the UCI layer). If this line were printed unconditionally,
    /// it would pollute the output of these tools — this is exactly what
    /// happened before this fix (benchmark drowned in currmove lines).
    /// Only SearchEngine::search() (the actual UCI-driven search)
    /// explicitly enables this flag on the main thread's instance.
    pub show_currmove: bool,
    /// Static evaluation per ply, for the "improving" flag (RFP/LMP/NMP)
    /// — see alphabeta.rs. Indexed directly by ply (0..MAX_PLY).
    /// EVAL_HISTORY_NONE = no value recorded at this ply for THIS
    /// branch (see static_eval_opt in alphabeta.rs for the conditions).
    pub eval_history: [i32; MAX_PLY],
    /// Contempt factor (UCI option "Contempt", centipawns). 0 by default
    /// = unchanged behavior (exact SCORE_DRAW for any drawn position).
    /// A positive value slightly penalizes draws from the point of view
    /// of the side at the root of the search — see alphabeta.rs::draw_score().
    ///
    /// IMPORTANT: must be IDENTICAL across all Lazy SMP threads of the
    /// same search (the shared TT stores scores that must remain
    /// consistent regardless of which thread computed them) — see
    /// SearchEngine::search() which copies it to the main thread AND to
    /// each secondary thread from the same SearchConfig.
    pub contempt: i32,
    /// Correction History (per thread): several tables combined (pawn,
    /// non-pawn per color, continuation). See the CorrectionHistory struct and
    /// its usage in alphabeta.rs (corr_keys + value/update). ⚠️ to be validated via SPRT.
    pub correction_history: CorrectionHistory,
    /// Runtime switches for heuristics (SPRT tests of the selfplay binary).
    /// All false in normal play. See the FeatureToggles struct.
    pub toggles: FeatureToggles,
}

impl SearchInfo {
    /// Creates a new instance with its own stop signal.
    pub fn new(time_limit: Duration) -> SearchInfo {
        SearchInfo {
            start_time:    Instant::now(),
            time_limit,
            nodes:         0,
            best_move:     Move::NULL,
            best_score:    0,
            depth_reached: 0,
            seldepth:      0,
            max_nodes:     None,
            stop:          Arc::new(AtomicBool::new(false)),
            show_currmove: false,
            eval_history:  [EVAL_HISTORY_NONE; MAX_PLY],
            contempt:      0,
            correction_history: CorrectionHistory::new(),
            toggles: FeatureToggles::default(),
        }
    }

    /// Creates a shared instance with an external stop signal.
    /// Used by Lazy SMP secondary threads.
    pub fn new_with_stop(time_limit: Duration, stop: Arc<AtomicBool>) -> SearchInfo {
        SearchInfo {
            start_time:    Instant::now(),
            time_limit,
            nodes:         0,
            best_move:     Move::NULL,
            best_score:    0,
            depth_reached: 0,
            seldepth:      0,
            max_nodes:     None,
            stop,
            show_currmove: false,
            eval_history:  [EVAL_HISTORY_NONE; MAX_PLY],
            contempt:      0,
            correction_history: CorrectionHistory::new(),
            toggles: FeatureToggles::default(),
        }
    }

    /// Returns true if the search must stop.
    #[inline]
    pub fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Checks whether the time OR the node limit (if set) has been reached.
    /// To be called periodically. Checks every 4096 nodes so as not to
    /// call Instant::now() on every node (expensive).
    pub fn check_time(&mut self) {
        if self.nodes & 0xFFF == 0 {
            if self.start_time.elapsed() >= self.time_limit {
                self.stop.store(true, Ordering::Relaxed);
            }
            if let Some(max) = self.max_nodes {
                if self.nodes >= max {
                    self.stop.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    /// Updates the best move from the root (ply == 0 only).
    pub fn update_best_move(&mut self, mv: Move, score: i32, ply: usize) {
        if ply == 0 {
            self.best_move  = mv;
            self.best_score = score;
        }
    }

    /// Returns the elapsed time in milliseconds.
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }
}

/// Configuration of a search.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Time available for White (in milliseconds).
    pub wtime: Option<u64>,
    /// Time available for Black (in milliseconds).
    pub btime: Option<u64>,
    /// Increment per move for White (in milliseconds).
    pub winc: Option<u64>,
    /// Increment per move for Black (in milliseconds).
    pub binc: Option<u64>,
    /// Number of moves before the time control.
    pub movestogo: Option<u32>,
    /// Maximum search depth.
    pub depth: Option<i32>,
    /// Fixed thinking time (in milliseconds).
    pub movetime: Option<u64>,
    /// Infinite search (until stop).
    pub infinite: bool,
    /// Difficulty level (1-64). 64 = full strength.
    pub skill_level: u8,
    /// Ponder mode: the engine thinks on the opponent's time.
    /// The search runs in infinite mode until ponderhit or stop.
    pub ponder: bool,
    /// List of moves to analyze in UCI notation (e.g. ["e2e4", "d2d4"]).
    /// Empty = all legal moves (default behavior).
    /// Corresponds to the "searchmoves" parameter of the "go" command.
    pub searchmoves: Vec<String>,
    /// Node limit for this search ("go nodes <x>").
    /// None = no limit (default behavior).
    pub nodes: Option<u64>,
    /// Search for a forced mate in <x> moves ("go mate <x>").
    /// Translated into depth = 2×x plies (a mate in N full moves is
    /// found in at most 2N-1 half-moves; 2N is a slightly loose but
    /// safe bound). None = no specific mate search.
    pub mate: Option<u32>,
    /// Number of principal variations to display ("option MultiPV").
    /// 1 = standard behavior (a single best line).
    pub multipv: usize,
    /// Safety margin (in ms) subtracted from the computed time budget, to
    /// compensate for GUI/network communication latency ("option Move
    /// Overhead"). Replaces the old fixed 50 ms margin hard-coded in
    /// compute_time_limit() — same default value, but now configurable
    /// (important in online play/tournaments: without a sufficient margin, the engine can
    /// lose on time simply because of command relay delay).
    pub move_overhead: u64,
    /// Contempt factor (UCI option "Contempt", centipawns). 0 by default
    /// = unchanged behavior. Copied into SearchInfo::contempt — see
    /// alphabeta.rs::draw_score() for details of the adjustment.
    pub contempt: i32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            wtime:       None,
            btime:       None,
            winc:        None,
            binc:        None,
            movestogo:   None,
            depth:       None,
            movetime:    None,
            infinite:    false,
            skill_level: 64,
            ponder:      false,
            searchmoves: vec![],
            nodes:       None,
            mate:        None,
            multipv:     1,
            move_overhead: 50, // identical to the old fixed margin — zero regression by default
            contempt:    0,
        }
    }
}

/// Result of a search.
pub struct SearchResult {
    /// Best move found.
    pub best_move: Move,
    /// Predicted opponent response move (for pondering).
    /// Obtained by probing the transposition table after best_move.
    /// Move::NULL if no prediction is available (mate, unknown position, etc.).
    pub ponder_move: Move,
    /// Score in centipawns.
    pub score: i32,
    /// Depth reached.
    pub depth: i32,
    /// Number of nodes explored (main thread only).
    pub nodes: u64,
    /// Search time in milliseconds.
    pub time_ms: u64,
}

// =============================================================================
// Main search engine
// =============================================================================

/// Search engine. Contains the shared transposition table and heuristics.
pub struct SearchEngine {
    /// Transposition table shared among all threads via Arc.
    /// Internal AtomicU64 → no need for Mutex, lock-free.
    pub tt:          Arc<TranspositionTable>,
    /// Killer moves (main thread only, not shared).
    pub killers:     KillerMoves,
    /// History heuristic (main thread only, not shared).
    pub history:     HistoryTable,
    /// Countermove heuristic (main thread only, not shared).
    pub countermoves: CountermoveTable,
    /// Continuation history — cumulative generalization of the countermove
    /// (main thread only, not shared). ~576 KiB, allocated on
    /// the heap (see continuation_history.rs for the storage choice).
    pub cont_history: ContinuationHistoryTable,
    /// Number of search threads (1 = single-threaded, >1 = Lazy SMP).
    pub num_threads: usize,
    /// Stop signal shared with the UCI thread.
    /// Set to true by stop() to immediately interrupt the ongoing search.
    pub stop_flag:   Arc<AtomicBool>,
}

impl SearchEngine {
    /// Creates a new search engine.
    pub fn new() -> SearchEngine {
        let default_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        SearchEngine {
            tt:           Arc::new(TranspositionTable::new(32)),
            killers:      KillerMoves::new(),
            history:      HistoryTable::new(),
            countermoves: CountermoveTable::new(),
            cont_history: ContinuationHistoryTable::new(),
            num_threads:  default_threads,
            stop_flag:    Arc::new(AtomicBool::new(false)),
        }
    }

    /// Resets the heuristics between two games.
    pub fn new_game(&mut self) {
        self.tt.clear();
        self.killers.clear();
        self.history.clear();
        self.countermoves.clear();
        self.cont_history.clear();
    }

    /// Immediately interrupts the ongoing search (thread-safe).
    /// The search will stop at the next internal check cycle (~4,096 nodes).
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }

    /// Launches a search on the given position with the specified configuration.
    /// Returns the best move found.
    ///
    /// In multi-thread mode, the secondary threads (Lazy SMP) populate the shared
    /// TT in parallel with the main thread.
    pub fn search(&mut self, board: &mut Board, config: &SearchConfig) -> SearchResult {
        // Increment the TT generation: the entries from the previous search
        // are now stale and replaceable even at a lower depth.
        self.tt.new_search();

        // Compute the time allocated for this move
        let time_limit = compute_time_limit(board, config);

        // Maximum depth: "mate <x>" takes priority over "depth <x>"
        // if specified (search for a forced mate in x moves → 2x plies,
        // safe bound — see SearchConfig::mate).
        let max_depth = if let Some(mate_in) = config.mate {
            (mate_in as i32).saturating_mul(2).max(1)
        } else {
            config.depth.unwrap_or(if config.infinite { 128 } else { 64 })
        };

        // Make sure there is at least one legal move
        let legal_moves = generate_legal_moves(board);
        if legal_moves.is_empty() {
            return SearchResult {
                best_move:   Move::NULL,
                ponder_move: Move::NULL,
                score:       0,
                depth:       0,
                nodes:       0,
                time_ms:     0,
            };
        }

        // Pre-filtering searchmoves — only once, before the depth iteration.
        // Advantages vs the old filter in alpha_beta:
        //   - to_uci() (String allocation) called O(|legal_moves|) instead of
        //     O(|legal_moves| × depth × aspiration_retries).
        //   - SearchInfo no longer has a heap-allocated Vec<String> propagated at every node.
        //   - alpha_beta receives &[Move] (slice, zero copy) only at ply==0.
        let root_moves: Vec<Move> = if config.searchmoves.is_empty() {
            // Empty Vec → alpha_beta generates the moves normally.
            vec![]
        } else {
            let filtered: Vec<Move> = legal_moves.iter()
                .copied()
                .filter(|mv| config.searchmoves.iter().any(|s| *s == mv.to_uci()))
                .collect();
            // Defense: invalid list → let alpha_beta generate everything.
            if filtered.is_empty() { vec![] } else { filtered }
        };

        // Default move = first legal move (safety in case time runs out
        // before completing the first depth)
        let mut best_move  = legal_moves[0];
        let mut best_score = 0i32;

        // Maximum depth based on the difficulty level
        let effective_max_depth = skill_level_max_depth(config.skill_level, max_depth);

        // Aging of the history between searches
        self.history.age();
        self.killers.clear();
        // Countermove: like the killers, reset to zero on every "go" — a
        // countermove relevant in one search has no reason to
        // be relevant in the following position (the opponent actually made a move).
        self.countermoves.clear();
        // Continuation history: aged like the history (not a full
        // clear()) — a CUMULATIVE score retains value in being carried over,
        // attenuated, from one search to the next, unlike the countermove
        // which is only a single slot per context.
        self.cont_history.age();

        // --- Stop signal shared among all threads ---
        // Reset for this search cycle, then share via Arc.
        // The UCI thread can also set self.stop_flag via stop() at any time.
        self.stop_flag.store(false, Ordering::SeqCst);
        let stop_flag = Arc::clone(&self.stop_flag);

        // --- Launching secondary threads (Lazy SMP) ---
        let mut handles = vec![];
        let num_threads = self.num_threads;

        if num_threads > 1 {
            for t in 1..num_threads {
                // Each secondary thread has its own copy of the board
                let mut board_copy = board.clone();
                // All share the same TT via Arc
                let tt_shared = Arc::clone(&self.tt);
                // All share the same stop signal
                let stop_shared = Arc::clone(&stop_flag);
                // Depth variation to diversify the searches
                let depth_variation = (t % 3) as i32;
                // root_moves shared with the main thread (Clone O(searchmoves))
                let root_moves_smp = root_moves.clone();
                // Contempt copied by value (i32: Copy) — see the note on
                // SearchInfo::contempt: MUST be identical on all
                // threads, otherwise the shared TT would store inconsistent scores.
                let contempt_smp = config.contempt;

                // 8 MiB stack (instead of the ~2 MiB default of Rust threads): the
                // search is heavily recursive (up to ~MAX_PLY plies +
                // quiescence) and each frame now carries move lists
                // allocated on the STACK (MoveList + scored capture array).
                // 8 MiB gives a wide margin against any stack overflow on
                // these secondary threads (the main thread already has ~8 MiB by
                // default on macOS). In case of creation failure (OS resources
                // exhausted), we simply continue with fewer threads — the
                // search remains correct, just a bit less parallel.
                let builder = std::thread::Builder::new().stack_size(8 * 1024 * 1024);
                let spawn_result = builder.spawn(move || {
                    // Thread-local heuristics (not shared)
                    let mut killers      = KillerMoves::new();
                    let mut history      = HistoryTable::new();
                    let mut countermoves = CountermoveTable::new();
                    let mut cont_history = ContinuationHistoryTable::new();
                    // SearchInfo with shared stop signal and unlimited time
                    // (the thread stops only via stop_flag)
                    let mut info = SearchInfo::new_with_stop(
                        Duration::from_secs(3600),
                        stop_shared.clone(),
                    );
                    info.contempt = contempt_smp;

                    // Search with slight depth variation
                    let depth_max = effective_max_depth.saturating_add(depth_variation);
                    for depth in 1..=depth_max {
                        if stop_shared.load(Ordering::Relaxed) { break; }
                        alpha_beta(
                            &mut board_copy,
                            depth,
                            -SCORE_MATE,
                            SCORE_MATE,
                            0,
                            &tt_shared,
                            &mut killers,
                            &mut history,
                            &mut countermoves,
                            &mut cont_history,
                            Move::NULL, // prev_move: no previous move at the root
                            &mut info,
                            Move::NULL,
                            &root_moves_smp,
                        );
                    }
                });
                match spawn_result {
                    Ok(handle) => handles.push(handle),
                    Err(_)     => { /* thread not created: continuing with fewer threads */ }
                }
            }
        }

        // --- Main search (current thread) with Aspiration Windows ---
        let mut info = SearchInfo::new_with_stop(time_limit, Arc::clone(&stop_flag));
        info.max_nodes = config.nodes; // "go nodes <x>" — None if not specified
        // Only this thread (the actual search driven by the UCI) emits
        // "info currmove" — see the comment on SearchInfo::show_currmove.
        info.show_currmove = true;
        info.contempt = config.contempt;

        for depth in 1..=effective_max_depth {
            // Reset the best move and seldepth before each new depth.
            // Ensures that a result at depth N does not pollute depth N+1.
            info.best_move = Move::NULL;
            info.seldepth  = depth; // the regular search reaches at least `depth` plies

            // --- Aspiration Windows ---
            // Starting from depth 4, we first search within a
            // narrow window around the previous score. If it fails (fail-low/fail-high),
            // we progressively widen it. In practice, the narrow window is sufficient
            // most of the time and considerably speeds up the search.
            let score;

            if depth >= 4 && best_score.abs() < SCORE_MATE - 200 {
                // Initial window of ±50 centipawns around the previous score
                let mut asp_delta = 50i32;
                let mut asp_alpha = (best_score - asp_delta).max(-SCORE_MATE);
                let mut asp_beta  = (best_score + asp_delta).min(SCORE_MATE);
                // Initialization after the first iteration of the loop (never read before)
                let mut asp_score;

                'aspiration: loop {
                    // Reset before each attempt within the window
                    info.best_move = Move::NULL;

                    asp_score = alpha_beta(
                        board, depth, asp_alpha, asp_beta, 0,
                        &self.tt, &mut self.killers, &mut self.history,
                        &mut self.countermoves, &mut self.cont_history,
                        Move::NULL, &mut info,
                        Move::NULL,
                        &root_moves,
                    );

                    if info.should_stop() { break 'aspiration; }

                    if asp_score <= asp_alpha {
                        // Fail-low: the true score is BELOW our window.
                        // → score is AT MOST asp_score (upperbound UCI).
                        let el  = info.elapsed_ms();
                        let nps = compute_nps(info.nodes, el);
                        println!(
                            "info depth {} seldepth {} score {} upperbound nodes {} nps {} time {} hashfull {}",
                            depth, info.seldepth, format_score(asp_score),
                            info.nodes, nps, el, self.tt.hashfull(),
                        );
                        asp_alpha  = (asp_alpha - asp_delta).max(-SCORE_MATE);
                        asp_delta  = asp_delta.saturating_mul(2);
                    } else if asp_score >= asp_beta {
                        // Fail-high: the true score is ABOVE our window.
                        // → score is AT LEAST asp_score (lowerbound UCI).
                        let el  = info.elapsed_ms();
                        let nps = compute_nps(info.nodes, el);
                        println!(
                            "info depth {} seldepth {} score {} lowerbound nodes {} nps {} time {} hashfull {}",
                            depth, info.seldepth, format_score(asp_score),
                            info.nodes, nps, el, self.tt.hashfull(),
                        );
                        asp_beta   = (asp_beta + asp_delta).min(SCORE_MATE);
                        asp_delta  = asp_delta.saturating_mul(2);
                    } else {
                        // Score within the window: exact result, we stop
                        break 'aspiration;
                    }

                    // Safety: if the window is at its maximum, do not retry
                    if asp_alpha <= -SCORE_MATE && asp_beta >= SCORE_MATE {
                        info.best_move = Move::NULL;
                        asp_score = alpha_beta(
                            board, depth, -SCORE_MATE, SCORE_MATE, 0,
                            &self.tt, &mut self.killers, &mut self.history,
                            &mut self.countermoves, &mut self.cont_history,
                            Move::NULL, &mut info,
                            Move::NULL,
                            &root_moves,
                        );
                        break 'aspiration;
                    }
                }

                score = asp_score;
            } else {
                // Depths 1-3: full window (no aspiration)
                score = alpha_beta(
                    board, depth, -SCORE_MATE, SCORE_MATE, 0,
                    &self.tt, &mut self.killers, &mut self.history,
                    &mut self.countermoves, &mut self.cont_history,
                    Move::NULL, &mut info,
                    Move::NULL,
                    &root_moves,
                );
            }

            // If the search was interrupted, use the previous result
            if info.should_stop() && depth > 1 {
                break;
            }

            // Update the best move if the depth is complete
            if !info.best_move.is_null() {
                best_move  = info.best_move;
                best_score = score;
                info.depth_reached = depth;
            }

            // Display progress information (UCI protocol)
            let elapsed = info.elapsed_ms();
            let nps     = compute_nps(info.nodes, elapsed);
            println!(
                "info depth {} seldepth {} score {} nodes {} nps {} time {} hashfull {} pv {}",
                depth,
                info.seldepth,
                format_score(score),
                info.nodes,
                nps,
                elapsed,
                self.tt.hashfull(),
                best_move.to_uci(),
            );

            // Stop if the time is exhausted after a complete depth
            if info.start_time.elapsed() >= time_limit && depth > 1 {
                break;
            }

            // Stop if a mate has been found
            if score.abs() > SCORE_MATE - 200 {
                break;
            }
        }

        // --- Signal the stop to the secondary threads ---
        stop_flag.store(true, Ordering::Relaxed);

        // --- Wait for all secondary threads to finish ---
        for h in handles {
            let _ = h.join();
        }

        // Introduce a random error for low difficulty levels
        let final_move = apply_skill_level(board, best_move, config.skill_level);

        // --- Ponder move: predicted response move from the opponent ---
        // We play final_move, probe the TT for the resulting position,
        // then undo the move. The best move stored in the TT for this
        // position is the expected move from the opponent.
        let ponder_move = if !final_move.is_null() {
            board.make_move(final_move);
            let pm = self.tt.probe(board.hash)
                .map(|entry| entry.best_move)
                .unwrap_or(Move::NULL);
            board.unmake_move(final_move);
            pm
        } else {
            Move::NULL
        };

        SearchResult {
            best_move:   final_move,
            ponder_move,
            score:       best_score,
            depth:       info.depth_reached,
            nodes:       info.nodes,
            time_ms:     info.elapsed_ms(),
        }
    }

    /// Runs a MultiPV search: finds the `config.multipv` best
    /// lines (ranked from best to worst), instead of just one.
    /// Returns an ordered vector — index 0 = best line.
    ///
    /// Principle (deliberately simple, reuses search() without modifying it):
    ///   To find the N-th best line, we relaunch a
    ///   complete search (with its own iterative deepening, its own time
    ///   management, taking advantage of Lazy SMP as usual) while EXCLUDING moves
    ///   already selected for previous lines — exactly the mechanism
    ///   `searchmoves` already used to filter the root (UCI option
    ///   "go searchmoves"). No modification of search() or alpha_beta()
    ///   is necessary: MultiPV is just an orchestration layer on top.
    ///
    /// Accepted trade-off (documented rather than hidden):
    ///   - Each searched line takes its own full time (the time budget
    ///     is not divided among the lines) — a MultiPV=3 search
    ///     therefore takes about 3× longer than a normal search.
    ///     This is the standard expected behavior for MultiPV.
    ///   - The transposition table is SHARED between successive calls
    ///     (self.tt), so lines 2, 3... partially benefit from the
    ///     work already done for line 1 — not a totally wasted computation.
    ///   - If MultiPV <= 1: strictly equivalent to calling search()
    ///     directly (no change in default behavior).
    pub fn search_multipv(&mut self, board: &mut Board, config: &SearchConfig) -> Vec<SearchResult> {
        if config.multipv <= 1 {
            return vec![self.search(board, config)];
        }

        let legal_moves = generate_legal_moves(board);
        if legal_moves.is_empty() {
            return vec![SearchResult {
                best_move: Move::NULL, ponder_move: Move::NULL,
                score: 0, depth: 0, nodes: 0, time_ms: 0,
            }];
        }

        // Respect any "searchmoves" already provided by the GUI: it
        // restricts the starting set even before the lines are split up.
        let mut remaining: Vec<Move> = if config.searchmoves.is_empty() {
            legal_moves
        } else {
            let filtered: Vec<Move> = generate_legal_moves(board).into_iter()
                .filter(|mv| config.searchmoves.iter().any(|s| *s == mv.to_uci()))
                .collect();
            if filtered.is_empty() { generate_legal_moves(board) } else { filtered }
        };

        let slots = config.multipv.min(remaining.len()).max(1);
        let mut results = Vec::with_capacity(slots);

        for _ in 0..slots {
            let mut slot_config = config.clone();
            // Restricts this search to the REMAINING moves (not yet ranked).
            slot_config.searchmoves = remaining.iter().map(|mv| mv.to_uci()).collect();

            let result = self.search(board, &slot_config);
            if result.best_move.is_null() {
                // No more legal moves to rank (should only happen at
                // the very start, already handled above — extra safety).
                break;
            }

            remaining.retain(|mv| *mv != result.best_move);
            results.push(result);

            if remaining.is_empty() { break; }
        }

        results
    }
}

// =============================================================================
// Time management
// =============================================================================

/// Calculates the time allocated for this move according to the configuration.
///
/// `config.move_overhead` (UCI option "Move Overhead", 50 ms by default)
/// replaces what used to be a fixed hard-coded margin. It is
/// subtracted from the calculated time to compensate for GUI/network
/// communication latency — without this margin, a delay in relaying
/// commands can cause the game to be lost on time, particularly online (Lichess,
/// cutechess-cli) where latency is not negligible.
fn compute_time_limit(board: &Board, config: &SearchConfig) -> Duration {
    // Fixed time
    if let Some(movetime) = config.movetime {
        return Duration::from_millis(movetime.saturating_sub(config.move_overhead));
    }

    // Infinite search
    if config.infinite {
        return Duration::from_secs(3600);
    }

    // Game time: calculate based on remaining time
    let (time_remaining, increment) = match board.side_to_move {
        crate::utils::types::Color::White => (
            config.wtime.unwrap_or(30_000),
            config.winc.unwrap_or(0),
        ),
        crate::utils::types::Color::Black => (
            config.btime.unwrap_or(30_000),
            config.binc.unwrap_or(0),
        ),
    };

    // Estimate of the number of remaining moves.
    // .max(1): defense against movestogo = Some(0) (invalid per the UCI spec but
    // possible via the public SearchConfig API) — avoids division by zero.
    let moves_to_go = config.movestogo.unwrap_or(30).max(1) as u64;

    // Allocate a fraction of the remaining time + the increment
    let time_for_move = time_remaining / moves_to_go + increment / 2;

    // Never use more than half of the remaining time
    let max_time  = time_remaining / 2;
    let allocated = time_for_move.min(max_time).max(100);

    Duration::from_millis(allocated.saturating_sub(config.move_overhead))
}

// =============================================================================
// UCI_LimitStrength / UCI_Elo — conversion to the skill_level system
// =============================================================================

/// Minimum and maximum Elo covered by the UCI_Elo → skill_level conversion.
/// ELO_MAX = ~2600, consistent with the measured playing strength of Vendetta Chess Engine
/// (confirmed wins against Stockfish at 2500 Elo limited after the Texel
/// Tuning v3 — see README.md). ELO_MIN = 600, reasonable
/// lower bound for an "absolute beginner" (skill_level = 1).
pub const ELO_MIN: u16 = 600;
pub const ELO_MAX: u16 = 2600;

/// Converts a UCI_Elo value into a skill_level (1-64).
///
/// Simple LINEAR interpolation between (ELO_MIN → level 1) and
/// (ELO_MAX → level 64). This is not a precise Elo calibration (which
/// would require hundreds of games per tier to be rigorous) —
/// it is a reasonable mapping allowing GUIs/platforms
/// using the standard UCI_LimitStrength + UCI_Elo mechanism to limit
/// Vendetta Chess Engine, rather than having to know the custom option
/// "Skill Level". Outside the range, the value is clamped.
pub fn elo_to_skill_level(elo: u16) -> u8 {
    let elo = elo.clamp(ELO_MIN, ELO_MAX);
    let fraction = (elo - ELO_MIN) as f32 / (ELO_MAX - ELO_MIN) as f32;
    let skill = 1.0 + fraction * 63.0;
    skill.round().clamp(1.0, 64.0) as u8
}

// =============================================================================
// Difficulty level management
// =============================================================================

/// Maximum depth according to the difficulty level (1-64).
///
/// Continuous graduation over 64 levels:
///   - Level  1: depth 1  (absolute beginner)
///   - Level 16: depth 4  (amateur)
///   - Level 32: depth 7  (intermediate)
///   - Level 48: depth 11 (advanced)
///   - Level 64: full strength (no depth limit)
///
/// Formula: depth = 1 + (skill - 1) * (max_depth - 1) / 63
/// interpolated quadratically for a natural graduation.
fn skill_level_max_depth(skill: u8, requested_depth: i32) -> i32 {
    // Level 64 = full strength, no limit
    if skill >= 64 {
        return requested_depth;
    }

    // Quadratic interpolation between depth 1 (level 1) and
    // depth 16 (level 63). Quadratic so that the early
    // levels progress slowly and the later ones faster.
    let s = (skill as f32 - 1.0) / 62.0; // [0.0, 1.0]
    let max_depth_for_level = (1.0 + s * s * 15.0).round() as i32;

    max_depth_for_level.min(requested_depth)
}

/// Introduces a random error to simulate a human player (levels 1-64).
///
/// Continuous graduation:
///   - Level  1: 90% chance of playing randomly
///   - Level 16: 40% chance
///   - Level 32: 10% chance
///   - Level 48:  2% chance
///   - Level 57+: no error (full strength)
fn apply_skill_level(board: &mut Board, best_move: Move, skill: u8) -> Move {
    // Beyond level 56, always the best move
    if skill >= 57 {
        return best_move;
    }

    // Error probability: quadratic decay from 90% (level 1) to 1% (level 56)
    let s = (skill as f32 - 1.0) / 55.0; // [0.0, 1.0]
    let random_chance = ((1.0 - s * s) * 90.0).round() as u64;

    let pseudo_random = (board.hash
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
        >> 33) % 100;

    if pseudo_random < random_chance {
        let moves = generate_legal_moves(board);
        if !moves.is_empty() {
            return moves[(pseudo_random as usize) % moves.len()];
        }
    }

    best_move
}

// =============================================================================
// UCI formatting
// =============================================================================

/// Formats a score for UCI display.
pub fn format_score(score: i32) -> String {
    if score.abs() > SCORE_MATE - 200 {
        let mate_in = (SCORE_MATE - score.abs() + 1) / 2;
        if score > 0 {
            format!("mate {}", mate_in)
        } else {
            format!("mate -{}", mate_in)
        }
    } else {
        format!("cp {}", score)
    }
}

/// Nodes per second (NPS) for UCI `info` display. Returns 0 if the elapsed
/// duration is zero (case of the very start of a search) — avoids division
/// by zero. `saturating_mul` bounds the (theoretical) case of a u64 overflow.
#[inline]
pub fn compute_nps(nodes: u64, elapsed_ms: u64) -> u64 {
    nodes.saturating_mul(1000).checked_div(elapsed_ms).unwrap_or(0)
}

impl Default for SearchEngine {
    fn default() -> Self {
        SearchEngine::new()
    }
}
