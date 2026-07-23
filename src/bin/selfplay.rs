// =============================================================================
// Vendetta Chess Engine — src/bin/selfplay.rs
//
// Role: Measure whether a change to the engine ADDS Elo, via a test
//        SPRT in internal self-play (the engine plays against itself, two
//        variants A and B, without UCI or subprocesses).
//
// Workflow (full instructions: COMMENT_TESTER_SPRT.md):
//   1. A `key = value` config file describes the test (prepared by you).
//   2. This binary plays A against B in fast games (fixed nodes per move),
//      from random openings (alternating colors for fairness).
//   3. The SPRT stops on its own as soon as it concludes (PASS / FAIL), or at the cap.
//   4. A `key = value` report is written (and rewritten regularly = autosave).
//
// Clean stop: create a `STOP` file in the current directory
//   (`touch STOP`) → the program finishes the current game, writes a
//   final report marked "INTERRUPTED", deletes STOP, and exits.
//
// Launch:
//   cargo run --release --bin selfplay -- <config.txt>
//   (default: selfplay_config.txt)
//
// Zero external dependencies (custom parsing, custom PRNG, custom SPRT).
// =============================================================================

use std::env;
use std::fs;
use std::path::Path;
use std::sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::thread;
use std::time::Duration;

use vendetta_chess_engine::board::state::Board;
use vendetta_chess_engine::board::bitboard::init_attack_tables;
use vendetta_chess_engine::search::transposition::TranspositionTable;
use vendetta_chess_engine::search::killers::KillerMoves;
use vendetta_chess_engine::search::history::HistoryTable;
use vendetta_chess_engine::search::countermove::CountermoveTable;
use vendetta_chess_engine::search::continuation_history::ContinuationHistoryTable;
use vendetta_chess_engine::search::alphabeta::alpha_beta;
use vendetta_chess_engine::search::SearchInfo;
use vendetta_chess_engine::utils::types::{Color, Move, SCORE_MATE};
use vendetta_chess_engine::moves::generate_legal_moves;
use vendetta_chess_engine::game::Game;
use vendetta_chess_engine::game::rules::GameResult;

// --- Internal constants -----------------------------------------------------
const SELFPLAY_TT_MB:     usize = 8;   // small TT per side (shallow search)
const SELFPLAY_MAX_DEPTH: i32   = 64;  // bound of the iterative deepening (the node limit cuts it off before that)
const OPENING_PLIES:      usize = 8;   // random half-moves in the opening (variety)
const MAX_PLIES:          usize = 400; // safeguard against infinite games → draw
const POLL_MS:            u64   = 500; // polling interval of the main thread (ms)
const MAX_CONCURRENCY:    usize = 64;  // safeguard: upper bound on parallelism (anti-OOM)
const STOP_FILE:          &str  = "STOP";

// =============================================================================
// Configuration (key = value file)
// =============================================================================

struct Config {
    nodes_a:     u64,   // nodes per move, variant A (reference)
    nodes_b:     u64,   // nodes per move, variant B (candidate)
    improving_a: bool,  // "improving" feature enabled for A?
    improving_b: bool,  // "improving" feature enabled for B?
    futility_a:  bool,  // Futility Pruning per move enabled for A?
    futility_b:  bool,  // Futility Pruning per move enabled for B?
    lmr_a:       bool,  // enriched LMR (±1 adjustments) enabled for A?
    lmr_b:       bool,  // enriched LMR (±1 adjustments) enabled for B?
    correction_a: bool, // Correction History enabled for A?
    correction_b: bool, // Correction History enabled for B?
    king_attack_a: bool, // King safety by attack enabled for A?
    king_attack_b: bool, // King safety by attack enabled for B?
    games_max:   u64,   // game cap (stop if reached without conclusion)
    elo0:        f64,   // lower SPRT bound
    elo1:        f64,   // upper SPRT bound
    alpha:       f64,   // type I error risk
    beta:        f64,   // type II error risk
    concurrency: usize, // number of games played IN PARALLEL (0/1 = sequential)
    report:      String,// report file
}

