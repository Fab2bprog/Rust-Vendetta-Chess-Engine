// =============================================================================
// Vendetta Chess Motor — src/bin/benchmark.rs
//
// Rôle : Mesurer les performances réelles du moteur en conditions de jeu.
//
// Ce benchmark est différent de perft :
//   - perft    mesure la CORRECTION  (bon nombre de coups générés ?)
//   - benchmark mesure la PERFORMANCE (combien de nœuds alpha-bêta par seconde ?)
//
// Ce que mesure ce benchmark :
//   1. NPS (nœuds par seconde) en recherche alpha-bêta réelle
//   2. Scalabilité Lazy SMP : gain sur 1, 2, 4, 8, N cœurs
//   3. Profondeur atteinte en 3 secondes sur des positions types
//
// Utilisation :
//   cargo run --release --bin benchmark
//   cargo run --release --bin benchmark -- --time 5000    # 5 secondes par test
//   cargo run --release --bin benchmark -- --threads 4    # forcer 4 threads max
//
// Interprétation :
//   - NPS croissant avec les threads → Lazy SMP scale bien
//   - Plateau rapide → contention sur la table de transposition
//   - Gain ×3-5 sur 12 cœurs est normal et sain pour Lazy SMP
// =============================================================================

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use vendetta_chess_motor::board::state::Board;
use vendetta_chess_motor::board::bitboard::init_attack_tables;
use vendetta_chess_motor::search::transposition::TranspositionTable;
use vendetta_chess_motor::search::killers::KillerMoves;
use vendetta_chess_motor::search::history::HistoryTable;
use vendetta_chess_motor::search::countermove::CountermoveTable;
use vendetta_chess_motor::search::continuation_history::ContinuationHistoryTable;
use vendetta_chess_motor::search::alphabeta::alpha_beta;
use vendetta_chess_motor::search::SearchInfo;
use vendetta_chess_motor::utils::types::{Move, SCORE_MATE};

// =============================================================================
// Positions de benchmark
// =============================================================================

struct BenchPosition {
    name: &'static str,
    fen:  &'static str,
}

