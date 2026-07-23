// =============================================================================
// Vendetta Chess Engine — src/uci/mod.rs
//
// Role: Complete implementation of the UCI (Universal Chess Interface) protocol.
//        Main communication loop between the engine and the graphical
//        interface (Arena, Lichess, Chessbase, etc.).
//
// Contents:
//   - UciEngine: main structure that orchestrates everything
//   - run(): main UCI loop (reads stdin, writes stdout)
//   - Handling of all mandatory UCI commands + pondering
//
// UCI loop architecture:
//   - Reading stdin is done in a dedicated thread via an mpsc channel.
//     This allows the main loop to stay responsive (notably to
//     "stop" and "ponderhit") while a search is in progress.
//   - The search runs in a separate thread (spawn_search).
//
// Pondering (thinking on the opponent's time):
//   Two-mode state machine: Normal and Ponder.
//
//   Full ponder flow:
//     1. Engine → GUI: "bestmove e2e4 ponder e7e5"
//     2. GUI  → Engine: "position ... moves e2e4 e7e5"
//                        "go ponder wtime 300000 btime 300000 ..."
//     3. Engine: starts an INFINITE search on the position after e7e5
//                 (is_pondering = true, ponder_config saved)
//     4a. GUI → Engine: "ponderhit" (opponent played e7e5)
//          → the ponder search is stopped (TT already warm)
//          → a NORMAL search is restarted with ponder_config (time management)
//     4b. GUI → Engine: "stop" (opponent played something else)
//          → the ponder search is stopped
//          → "bestmove <best_move_in_ponder_position>" is sent
//          → the GUI will then send "position" + "go" for the actual position
//
// UCI options of Vendetta Chess Engine:
//   - Hash (MB)      : transposition table size (default 16 MB)
//   - Skill Level    : difficulty level 1-64 (default 64 = full strength)
//   - Threads        : number of search threads (default = available cores)
//   - Ponder         : enables pondering mode (default = true)
// =============================================================================

pub mod parser;

use std::io::{self, BufRead, Write};
use std::sync::{Arc, mpsc};
use std::sync::atomic::Ordering;
use std::time::Duration;
use crate::game::Game;
use crate::search::{SearchEngine, SearchConfig, SearchResult, elo_to_skill_level, ELO_MIN, ELO_MAX};
use crate::search::killers::KillerMoves;
use crate::search::history::HistoryTable;
use crate::search::countermove::CountermoveTable;
use crate::search::continuation_history::ContinuationHistoryTable;
use crate::utils::types::Move;
use parser::{parse_command, parse_move_uci, UciCommand};

/// Engine name and version.
pub const ENGINE_NAME:    &str = "Vendetta Chess Engine";
pub const ENGINE_VERSION: &str = "1.1.2";
/// Project author.
pub const ENGINE_AUTHOR:  &str = "Fabrice Garcia";

/// Main UCI engine.
pub struct UciEngine {
    /// Current game.
    game: Game,
    /// Search engine.
    search_engine: SearchEngine,
    /// Current difficulty level (1-64), driven by the custom "Skill Level" option.
    skill_level: u8,
    /// Transposition table size in MB.
    hash_size_mb: usize,

    // --- Standard UCI strength limitation ---

    /// true if "UCI_LimitStrength" is enabled: in that case, `elo` takes
    /// precedence over `skill_level` to determine playing strength (see the
    /// Go command). Allows standard GUIs/platforms to limit
    /// Vendetta Chess Engine without knowing the custom "Skill Level" option.
    limit_strength: bool,
    /// Target strength in Elo when `limit_strength` is active ("UCI_Elo" option).
    elo: u16,
    /// Number of principal variations to display ("MultiPV" option).
    /// 1 = standard behavior (a single best line).
    multipv: usize,
    /// Safety margin (ms) subtracted from the time budget ("Move
    /// Overhead" option) — compensates for GUI/network latency, avoids losses on
    /// time online/in tournaments. 50 ms by default (value of the former
    /// fixed margin hard-coded in compute_time_limit(), now adjustable).
    move_overhead_ms: u64,
    /// true if "UCI_AnalyseMode" is enabled: always forces the best
    /// move (skill_level = 64), regardless of "Skill Level" or
    /// "UCI_LimitStrength" — an analysis tool must never receive
    /// a deliberate error from the difficulty level system.
    analyse_mode: bool,
    /// Contempt factor in centipawns (UCI "Contempt" option). 0 by
    /// default = unchanged behavior (no penalty on drawn positions).
    /// A positive value slightly penalizes drawishness from the point
    /// of view of the side the engine is currently playing — useful against a
    /// weaker opponent, to keep trying to win rather
    /// than settle for a draw. See
    /// alphabeta.rs::draw_score() for the exact mechanism.
    contempt: i32,
    /// Handle of the currently running search thread (None if inactive).
    search_handle: Option<std::thread::JoinHandle<SearchResult>>,