impl Config {
    fn defaults() -> Config {
        Config {
            nodes_a:     20_000,
            nodes_b:     20_000,
            improving_a: true,
            improving_b: true,
            futility_a:  true,
            futility_b:  true,
            lmr_a:       true,
            lmr_b:       true,
            correction_a: true,
            correction_b: true,
            king_attack_a: true,
            king_attack_b: true,
            games_max:   4_000,
            elo0:        0.0,
            elo1:        5.0,
            alpha:       0.05,
            beta:        0.05,
            concurrency: 4,
            report:      "rapport_selfplay.txt".to_string(),
        }
    }
}

fn parse_bool(v: &str, default: bool) -> bool {
    match v.to_ascii_lowercase().as_str() {
        "true" | "1" | "oui" | "on"  => true,
        "false" | "0" | "non" | "off" => false,
        _ => default,
    }
}

fn parse_config(path: &str) -> Config {
    let mut c = Config::defaults();
    match fs::read_to_string(path) {
        Ok(content) => {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let k = k.trim();
                    let v = v.trim();
                    match k {
                        "nodes_a"     => c.nodes_a     = v.parse().unwrap_or(c.nodes_a),
                        "nodes_b"     => c.nodes_b     = v.parse().unwrap_or(c.nodes_b),
                        "improving_a" => c.improving_a = parse_bool(v, c.improving_a),
                        "improving_b" => c.improving_b = parse_bool(v, c.improving_b),
                        "futility_a"  => c.futility_a  = parse_bool(v, c.futility_a),
                        "futility_b"  => c.futility_b  = parse_bool(v, c.futility_b),
                        "lmr_a"       => c.lmr_a       = parse_bool(v, c.lmr_a),
                        "lmr_b"       => c.lmr_b       = parse_bool(v, c.lmr_b),
                        "correction_a" => c.correction_a = parse_bool(v, c.correction_a),
                        "correction_b" => c.correction_b = parse_bool(v, c.correction_b),
                        "king_attack_a" => c.king_attack_a = parse_bool(v, c.king_attack_a),
                        "king_attack_b" => c.king_attack_b = parse_bool(v, c.king_attack_b),
                        "games_max"   => c.games_max   = v.parse().unwrap_or(c.games_max),
                        "elo0"        => c.elo0        = v.parse().unwrap_or(c.elo0),
                        "elo1"        => c.elo1        = v.parse().unwrap_or(c.elo1),
                        "alpha"       => c.alpha       = v.parse().unwrap_or(c.alpha),
                        "beta"        => c.beta        = v.parse().unwrap_or(c.beta),
                        "concurrency" => c.concurrency = v.parse().unwrap_or(c.concurrency),
                        "report"      => c.report      = v.to_string(),
                        _ => eprintln!("⚠ clé inconnue ignorée : {}", k),
                    }
                }
            }
        }
        Err(_) => {
            eprintln!("⚠ config '{}' introuvable — valeurs par défaut utilisées.", path);
        }
    }
    c
}

// =============================================================================
// custom PRNG (for reproducible random openings)
// =============================================================================

fn next_rand(state: &mut u64) -> u64 {
    // LCG + final mixing (splitmix-like). Deterministic for a given seed.
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut x = *state;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x
}

// =============================================================================
// Search state of a side (persistent across an entire game)
// =============================================================================

struct SideState {
    tt:                TranspositionTable,
    killers:           KillerMoves,
    history:           HistoryTable,
    countermoves:      CountermoveTable,
    cont_history:      ContinuationHistoryTable,
    node_limit:         u64,
    disable_improving:   bool,
    disable_futility:    bool,
    disable_lmr_tweaks:  bool,
    disable_correction:  bool,
    disable_king_attack: bool,
}

