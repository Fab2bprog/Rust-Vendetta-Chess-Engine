// =============================================================================
// Vendetta Chess Motor — src/bin/perft.rs
//
// Rôle : Outil de validation de la génération de coups par la méthode Perft.
//
// Principe :
//   Perft (PERFormance Test) compte le nombre exact de positions atteignables
//   depuis une position donnée à une profondeur N. Ces résultats sont connus
//   avec précision et servent de référence absolue pour valider qu'un moteur
//   ne génère ni coup illégal ni coup manquant.
//
//   Si perft(pos, depth) retourne 197 281 au lieu de 197 281 → le moteur est
//   correct à cette profondeur. Un écart, même de 1, indique un bug précis
//   dans la génération : clouage non détecté, roque illégal accepté, prise en
//   passant manquée, promotion mal gérée, etc.
//
// Utilisation :
//   # Lancer toutes les positions de référence
//   cargo run --release --bin perft
//
//   # Lancer une position spécifique jusqu'à une profondeur donnée
//   cargo run --release --bin perft -- "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1" 5
//
//   # Mode divide : afficher le compte par coup racine (utile pour isoler un bug)
//   cargo run --release --bin perft -- divide "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1" 3
//
// Positions de référence :
//   Les 6 positions standard de la Chess Programming Wiki, couvrant
//   tous les cas spéciaux : roques, prises en passant, promotions,
//   découvertes d'échec, positions de fin de partie.
//
// Interprétation de la sortie :
//   ✓ PASS  → génération correcte à cette profondeur
//   ✗ FAIL  → bug détecté ; utiliser le mode divide pour le localiser
//   ?       → aucune valeur de référence connue pour cette profondeur
// =============================================================================

use std::time::Instant;
use vendetta_chess_motor::board::state::Board;
use vendetta_chess_motor::moves::{perft, perft_divide};

// =============================================================================
// Positions de référence
// =============================================================================

/// Une position de test perft avec ses résultats attendus par profondeur.
struct PerftPosition {
    /// Nom descriptif de la position (pour l'affichage).
    name:     &'static str,
    /// FEN de la position.
    fen:      &'static str,
    /// Résultats attendus : expected[i] = nombre de nœuds à profondeur i+1.
    /// None = résultat non renseigné pour cette profondeur.
    expected: &'static [Option<u64>],
}

/// Les 6 positions de référence standard de la Chess Programming Wiki.
/// Source : https://www.chessprogramming.org/Perft_Results
///
/// Couverture des cas spéciaux :
///   Pos 1 — Position initiale          : cas de base
///   Pos 2 — Kiwipete                   : roques des deux côtés, promotions, en passant
///   Pos 3 — Finale avec pions passés    : promotions, règle des 50 coups
///   Pos 4 — Promotions intensives       : promotions + roques avec droits limités
///   Pos 5 — En passant et promotions    : prises en passant edge-cases
///   Pos 6 — Milieu de partie équilibré  : coups silencieux et captures mélangés
static POSITIONS: &[PerftPosition] = &[
    PerftPosition {
        name: "Position 1 — Initiale",
        fen:  "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        // d1=20, d2=400, d3=8902, d4=197281, d5=4865609, d6=119060324
        expected: &[
            Some(20),
            Some(400),
            Some(8_902),
            Some(197_281),
            Some(4_865_609),
            Some(119_060_324),
        ],
    },
    PerftPosition {
        name: "Position 2 — Kiwipete (roques, en passant, promotions)",
        fen:  "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        // d1=48, d2=2039, d3=97862, d4=4085603, d5=193690690
        expected: &[
            Some(48),
            Some(2_039),
            Some(97_862),
            Some(4_085_603),
            Some(193_690_690),
        ],
    },
    PerftPosition {
        name: "Position 3 — Finale pions passés",
        fen:  "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        // d1=14, d2=191, d3=2812, d4=43238, d5=674624, d6=11030083
        expected: &[
            Some(14),
            Some(191),
            Some(2_812),
            Some(43_238),
            Some(674_624),
            Some(11_030_083),
        ],
    },
    PerftPosition {
        name: "Position 4 — Promotions et roques minoritaires",
        fen:  "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
        // d1=6, d2=264, d3=9467, d4=422333, d5=15833292
        expected: &[
            Some(6),
            Some(264),
            Some(9_467),
            Some(422_333),
            Some(15_833_292),
        ],
    },
    PerftPosition {
        name: "Position 5 — En passant et promotions edge-cases",
        fen:  "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
        // d1=44, d2=1486, d3=62379, d4=2103487, d5=89941194
        expected: &[
            Some(44),
            Some(1_486),
            Some(62_379),
            Some(2_103_487),
            Some(89_941_194),
        ],
    },
    PerftPosition {
        name: "Position 6 — Milieu de partie équilibré",
        fen:  "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
        // d1=46, d2=2079, d3=89890, d4=3894594
        expected: &[
            Some(46),
            Some(2_079),
            Some(89_890),
            Some(3_894_594),
        ],
    },
];

// =============================================================================
// Exécution d'une suite perft
// =============================================================================

