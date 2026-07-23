// =============================================================================
// Vendetta Chess Motor — src/lib.rs
//
// Role: Root of the library. Declares all the project's modules and
//        re-exports them to facilitate their use in tests and
//        future external components (future GUI, etc.).
//
// Module structure:
//   utils   → common types (Color, Piece, Move, constants...)
//   board   → chessboard representation (bitboards, state, FEN)
//   moves   → legal move generation
//   eval    → static position evaluation
//   search  → alpha-beta search algorithm
//   game    → game management (history, end rules)
//   uci     → UCI protocol (communication with the graphical interface)
// =============================================================================

pub mod utils;
pub mod board;
pub mod moves;
pub mod eval;
pub mod search;
pub mod game;
pub mod uci;