impl SideState {
    fn new(node_limit: u64, disable_improving: bool, disable_futility: bool, disable_lmr_tweaks: bool, disable_correction: bool, disable_king_attack: bool) -> SideState {
        SideState {
            tt:                TranspositionTable::new(SELFPLAY_TT_MB),
            killers:           KillerMoves::new(),
            history:           HistoryTable::new(),
            countermoves:      CountermoveTable::new(),
            cont_history:      ContinuationHistoryTable::new(),
            node_limit,
            disable_improving,
            disable_futility,
            disable_lmr_tweaks,
            disable_correction,
            disable_king_attack,
        }
    }

    /// Finds the best move for the current position, within the
    /// fixed node limit. Reuses its tables (TT/history…) from one move to the next.
    fn best_move(&mut self, board: &mut Board) -> Move {
        let mut info = SearchInfo::new_with_stop(
            Duration::from_secs(3600),
            Arc::new(AtomicBool::new(false)),
        );
        info.max_nodes        = Some(self.node_limit);
        info.toggles.disable_improving   = self.disable_improving;
        info.toggles.disable_futility    = self.disable_futility;
        info.toggles.disable_lmr_tweaks  = self.disable_lmr_tweaks;
        info.toggles.disable_correction  = self.disable_correction;
        info.toggles.disable_king_attack = self.disable_king_attack;

        let mut chosen = Move::NULL;
        for depth in 1..=SELFPLAY_MAX_DEPTH {
            if info.should_stop() {
                break;
            }
            info.best_move = Move::NULL;
            alpha_beta(
                board, depth, -SCORE_MATE, SCORE_MATE, 0,
                &self.tt, &mut self.killers, &mut self.history,
                &mut self.countermoves, &mut self.cont_history, Move::NULL,
                &mut info, Move::NULL, &[],
            );
            // If the node limit cut off WHILE a depth was in progress, we keep the
            // move from the last COMPLETE depth (already in `chosen`).
            if info.should_stop() {
                break;
            }
            chosen = info.best_move;
        }

        if chosen.is_null() {
            chosen = info.best_move; // partial result of an interrupted depth
        }
        if chosen.is_null() {
            // Last-resort safety net (should not happen if the position is not terminal).
            let legal = generate_legal_moves(board);
            if !legal.is_empty() {
                chosen = legal[0];
            }
        }
        chosen
    }
}

// =============================================================================
// Course of a game
// =============================================================================

/// Builds a reproducible random opening (seed = pair number).
fn random_opening(seed: u64) -> Game {
    let mut game = Game::new();
    let mut rng = seed ^ 0x9E3779B97F4A7C15;
    for _ in 0..OPENING_PLIES {
        let legal = generate_legal_moves(&mut game.board);
        if legal.is_empty() {
            break; // terminal position reached (rare) — we stop there
        }
        let idx = (next_rand(&mut rng) as usize) % legal.len();
        game.make_move(legal[idx]);
    }
    game
}

/// Plays a complete game from `game`. `a_is_white` indicates A's color.
/// Returns the result FROM B's POINT OF VIEW (the candidate):
///   +1 = B wins, 0 = draw, -1 = B loses.
fn play_out(mut game: Game, a_is_white: bool, side_a: &mut SideState, side_b: &mut SideState) -> i32 {
    let mut plies = 0usize;
    loop {
        match game.result() {
            GameResult::Ongoing => {}
            GameResult::Checkmate => {
                // The side to move is checkmated → it loses.
                let loser_is_white = game.board.side_to_move == Color::White;
                let b_is_white     = !a_is_white;
                let b_loses        = loser_is_white == b_is_white;
                return if b_loses { -1 } else { 1 };
            }
            _ => return 0, // any draw (50-move rule, repetition, material, stalemate)
        }

        if plies >= MAX_PLIES {
            return 0; // safeguard: game too long → draw
        }

        let stm_is_white = game.board.side_to_move == Color::White;
        let a_to_move    = stm_is_white == a_is_white;

        let mv = if a_to_move {
            side_a.best_move(&mut game.board)
        } else {
            side_b.best_move(&mut game.board)
        };

        if mv.is_null() {
            return 0; // safety: no move found (should not happen)
        }
        game.make_move(mv);
        plies += 1;
    }
}