/// Lance toutes les profondeurs définies pour une position et affiche les résultats.
/// Retourne true si tous les résultats sont corrects (ou non vérifiables).
fn run_position(pos: &PerftPosition, max_depth: Option<u32>) -> bool {
    let mut board = match Board::from_fen(pos.fen) {
        Ok(b)  => b,
        Err(e) => {
            println!("  ✗ ERREUR FEN : {}", e);
            return false;
        }
    };

    let depth_limit = max_depth
        .unwrap_or(pos.expected.len() as u32)
        .min(pos.expected.len() as u32);

    let mut all_pass = true;

    for depth in 1..=depth_limit {
        let t0      = Instant::now();
        let result  = perft(&mut board, depth);
        let elapsed = t0.elapsed();

        let ms  = elapsed.as_millis();
        let nps = if ms > 0 { result * 1000 / ms as u64 } else { result };

        let expected = pos.expected[(depth - 1) as usize];
        let status   = match expected {
            Some(exp) if exp == result => "✓ PASS",
            Some(_)                   => { all_pass = false; "✗ FAIL" }
            None                      => "? ----",
        };

        match expected {
            Some(exp) => println!(
                "  d{} {:>12} nœuds  {:>8} ms  {:>10} nps  {}  (attendu : {})",
                depth, fmt_num(result), ms, fmt_num(nps), status, fmt_num(exp)
            ),
            None => println!(
                "  d{} {:>12} nœuds  {:>8} ms  {:>10} nps  {}",
                depth, fmt_num(result), ms, fmt_num(nps), status
            ),
        }
    }

    all_pass
}

// =============================================================================
// Utilitaires d'affichage
// =============================================================================

/// Formate un entier avec des espaces comme séparateurs de milliers.
/// Ex : 4865609 → "4 865 609"
fn fmt_num(n: u64) -> String {
    let s   = n.to_string();
    let len = s.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// Affiche une ligne séparatrice.
fn separator() {
    println!("{}", "─".repeat(72));
}

// =============================================================================
// Point d'entrée
// =============================================================================

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // --- Mode divide : perft divide <fen> <depth> ---
    // Affiche le nombre de nœuds pour chaque coup racine.
    // Indispensable pour isoler un bug : on compare coup par coup avec
    // un moteur de référence (Stockfish) jusqu'à trouver la divergence.
    if args.len() >= 4 && args[1] == "divide" {
        let fen   = &args[2];
        let depth = args[3].parse::<u32>().unwrap_or(1);

        println!();
        println!("  Perft divide — profondeur {}", depth);
        println!("  FEN : {}", fen);
        separator();

        let mut board = match Board::from_fen(fen) {
            Ok(b)  => b,
            Err(e) => { eprintln!("Erreur FEN : {}", e); return; }
        };

        let t0    = Instant::now();
        let total = perft_divide(&mut board, depth);
        let ms    = t0.elapsed().as_millis();
        let nps   = if ms > 0 { total * 1000 / ms as u64 } else { total };

        separator();
        println!("  Total : {}   ({} ms, {} nps)", fmt_num(total), ms, fmt_num(nps));
        println!();
        return;
    }

    // --- Mode position unique : perft <fen> <depth> ---
    // On n'utilise pas PerftPosition ici car son champ `fen` est &'static str
    // et args[1] est une variable locale — on inline la logique directement.
    if args.len() >= 3 && args[1] != "divide" {
        let fen       = args[1].clone();
        let max_depth = args[2].parse::<u32>().unwrap_or(5);

        println!();
        println!("  Perft — position personnalisée");
        println!("  FEN : {}", fen);
        separator();

        let mut board = match Board::from_fen(&fen) {
            Ok(b)  => b,
            Err(e) => { eprintln!("  ✗ Erreur FEN : {}", e); return; }
        };

        for depth in 1..=max_depth {
            let t0      = Instant::now();
            let result  = perft(&mut board, depth);
            let ms      = t0.elapsed().as_millis();
            let nps     = if ms > 0 { result * 1000 / ms as u64 } else { result };
            println!("  d{} {:>12} nœuds  {:>8} ms  {:>10} nps  ? ----",
                depth, fmt_num(result), ms, fmt_num(nps));
        }
        println!();
        return;
    }

    // --- Mode suite complète (défaut) ---
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║         Vendetta Chess Motor — Suite de validation Perft                   ║");
    println!("║         6 positions standard · Chess Programming Wiki               ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    println!();
    println!("  Légende : ✓ PASS  ✗ FAIL  ? (pas de référence à cette profondeur)");
    println!();

    // Profondeurs par défaut pour la suite (équilibre vitesse/couverture).
    // En mode release, la suite complète tourne en < 30 secondes.
    // Augmenter les profondeurs pour une validation exhaustive.
    let max_depths: &[u32] = &[
        5, // Pos 1 : 4 865 609 nœuds (~2 s en release)
        4, // Pos 2 : 4 085 603 nœuds (~2 s en release)
        5, // Pos 3 :   674 624 nœuds (<1 s en release)
        4, // Pos 4 :   422 333 nœuds (<1 s en release)
        4, // Pos 5 : 2 103 487 nœuds (~1 s en release)
        4, // Pos 6 : 3 894 594 nœuds (~2 s en release)
    ];

    let mut total_pass = 0usize;
    let mut total_fail = 0usize;
    let suite_start    = Instant::now();

    for (i, pos) in POSITIONS.iter().enumerate() {
        separator();
        println!("  {}", pos.name);
        println!("  FEN : {}", pos.fen);
        println!();

        let max_d  = max_depths.get(i).copied().unwrap_or(4);
        let pass   = run_position(pos, Some(max_d));

        if pass { total_pass += 1; } else { total_fail += 1; }
        println!();
    }

    let suite_ms = suite_start.elapsed().as_millis();

    separator();
    println!();
    println!("  Résultat global : {}/{} positions correctes  ({} ms total)",
        total_pass, POSITIONS.len(), suite_ms);
    println!();

    if total_fail == 0 {
        println!("  ✓ Génération de coups validée — aucun bug détecté.");
    } else {
        println!("  ✗ {} position(s) en échec — utiliser le mode divide pour isoler le bug :", total_fail);
        println!("    cargo run --release --bin perft -- divide \"<fen>\" <depth>");
    }
    println!();

    // Quitter avec code d'erreur si des tests échouent (utile pour CI).
    if total_fail > 0 {
        std::process::exit(1);
    }
}