    // --- Pondering state ---

    /// true when the ongoing search is in ponder mode
    /// (thinking on the opponent's time).
    is_pondering: bool,
    /// UCI configuration saved during "go ponder".
    /// Used to launch the normal search on "ponderhit".
    ponder_config: Option<SearchConfig>,

    // --- Debug mode ---

    /// true if UCI debug mode is enabled ("debug on").
    /// When active, the engine may emit additional "info string"
    /// messages to help with diagnostics.
    debug_mode: bool,
}

impl UciEngine {
    /// Creates a new UCI engine.
    pub fn new() -> UciEngine {
        UciEngine {
            game:          Game::new(),
            search_engine: SearchEngine::new(),
            skill_level:   64,
            hash_size_mb:  32,
            limit_strength: false,
            elo:            ELO_MAX,
            multipv:        1,
            move_overhead_ms: 50,
            analyse_mode:     false,
            contempt:         0,
            search_handle: None,
            is_pondering:  false,
            ponder_config: None,
            debug_mode:    false,
        }
    }

    /// Main UCI loop.
    ///
    /// Architecture:
    ///   - Dedicated thread for stdin → mpsc channel (recv_timeout 5 ms).
    ///   - Non-blocking main loop → responsive to commands in real time.
    ///   - Search in a separate thread (spawn_search).
    ///   - check_search_done() detects the end of the search and emits bestmove.
    pub fn run(&mut self) {
        let stdout = io::stdout();

        // Thread dedicated to reading stdin (non-blocking for the main loop).
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l)  => { if tx.send(l).is_err() { break; } }
                    Err(_) => break,
                }
            }
        });

        'main: loop {
            // Check whether the ongoing search has finished → emit bestmove
            self.check_search_done(&stdout);

            // Wait for a UCI command (timeout to stay responsive)
            let line = match rx.recv_timeout(Duration::from_millis(5)) {
                Ok(l)                                     => l,
                Err(mpsc::RecvTimeoutError::Timeout)      => continue 'main,
                Err(mpsc::RecvTimeoutError::Disconnected) => break 'main,
            };

            match parse_command(&line) {

                UciCommand::Uci => {
                    self.cmd_uci();
                }

                UciCommand::IsReady => {
                    println!("readyok");
                }

                UciCommand::UciNewGame => {
                    self.cmd_new_game();
                }

                UciCommand::Position { fen, moves } => {
                    // If a ponder is in progress on a different position,
                    // it must be stopped (the GUI is sending a new position).
                    self.abort_ponder();
                    self.cmd_position(fen, moves);
                }

                UciCommand::Go(mut config) => {
                    // Priority (from highest to lowest):
                    //   1. UCI_AnalyseMode: always the best move, never
                    //      a deliberate error — an analysis tool must
                    //      never receive a deliberately weakened response.
                    //   2. UCI_LimitStrength + UCI_Elo: standard
                    //      limiting mechanism, for GUIs/platforms that don't
                    //      know the custom "Skill Level" option.
                    //   3. "Skill Level": custom option, default setting.
                    config.skill_level = if self.analyse_mode {
                        64
                    } else if self.limit_strength {
                        elo_to_skill_level(self.elo)
                    } else {
                        self.skill_level
                    };
                    config.multipv       = self.multipv;
                    config.move_overhead = self.move_overhead_ms;
                    config.contempt      = self.contempt;

                    // Stop any ongoing search (normal or ponder)
                    self.stop_current_search();

                    // Systematically reset the ponder state before any new Go,
                    // even if it isn't a ponder Go (defense against non-compliant GUIs).
                    self.is_pondering  = false;
                    self.ponder_config = None;

                    if config.ponder {
                        // --- Ponder mode ---
                        // Save the config for use during ponderhit.
                        // Start an INFINITE search on the expected opponent position.
                        self.ponder_config = Some(config.clone());
                        self.is_pondering  = true;

                        // The search thread runs in infinite mode:
                        // it will ignore time limits and wait for stop/ponderhit.
                        let mut ponder_cfg     = config;
                        ponder_cfg.infinite    = true;
                        ponder_cfg.ponder      = false; // The thread doesn't need to know this
                        ponder_cfg.wtime       = None;  // No time management in ponder mode
                        ponder_cfg.btime       = None;
                        ponder_cfg.movetime    = None;

                        match self.spawn_search(ponder_cfg) {
                            Ok(handle) => self.search_handle = Some(handle),
                            Err(_) => {
                                // Thread could not be created: pondering is an
                                // INFINITE search, so it CANNOT be done
                                // synchronously (it would block forever). We
                                // silently abandon the ponder; the GUI
                                // will then send an actual "go" (or "ponderhit").
                                self.is_pondering  = false;
                                self.ponder_config = None;
                            }
                        }
                    } else {
                        // --- Normal mode ---
                        match self.spawn_search(config.clone()) {
                            Ok(handle) => self.search_handle = Some(handle),
                            Err(_) => {
                                // ROBUST FALLBACK: the OS cannot create the search
                                // thread. Rather than crash, we search
                                // SYNCHRONOUSLY on the current thread, forced to 1
                                // thread (SMP spawns would fail too). The
                                // UCI loop is blocked for the duration of the search,
                                // but the time limit is respected → a
                                // bestmove is indeed emitted.
                                let board = self.game.board.clone();
                                let tt    = Arc::clone(&self.search_engine.tt);
                                let stop  = Arc::clone(&self.search_engine.stop_flag);
                                stop.store(false, Ordering::SeqCst);
                                let result = run_search(tt, stop, 1, board, config);
                                self.emit_bestmove(&result, &stdout);
                            }
                        }
                    }
                }

                UciCommand::PonderHit => {
                    // The opponent played the predicted move: we exit ponder mode
                    // and start a normal search with the saved parameters.
                    if self.is_pondering {
                        // 1. Stop the ponder search (the TT stays warm)
                        self.search_engine.stop();
                        if let Some(h) = self.search_handle.take() {
                            let _ = h.join(); // Wait for a clean end (~a few ms)
                        }
                        self.is_pondering = false;

                        // 2. Start the normal search with the "go ponder" parameters
                        if let Some(mut real_cfg) = self.ponder_config.take() {
                            real_cfg.ponder = false;
                            // Reset the stop_flag before starting the new search
                            self.search_engine.stop_flag.store(false, Ordering::SeqCst);
                            match self.spawn_search(real_cfg.clone()) {
                                Ok(handle) => self.search_handle = Some(handle),
                                Err(_) => {
                                    // Same robust fallback as for a normal "go":
                                    // synchronous single-thread search rather than panic.
                                    let board = self.game.board.clone();
                                    let tt    = Arc::clone(&self.search_engine.tt);
                                    let stop  = Arc::clone(&self.search_engine.stop_flag);
                                    stop.store(false, Ordering::SeqCst);
                                    let result = run_search(tt, stop, 1, board, real_cfg);
                                    self.emit_bestmove(&result, &stdout);
                                }
                            }
                        }
                    }
                    // If ponderhit arrives without a prior go ponder: it is ignored.
                }

                UciCommand::Stop => {
                    // Stop any ongoing search.
                    // In ponder mode: the GUI has decided to stop (opponent played something else).
                    // bestmove will be emitted by check_search_done() on the next iteration.
                    self.is_pondering = false;
                    self.ponder_config = None;
                    self.search_engine.stop();
                }

                UciCommand::Debug { on } => {
                    self.debug_mode = on;
                    if self.debug_mode {
                        println!("info string debug mode enabled");
                    }
                }

                UciCommand::Register => {
                    // Vendetta Chess Engine has NO anti-copy protection: it
                    // accepts the command without doing anything. The spec forbids
                    // issuing a "registration" response if the engine does not
                    // need one — hence no-op. Reported only in debug mode.
                    if self.debug_mode {
                        println!("info string register ignoré (aucune protection anti-copie)");
                    }
                }

                UciCommand::SetOption { name, value } => {
                    self.cmd_setoption(&name, &value);
                }

                UciCommand::Quit => {
                    // Clean shutdown before quitting
                    self.search_engine.stop();
                    if let Some(h) = self.search_handle.take() {
                        let _ = h.join();
                    }
                    break 'main;
                }

                UciCommand::Unknown => {
                    // Silently ignore (required by the UCI spec)
                }
            }

            let _ = stdout.lock().flush();
        }
    }

    // =========================================================================
    // Search thread management
    // =========================================================================

    /// Stops the current search and waits for the thread to finish.
    /// Does not touch is_pondering (caller's responsibility).
    fn stop_current_search(&mut self) {
        if self.search_handle.is_some() {
            self.search_engine.stop();
            if let Some(h) = self.search_handle.take() {
                let _ = h.join();
            }
        }
    }

    /// Stops an ongoing ponder without emitting bestmove.
    /// Used when the GUI sends a new position during a ponder.
    fn abort_ponder(&mut self) {
        if self.is_pondering {
            self.search_engine.stop();
            if let Some(h) = self.search_handle.take() {
                let _ = h.join();
            }
            self.is_pondering  = false;
            self.ponder_config = None;
        }
    }

    /// Checks whether the search has finished and emits bestmove if so.
    ///
    /// In ponder mode, this function is called at every iteration but
    /// NEVER emits bestmove while is_pondering is true: the ponder
    /// search runs until ponderhit or stop.
    fn check_search_done(&mut self, stdout: &io::Stdout) {
        // In active ponder mode, the search runs freely — we do nothing.
        if self.is_pondering {
            // Safety check: if the thread finishes on its own during
            // a ponder (impossible in infinite mode, but defense in depth), we clean up
            // without emitting bestmove (this is not a normal end of search).
            let thread_done = self.search_handle
                .as_ref()
                .map(|h| h.is_finished())
                .unwrap_or(false);
            if thread_done {
                if let Some(h) = self.search_handle.take() {
                    let _ = h.join();
                }
            }
            return;
        }

        // Normal mode: emit bestmove as soon as the thread has finished.
        if let Some(handle) = self.search_handle.take() {
            if handle.is_finished() {
                match handle.join() {
                    Ok(result) => {
                        self.emit_bestmove(&result, stdout);
                    }
                    Err(_) => {
                        eprintln!("info string Erreur interne : le thread de recherche a planté");
                        println!("bestmove (none)");
                        let _ = stdout.lock().flush();
                    }
                }
            } else {
                // Search still in progress: put the handle back
                self.search_handle = Some(handle);
            }
        }
    }

    /// Emits "bestmove <move> [ponder <expected_move>]" on stdout.
    fn emit_bestmove(&self, result: &SearchResult, stdout: &io::Stdout) {
        if result.best_move.is_null() {
            // No legal move (checkmate or stalemate).
            // We emit "(none)" rather than "0000": the "0000" notation is not
            // standard UCI and some GUIs (Cutechess, Fritz) reject it or
            // disconnect. "(none)" is the universally accepted convention.
            println!("bestmove (none)");
        } else if !result.ponder_move.is_null() {
            // Include the predicted reply move so the GUI can start a ponder
            println!("bestmove {} ponder {}",
                result.best_move.to_uci(),
                result.ponder_move.to_uci());
        } else {
            println!("bestmove {}", result.best_move.to_uci());
        }
        let _ = stdout.lock().flush();
    }

    /// Launches the search in a dedicated thread and returns its JoinHandle.
    ///
    /// Returns `Err` if the OS refuses to create the thread (resources exhausted,
    /// catastrophic case) INSTEAD of panicking: the caller (Go command)
    /// then falls back to a synchronous path. Normal behavior strictly
    /// unchanged — the `Ok` path is exactly the old one.
    fn spawn_search(
        &mut self,
        config: SearchConfig,
    ) -> std::io::Result<std::thread::JoinHandle<SearchResult>> {
        let board       = self.game.board.clone();
        let tt          = Arc::clone(&self.search_engine.tt);
        let stop        = Arc::clone(&self.search_engine.stop_flag);
        let num_threads = self.search_engine.num_threads;

        // Reset the stop signal before launching.
        stop.store(false, Ordering::SeqCst);

        // 8 MiB stack (instead of the ~2 MiB default): this thread carries out the
        // MAIN search at full depth, the most exposed to deep recursion
        // + to move lists allocated on the stack (MoveList, scored captures).
        // The body is delegated to run_search(), shared with the synchronous fallback.
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || run_search(tt, stop, num_threads, board, config))
    }

    // =========================================================================
    // UCI command handlers
    // =========================================================================

    /// "uci" command: identify the engine and list the options.
    fn cmd_uci(&self) {
        println!("id name {} {}", ENGINE_NAME, ENGINE_VERSION);
        println!("id author {}", ENGINE_AUTHOR);
        println!();

        println!("option name Hash type spin default 32 min 1 max 32768");
        println!("option name Skill Level type spin default 64 min 1 max 64");
        println!("option name Ponder type check default true");
        println!("option name Debug type check default false");

        let default_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        println!("option name Threads type spin default {} min 1 max 768", default_threads);

        // --- Standard UCI options (interoperability with GUIs/platforms) ---
        // UCI_LimitStrength + UCI_Elo: standard mechanism for limiting strength,
        // an alternative to "Skill Level" for tools that don't know about
        // this custom option (see search::elo_to_skill_level).
        println!("option name UCI_LimitStrength type check default false");
        println!("option name UCI_Elo type spin default {} min {} max {}", ELO_MAX, ELO_MIN, ELO_MAX);
        // MultiPV: number of principal variations displayed (1 = standard default).
        println!("option name MultiPV type spin default 1 min 1 max 218");

        // Move Overhead: safety margin (ms) against time losses
        // due to GUI/network latency — replaces the old fixed margin of
        // 50 ms hard-coded in compute_time_limit() (same default value).
        println!("option name Move Overhead type spin default 50 min 0 max 5000");

        // Clear Hash: button that manually clears the transposition table,
        // without having to send "ucinewgame" (which also resets killers/history).
        println!("option name Clear Hash type button");

        // UCI_AnalyseMode: always forces the best move (skill_level=64),
        // takes priority over "Skill Level" and "UCI_LimitStrength" — see the
        // Go command. Useful for analysis tools that never want
        // deliberate error from the engine.
        println!("option name UCI_AnalyseMode type check default false");

        // Contempt: slightly penalizes drawn positions (all causes)
        // from the point of view of the side the engine is currently playing — useful
        // against a weaker opponent (keep looking for a win
        // rather than settling for a draw). 0 = behavior
        // unchanged (default). Standard convention: centipawns, range
        // -100 to 100. See alphabeta.rs::draw_score() for the mechanism.
        println!("option name Contempt type spin default 0 min -100 max 100");

        // UCI_EngineAbout: cosmetic information about the engine, with no
        // functional effect — UCI convention for displaying author/license/link.
        println!(
            "option name UCI_EngineAbout type string default {} {} - Auteur initial: {} - MIT License",
            ENGINE_NAME, ENGINE_VERSION, ENGINE_AUTHOR
        );
        println!();

        println!("uciok");
    }

    /// "ucinewgame" command: reset for a new game.
    fn cmd_new_game(&mut self) {
        // Stop any ongoing ponder or search before the reset
        self.abort_ponder();
        self.stop_current_search();
        self.game.reset();
        self.search_engine.new_game();
    }

    /// "position" command: set the current position.
    fn cmd_position(&mut self, fen: Option<String>, moves: Vec<String>) {
        if let Some(fen_str) = fen {
            match Game::from_fen(&fen_str) {
                Ok(game) => { self.game = game; }
                Err(e)   => {
                    eprintln!("info string Erreur FEN : {}", e);
                    return;
                }
            }
        } else {
            self.game = Game::new();
        }

        for mv_str in &moves {
            if let Some(mv) = parse_move_uci(mv_str, &mut self.game.board) {
                self.game.make_move(mv);
            } else {
                eprintln!("info string Coup invalide ignoré : {}", mv_str);
                break;
            }
        }
    }

    /// "setoption" command: configure an engine option.
    fn cmd_setoption(&mut self, name: &str, value: &str) {
        match name {
            "Hash" => {
                if let Ok(size) = value.parse::<usize>() {
                    // Cap at 32 GB (32768 MB): generous for any
                    // modern machine, without changing the default (32 MB) that protects
                    // modest setups.
                    let requested = size.clamp(1, 32768);

                    // GRACEFUL FALLBACK (Vendetta Chess Engine's robustness priority):
                    // we try the requested size, then HALVE IT repeatedly as long
                    // as the allocation fails — instead of crashing. The TT is
                    // allocated as one block: a Hash setting larger than the available
                    // RAM must NEVER kill the engine. As a last
                    // resort (even the minimum size fails), we keep the
                    // current table. An "info string" message informs the GUI.
                    let mut try_size = requested;
                    loop {
                        if let Some(tt) =
                            crate::search::transposition::TranspositionTable::try_new(try_size)
                        {
                            if try_size != requested {
                                eprintln!(
                                    "info string Hash {} Mo impossible (mémoire insuffisante) \
                                     — repli sur {} Mo",
                                    requested, try_size
                                );
                            }
                            self.hash_size_mb = try_size;
                            self.search_engine.tt = Arc::new(tt);
                            break;
                        }
                        if try_size <= 1 {
                            eprintln!(
                                "info string Hash {} Mo impossible — table de transposition \
                                 actuelle conservée",
                                requested
                            );
                            break;
                        }
                        try_size /= 2;
                    }
                }
            }
            "Skill Level" => {
                if let Ok(level) = value.parse::<u8>() {
                    self.skill_level = level.clamp(1, 64);
                }
            }
            "Threads" => {
                if let Ok(n) = value.parse::<usize>() {
                    self.search_engine.num_threads = n.clamp(1, 768);
                }
            }
            "Ponder" => {
                // Informational option — pondering is always supported.
                // We accept the option for UCI conformity, no action needed.
            }
            "UCI_LimitStrength" => {
                self.limit_strength = value.eq_ignore_ascii_case("true");
            }
            "UCI_Elo" => {
                if let Ok(elo) = value.parse::<u16>() {
                    self.elo = elo.clamp(ELO_MIN, ELO_MAX);
                }
            }
            "MultiPV" => {
                if let Ok(n) = value.parse::<usize>() {
                    self.multipv = n.clamp(1, 218);
                }
            }
            "Move Overhead" => {
                if let Ok(ms) = value.parse::<u64>() {
                    self.move_overhead_ms = ms.min(5000);
                }
            }
            "Clear Hash" => {
                // Option of type "button": no value, the mere arrival of
                // the command triggers the action. Clears the TT immediately —
                // a partial equivalent to "ucinewgame", but without touching
                // killer moves / history (which are only reset between
                // games, not while thinking).
                self.search_engine.tt.clear();
            }
            "UCI_AnalyseMode" => {
                self.analyse_mode = value.eq_ignore_ascii_case("true");
            }
            "Contempt" => {
                if let Ok(cp) = value.parse::<i32>() {
                    self.contempt = cp.clamp(-100, 100);
                }
            }
            "UCI_EngineAbout" => {
                // Read-only informational option — declared for
                // UCI conformity (any announced option must be able to be "set" without
                // error), but with no effect: it is the engine that informs the
                // GUI via this option, not the other way around.
            }
            _ => {
                eprintln!("info string Option inconnue : {}", name);
            }
        }
    }
}