/// Plays ONE complete, self-contained game (creates its own sides). Serves as the unit
/// of work for parallel threads. `seed` fixes the opening, `a_white` the
/// color of A. Returns +1/0/-1 from B's point of view.
fn play_one_game(seed: u64, a_white: bool, cfg: &Config) -> i32 {
    let game = random_opening(seed);
    let mut side_a = SideState::new(cfg.nodes_a, !cfg.improving_a, !cfg.futility_a, !cfg.lmr_a, !cfg.correction_a, !cfg.king_attack_a);
    let mut side_b = SideState::new(cfg.nodes_b, !cfg.improving_b, !cfg.futility_b, !cfg.lmr_b, !cfg.correction_b, !cfg.king_attack_b);
    play_out(game, a_white, &mut side_a, &mut side_b)
}

// =============================================================================
// SPRT statistics (normal model, from B = candidate's point of view)
// =============================================================================

fn score_from_elo(elo: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf(-elo / 400.0))
}

fn elo_from_score(s: f64) -> f64 {
    let s = s.clamp(1e-6, 1.0 - 1e-6);
    -400.0 * (1.0 / s - 1.0).log10()
}

/// Log-likelihood ratio (normal approximation).
fn llr(w: u64, d: u64, l: u64, elo0: f64, elo1: f64) -> f64 {
    let n = (w + d + l) as f64;
    if n == 0.0 {
        return 0.0;
    }
    let sum_x  = w as f64 + 0.5 * d as f64;        // Σ score
    let sum_x2 = w as f64 + 0.25 * d as f64;       // Σ score²
    let mean   = sum_x / n;
    let var    = sum_x2 / n - mean * mean;          // variance per game
    if var <= 1e-9 {
        return 0.0; // no information (all draws, etc.)
    }
    let s0 = score_from_elo(elo0);
    let s1 = score_from_elo(elo1);
    (s1 - s0) / var * (sum_x - n * (s0 + s1) / 2.0)
}

/// Estimated Elo and half-margin (95 %).
fn elo_and_margin(w: u64, d: u64, l: u64) -> (f64, f64) {
    let n = (w + d + l) as f64;
    if n == 0.0 {
        return (0.0, 0.0);
    }
    let sum_x  = w as f64 + 0.5 * d as f64;
    let sum_x2 = w as f64 + 0.25 * d as f64;
    let mean   = sum_x / n;
    let var    = (sum_x2 / n - mean * mean).max(0.0);
    let se     = (var / n).sqrt();
    let elo    = elo_from_score(mean);
    let lo     = elo_from_score(mean - 1.96 * se);
    let hi     = elo_from_score(mean + 1.96 * se);
    (elo, (hi - lo) / 2.0)
}

// =============================================================================
// Display and reporting
// =============================================================================

fn print_progress(cfg: &Config, w: u64, d: u64, l: u64, llr_val: f64, upper: f64, lower: f64) {
    let n = w + d + l;
    let (elo, margin) = elo_and_margin(w, d, l);
    let pct_cap = (n as f64 / cfg.games_max as f64 * 100.0).min(100.0);
    let (bound, dir) = if llr_val >= 0.0 { (upper, "PASS") } else { (lower, "FAIL") };
    let pct_verdict = if bound != 0.0 { (llr_val / bound * 100.0).clamp(0.0, 100.0) } else { 0.0 };
    println!(
        "[{:5.1}%]  {}/{} parties  |  B {}-{}-{} (G-N-P)  |  Elo {:+.1} ±{:.1}  |  LLR {:.2}/{:.2} → {} ({:.0}%)",
        pct_cap, n, cfg.games_max, w, d, l, elo, margin, llr_val, bound, dir, pct_verdict
    );
}

