// =============================================================================
// Vendetta Chess Engine — src/main.rs
//
// Role: Program entry point. Initializes the necessary tables
//        and starts the main UCI loop.
//
// On startup:
//   1. Initialization of precomputed attack tables (knight, king)
//   2. Launch of the UCI loop (reading from stdin, writing to stdout)
//
// The engine communicates exclusively via stdin/stdout according to the UCI protocol.
// It must not display a graphical interface or open a window.
// =============================================================================

use vendetta_chess_engine::board::bitboard::init_attack_tables;
use vendetta_chess_engine::uci::UciEngine;

fn main() {
    // Initialize the precomputed attack tables for the knight and king.
    // Must be done before any use of the move generation functions.
    init_attack_tables();

    // Start the UCI engine
    let mut engine = UciEngine::new();
    engine.run();
}