impl Default for UciEngine {
    fn default() -> Self {
        UciEngine::new()
    }
}

/// Runs a full search (including MultiPV) and returns the BEST
/// result (results[0]).
///
/// Function shared by two callers:
///   - the dedicated search thread (NORMAL case, via spawn_search);
///   - the SYNCHRONOUS fallback triggered if creating this thread fails
///     (see the Go command) — then called with `num_threads = 1`.
///
/// This extraction avoids any duplication: a single definition of the search
/// flow + MultiPV display. The behavior of the normal case is
/// strictly identical to the old inline closure.
fn run_search(
    tt:          Arc<crate::search::transposition::TranspositionTable>,
    stop:        Arc<std::sync::atomic::AtomicBool>,
    num_threads: usize,
    mut board:   crate::board::state::Board,
    config:      SearchConfig,
) -> SearchResult {
    let mut engine = SearchEngine {
        tt,
        killers:      KillerMoves::new(),
        history:      HistoryTable::new(),
        countermoves: CountermoveTable::new(),
        cont_history: ContinuationHistoryTable::new(),
        num_threads,
        stop_flag:    stop,
    };

    // search_multipv() is strictly equivalent to search() when
    // config.multipv <= 1 (default case) — zero change in behavior
    // for a normal game.
    let results = engine.search_multipv(&mut board, &config);

    // In MultiPV (>1 line), a summary per variation, from the best (1)
    // to the worst. (Known limitation: intermediate "info depth" entries do
    // not carry the "multipv" field; no impact on the final result.)
    if results.len() > 1 {
        for (i, r) in results.iter().enumerate() {
            let nps = crate::search::compute_nps(r.nodes, r.time_ms);
            println!(
                "info multipv {} depth {} score {} nodes {} nps {} time {} pv {}",
                i + 1, r.depth, crate::search::format_score(r.score),
                r.nodes, nps, r.time_ms, r.best_move.to_uci(),
            );
        }
        let _ = io::stdout().lock().flush();
    }

    // bestmove/ponder always rely on the BEST line (results[0]).
    results.into_iter().next().unwrap_or(SearchResult {
        best_move: Move::NULL, ponder_move: Move::NULL,
        score: 0, depth: 0, nodes: 0, time_ms: 0,
    })
}