// Pure SERIALIZATION function: each parameter is a distinct, named
// field of the report. Grouping them into a struct solely to satisfy the
// lint would add no clarity here — hence the explicit and
// justified allowance to exceed the argument-count threshold.
#[allow(clippy::too_many_arguments)]
fn write_report(cfg: &Config, w: u64, d: u64, l: u64, llr_val: f64, upper: f64, lower: f64, statut: &str) {
    let n = w + d + l;
    let (elo, margin) = elo_and_margin(w, d, l);
    let verdict = if statut.contains("PASS") {
        "garder la modif (B est plus fort)"
    } else if statut.contains("FAIL") {
        "retirer la modif (pas de gain)"
    } else {
        "résultat partiel — relancer pour conclure"
    };
    let content = format!(
"# Rapport SPRT Vendetta Chess Engine (point de vue B = candidat)
statut              = {statut}
verdict             = {verdict}

parties             = {n}
B_gagnees           = {w}
nulles              = {d}
B_perdues           = {l}

elo_estime          = {elo:.1}
elo_demi_marge_95   = {margin:.1}

llr                 = {llr_val:.3}
llr_borne_pass      = {upper:.3}
llr_borne_fail      = {lower:.3}

# Rappel de la config testée
config_nodes_a      = {}
config_nodes_b      = {}
config_improving_a  = {}
config_improving_b  = {}
config_futility_a   = {}
config_futility_b   = {}
config_lmr_a        = {}
config_lmr_b        = {}
config_correction_a = {}
config_correction_b = {}
config_king_attack_a = {}
config_king_attack_b = {}
config_elo0         = {}
config_elo1         = {}
config_alpha        = {}
config_beta         = {}
config_games_max    = {}
",
        cfg.nodes_a, cfg.nodes_b, cfg.improving_a, cfg.improving_b,
        cfg.futility_a, cfg.futility_b, cfg.lmr_a, cfg.lmr_b,
        cfg.correction_a, cfg.correction_b,
        cfg.king_attack_a, cfg.king_attack_b,
        cfg.elo0, cfg.elo1, cfg.alpha, cfg.beta, cfg.games_max,
    );
    if let Err(e) = fs::write(&cfg.report, content) {
        eprintln!("⚠ impossible d'écrire le rapport '{}' : {}", cfg.report, e);
    }
}

// =============================================================================
// Entry point
// =============================================================================