/// Positions couvrant les 3 phases de la partie.
/// Choisies pour leur richesse tactique et leur représentativité.
static POSITIONS: &[BenchPosition] = &[
    BenchPosition {
        name: "Ouverture — Position initiale",
        fen:  "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
    },
    BenchPosition {
        name: "Milieu de partie — Kiwipete (roques, tactiques)",
        fen:  "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    },
    BenchPosition {
        name: "Milieu de partie — Position ouverte équilibrée",
        fen:  "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    },
    BenchPosition {
        name: "Finale — Pions passés et tours",
        fen:  "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    },
    BenchPosition {
        name: "Finale — Roi et pions",
        fen:  "8/8/p1p5/1p5p/1P5P/P4K2/8/7k w - - 0 1",
    },
];

// =============================================================================
// Moteur de mesure
// =============================================================================

/// Résultat d'une mesure sur un nombre de threads donné.
struct ThreadResult {
    threads:   usize,
    nodes:     u64,
    elapsed_ms: u64,
    nps:       u64,
    depth:     i32,
}

/// Lance une recherche alpha-bêta pendant `duration` sur `num_threads` threads.
/// Retourne le total de nœuds explorés (somme de tous les threads).
fn run_search(board: &Board, num_threads: usize, duration: Duration) -> ThreadResult {
    let tt        = Arc::new(TranspositionTable::new(64)); // 64 Mo TT
    let stop_flag = Arc::new(AtomicBool::new(false));
    let t0        = Instant::now();

    // --- Threads secondaires (Lazy SMP) ---
    let mut handles = vec![];

    for t in 1..num_threads {
        let mut board_copy  = board.clone();
        let tt_shared       = Arc::clone(&tt);
        let stop_shared     = Arc::clone(&stop_flag);
        let depth_variation = (t % 3) as i32;

        let handle = std::thread::spawn(move || -> u64 {
            let mut killers      = KillerMoves::new();
            let mut history      = HistoryTable::new();
            let mut countermoves = CountermoveTable::new();
            let mut cont_history = ContinuationHistoryTable::new();
            let mut info    = SearchInfo::new_with_stop(
                Duration::from_secs(3600),
                stop_shared.clone(),
            );

            let max_depth = 64i32 + depth_variation;
            for depth in 1..=max_depth {
                if stop_shared.load(Ordering::Relaxed) { break; }
                alpha_beta(
                    &mut board_copy, depth, -SCORE_MATE, SCORE_MATE, 0,
                    &tt_shared, &mut killers, &mut history,
                    &mut countermoves, &mut cont_history, Move::NULL, &mut info,
                    Move::NULL, &[],
                );
            }
            info.nodes
        });
        handles.push(handle);
    }

    // --- Thread principal ---
    let mut board_main = board.clone();
    let tt_main        = Arc::clone(&tt);
    let stop_main      = Arc::clone(&stop_flag);
    let mut killers      = KillerMoves::new();
    let mut history      = HistoryTable::new();
    let mut countermoves = CountermoveTable::new();
    let mut cont_history = ContinuationHistoryTable::new();
    let mut info       = SearchInfo::new_with_stop(duration, Arc::clone(&stop_main));

    let mut best_depth = 0i32;
    for depth in 1..=64i32 {
        if info.should_stop() { break; }
        info.best_move = Move::NULL;
        alpha_beta(
            &mut board_main, depth, -SCORE_MATE, SCORE_MATE, 0,
            &tt_main, &mut killers, &mut history,
            &mut countermoves, &mut cont_history, Move::NULL, &mut info,
            Move::NULL, &[],
        );
        if !info.should_stop() {
            best_depth = depth;
        }
        // Vérifier manuellement le temps (check_time ne suffit pas pour une durée exacte)
        if t0.elapsed() >= duration {
            stop_flag.store(true, Ordering::Relaxed);
            break;
        }
    }

    // Arrêt des threads secondaires
    stop_flag.store(true, Ordering::Relaxed);

    // Collecter les nœuds des threads secondaires
    let mut total_nodes = info.nodes;
    for h in handles {
        total_nodes += h.join().unwrap_or(0);
    }

    let elapsed_ms = t0.elapsed().as_millis() as u64;
    let nps        = total_nodes.saturating_mul(1000).checked_div(elapsed_ms).unwrap_or(total_nodes);

    ThreadResult {
        threads: num_threads,
        nodes: total_nodes,
        elapsed_ms,
        nps,
        depth: best_depth,
    }
}

// =============================================================================
// Utilitaires d'affichage
// =============================================================================

fn fmt_num(n: u64) -> String {
    let s   = n.to_string();
    let len = s.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) { out.push(' '); }
        out.push(ch);
    }
    out
}

fn separator() {
    println!("{}", "─".repeat(78));
}

fn bar(ratio: f64, width: usize) -> String {
    let filled = ((ratio * width as f64).round() as usize).min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

// =============================================================================
// Point d'entrée
// =============================================================================

fn main() {
    // --- Parsing des arguments ---
    let args: Vec<String> = std::env::args().collect();
    let mut duration_ms  = 3_000u64; // 3 secondes par défaut
    let mut max_threads  = std::thread::available_parallelism()
        .map(|n| n.get()).unwrap_or(1);

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--time" | "-t" => {
                if let Some(v) = args.get(i + 1) {
                    duration_ms = v.parse().unwrap_or(3_000);
                    i += 1;
                }
            }
            "--threads" | "-j" => {
                if let Some(v) = args.get(i + 1) {
                    max_threads = v.parse().unwrap_or(max_threads);
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Initialiser les tables d'attaque (magic bitboards)
    init_attack_tables();

    let duration    = Duration::from_millis(duration_ms);
    let cpu_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);

    // Paliers de threads à tester
    let thread_counts: Vec<usize> = {
        let mut counts = vec![1usize];
        let mut t = 2;
        while t <= max_threads {
            counts.push(t);
            t *= 2;
        }
        if *counts.last().unwrap() != max_threads {
            counts.push(max_threads);
        }
        counts
    };

    // En-tête
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════════╗");
    println!("║         Vendetta Chess Motor — Benchmark de performance                        ║");
    println!("╚══════════════════════════════════════════════════════════════════════════╝");
    println!();
    println!("  CPU        : {} cœurs disponibles", cpu_threads);
    println!("  Threads    : {:?}", thread_counts);
    println!("  Durée/test : {} ms", duration_ms);
    println!("  Positions  : {}", POSITIONS.len());
    println!();

    let mut grand_total_nps_1t  = 0u64;
    let mut grand_total_nps_max = 0u64;
    let mut pos_count           = 0usize;

    // --- Boucle sur les positions ---
    for pos in POSITIONS {
        let board = match Board::from_fen(pos.fen) {
            Ok(b)  => b,
            Err(e) => { println!("  ✗ FEN invalide : {}", e); continue; }
        };

        separator();
        println!("  {}", pos.name);
        println!("  FEN : {}", pos.fen);
        println!();

        let mut results: Vec<ThreadResult> = Vec::new();

        // Mesure pour chaque palier de threads
        for &n in &thread_counts {
            let r = run_search(&board, n, duration);
            print!(
                "  {:>2} thread{}  {:>14} nœuds  {:>4} ms  d{:<2}  {:>12} nps",
                r.threads,
                if r.threads == 1 { " " } else { "s" },
                fmt_num(r.nodes),
                r.elapsed_ms,
                r.depth,
                fmt_num(r.nps),
            );
            results.push(r);
            println!();
        }

        // Scalabilité : ratio par rapport au mono-thread
        if results.len() > 1 {
            let nps_1t = results[0].nps.max(1);
            println!();
            println!("  Scalabilité Lazy SMP :");
            let bar_max = results.iter().map(|r| r.nps).max().unwrap_or(1);
            for r in &results {
                let ratio = r.nps as f64 / nps_1t as f64;
                let b     = bar(r.nps as f64 / bar_max as f64, 30);
                println!(
                    "    {:>2} thread{}  {} ×{:.2}",
                    r.threads,
                    if r.threads == 1 { " " } else { "s" },
                    b,
                    ratio,
                );
            }
            grand_total_nps_1t  += nps_1t;
            grand_total_nps_max += results.last().unwrap().nps;
            pos_count += 1;
        }

        println!();
    }

    // --- Résumé global ---
    separator();
    println!();
    println!("  Résumé global ({} positions) :", pos_count);

    if pos_count > 0 {
        let avg_1t  = grand_total_nps_1t  / pos_count as u64;
        let avg_max = grand_total_nps_max / pos_count as u64;
        let gain    = avg_max as f64 / avg_1t.max(1) as f64;

        println!("    NPS moyen  1 thread  : {:>14} nps", fmt_num(avg_1t));
        println!("    NPS moyen {} threads : {:>14} nps", max_threads, fmt_num(avg_max));
        println!("    Gain Lazy SMP        : ×{:.2}  ({} → {} cœurs)", gain, 1, max_threads);
        println!();

        // Verdict sur la scalabilité
        if gain >= 4.0 {
            println!("  ✓ Excellent — Lazy SMP scale très bien sur ce matériel.");
        } else if gain >= 2.5 {
            println!("  ✓ Bon — Lazy SMP scale correctement.");
        } else if gain >= 1.5 {
            println!("  ~ Moyen — contention possible sur la table de transposition.");
        } else {
            println!("  ✗ Faible — contention élevée, Lazy SMP peu efficace ici.");
        }
    }

    println!();
    println!("  Conseil : relancer avec --time 10000 pour des mesures plus stables.");
    println!();
}