fn main() {
    // MANDATORY initialization of the attack / magic tables (as in perft/benchmark).
    init_attack_tables();

    let args: Vec<String> = env::args().collect();
    let config_path = args.get(1).map(|s| s.as_str()).unwrap_or("selfplay_config.txt");
    let cfg = parse_config(config_path);

    let upper = ((1.0 - cfg.beta) / cfg.alpha).ln();
    let lower = (cfg.beta / (1.0 - cfg.alpha)).ln();

    // Clean up any leftover STOP file from a previous run.
    let _ = fs::remove_file(STOP_FILE);

    println!("=== SPRT self-play Vendetta Chess Engine ===");
    println!("config            : {}", config_path);
    println!("A (référence)     : nodes={}  improving={}  futility={}  lmr={}  correction={}  king_attack={}", cfg.nodes_a, cfg.improving_a, cfg.futility_a, cfg.lmr_a, cfg.correction_a, cfg.king_attack_a);
    println!("B (candidat)      : nodes={}  improving={}  futility={}  lmr={}  correction={}  king_attack={}", cfg.nodes_b, cfg.improving_b, cfg.futility_b, cfg.lmr_b, cfg.correction_b, cfg.king_attack_b);
    println!("bornes SPRT       : elo0={} elo1={} (alpha={} beta={})", cfg.elo0, cfg.elo1, cfg.alpha, cfg.beta);
    println!("plafond           : {} parties", cfg.games_max);
    println!("parallélisme      : {} parties simultanées", cfg.concurrency.max(1));
    println!("rapport           : {}", cfg.report);
    println!("arrêt propre      : créer un fichier nommé '{}' (ex: touch {})", STOP_FILE, STOP_FILE);
    println!();

    // --- State shared between threads (all atomic, zero locks) ----------
    // Each game is independent: its own sides, its own board.
    // The ONLY shared state is this block of atomic counters.
    let cfg          = Arc::new(cfg);
    let wins         = Arc::new(AtomicU64::new(0)); // B wins
    let draws        = Arc::new(AtomicU64::new(0));
    let losses       = Arc::new(AtomicU64::new(0)); // B loses
    let game_counter = Arc::new(AtomicU64::new(0)); // index of the next game to play
    let stop         = Arc::new(AtomicBool::new(false));

    let concurrency = cfg.concurrency.clamp(1, MAX_CONCURRENCY);

    // --- Worker threads: play games as long as `stop` is false ------
    // Game index g → opening seed = g/2, color a_white = (g even).
    // This way each opening is played in both directions (color fairness),
    // exactly like the old pairing scheme, but spread across the threads.
    let mut handles = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let cfg          = Arc::clone(&cfg);
        let wins         = Arc::clone(&wins);
        let draws        = Arc::clone(&draws);
        let losses       = Arc::clone(&losses);
        let game_counter = Arc::clone(&game_counter);
        let stop         = Arc::clone(&stop);
        let h = thread::Builder::new()
            .stack_size(8 * 1024 * 1024) // margin for the alpha-beta recursion
            .spawn(move || {
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let g = game_counter.fetch_add(1, Ordering::Relaxed);
                    if g >= cfg.games_max {
                        break; // do not start beyond the cap
                    }
                    let seed    = g / 2;
                    let a_white = g.is_multiple_of(2);
                    match play_one_game(seed, a_white, &cfg) {
                        1  => { wins.fetch_add(1, Ordering::Relaxed); }
                        -1 => { losses.fetch_add(1, Ordering::Relaxed); }
                        _  => { draws.fetch_add(1, Ordering::Relaxed); }
                    }
                }
            })
            .expect("échec du lancement d'un thread ouvrier");
        handles.push(h);
    }

    // --- Main thread: polls, displays, saves, decides on the stop ---
    // `statut` is assigned by EVERY exit branch of the loop below
    // (the loop only exits via `break`), so no unnecessary initial value.
    let statut: String;
    // Number of games at the last display: avoids reprinting an identical
    // line when no game has finished between two polls
    // (frequent with a large node budget, where a game lasts several polls).
    let mut last_reported = u64::MAX;
    loop {
        thread::sleep(Duration::from_millis(POLL_MS));

        let w = wins.load(Ordering::Relaxed);
        let d = draws.load(Ordering::Relaxed);
        let l = losses.load(Ordering::Relaxed);
        let llr_val = llr(w, d, l, cfg.elo0, cfg.elo1);

        // Clean stop via STOP file.
        if Path::new(STOP_FILE).exists() {
            statut = "INTERROMPU".to_string();
            let _ = fs::remove_file(STOP_FILE);
            break;
        }
        // Game cap reached.
        if w + d + l >= cfg.games_max {
            statut = "PLAFOND_ATTEINT".to_string();
            break;
        }
        // SPRT concluded?
        if llr_val >= upper {
            statut = "SPRT_CONCLU_PASS".to_string();
            break;
        }
        if llr_val <= lower {
            statut = "SPRT_CONCLU_FAIL".to_string();
            break;
        }
        // Progress + autosave — only if the game counter has moved,
        // so as not to reprint an identical line (anti-spam).
        let n = w + d + l;
        if n != last_reported {
            last_reported = n;
            print_progress(&cfg, w, d, l, llr_val, upper, lower);
            write_report(&cfg, w, d, l, llr_val, upper, lower, "EN_COURS");
        }
    }

    // Signal the stop and wait for the games in progress to finish.
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }

    // --- Final report (read after join: includes the last games) ---
    let w = wins.load(Ordering::Relaxed);
    let d = draws.load(Ordering::Relaxed);
    let l = losses.load(Ordering::Relaxed);
    let final_llr = llr(w, d, l, cfg.elo0, cfg.elo1);
    println!();
    print_progress(&cfg, w, d, l, final_llr, upper, lower);
    println!("--> statut final : {}", statut);
    write_report(&cfg, w, d, l, final_llr, upper, lower, &statut);
    println!("--> rapport écrit dans : {}", cfg.report);
}
