// =============================================================================
// Vendetta Chess Engine — src/uci/parser.rs
//
// Role: Parsing of UCI commands received on standard input.
//        The UCI protocol communicates via text exchange over stdin/stdout.
//        This module transforms lines of text into structured commands.
//
// UCI commands handled:
//   - uci          : engine identification
//   - isready      : readiness check
//   - ucinewgame   : start of a new game
//   - position     : definition of the current position
//   - go           : start of the search
//   - stop         : stop the search
//   - setoption    : configuration of options (difficulty level, TT size...)
//   - quit         : shutdown of the engine
//
// Contents:
//   - UciCommand : enum of UCI commands
//   - parse_command() : transforms a line into a UciCommand
//   - parse_go_params() : parses the parameters of the go command
// =============================================================================

use crate::utils::types::square_from_str;
use crate::utils::types::Move;
use crate::search::SearchConfig;

/// UCI commands recognized by Vendetta Chess Engine.
#[derive(Debug)]
pub enum UciCommand {
    /// Engine identification: "uci"
    Uci,
    /// Readiness check: "isready"
    IsReady,
    /// New game: "ucinewgame"
    UciNewGame,
    /// Position to analyze: "position [startpos|fen <fen>] [moves <move1> ...]"
    Position {
        /// FEN of the starting position (None = initial position).
        fen: Option<String>,
        /// List of moves to play from the starting position.
        moves: Vec<String>,
    },
    /// Start the search: "go [wtime <ms>] [btime <ms>] ..."
    Go(SearchConfig),
    /// Stop the search: "stop"
    Stop,
    /// The opponent played the predicted move: "ponderhit"
    /// The engine must switch from ponder mode to normal search with time management.
    PonderHit,
    /// Configure an option: "setoption name <name> value <value>"
    SetOption {
        name:  String,
        value: String,
    },
    /// Quit the engine: "quit"
    Quit,
    /// Enable or disable debug mode: "debug on|off"
    /// In debug mode, the engine may emit additional "info string" messages.
    Debug {
        /// true = debug enabled, false = debug disabled.
        on: bool,
    },
    /// Engine registration: "register later|name <x> code <y>" (or "register").
    /// Vendetta Chess Engine uses NO anti-copy protection: the command
    /// is accepted with no action. Declared explicitly to cover the entirety
    /// of the UCI spec's commands (instead of treating it as unknown).
    Register,
    /// Unrecognized command (silently ignored — required by UCI).
    Unknown,
}

/// Parses a line of text and returns the corresponding UCI command.
pub fn parse_command(line: &str) -> UciCommand {
    let line = line.trim();
    let parts: Vec<&str> = line.split_whitespace().collect();

    if parts.is_empty() {
        return UciCommand::Unknown;
    }

    match parts[0] {
        "uci"        => UciCommand::Uci,
        "isready"    => UciCommand::IsReady,
        "ucinewgame" => UciCommand::UciNewGame,
        "stop"       => UciCommand::Stop,
        "ponderhit"  => UciCommand::PonderHit,
        "register"   => UciCommand::Register,
        "quit" | "q" => UciCommand::Quit,

        "position" => parse_position_command(&parts[1..]),
        "go"       => UciCommand::Go(parse_go_command(&parts[1..])),
        "setoption" => parse_setoption_command(&parts[1..]),
        "debug"    => parse_debug_command(&parts[1..]),

        _ => UciCommand::Unknown,
    }
}

/// Parses the "position" command.
/// Format: "position startpos [moves e2e4 e7e5 ...]"
///       or "position fen <fen> [moves e2e4 e7e5 ...]"
fn parse_position_command(parts: &[&str]) -> UciCommand {
    if parts.is_empty() {
        return UciCommand::Unknown;
    }

    let mut fen: Option<String> = None;
    let mut moves: Vec<String> = Vec::new();
    let mut i = 0;

    if parts[0] == "startpos" {
        // Initial position
        fen = None;
        i = 1;
    } else if parts[0] == "fen" {
        // FEN provided: read up to the word "moves" or the end
        i = 1;
        let mut fen_parts: Vec<&str> = Vec::new();
        while i < parts.len() && parts[i] != "moves" {
            fen_parts.push(parts[i]);
            i += 1;
        }
        fen = Some(fen_parts.join(" "));
    }

    // Read the moves if present
    if i < parts.len() && parts[i] == "moves" {
        i += 1;
        while i < parts.len() {
            moves.push(parts[i].to_string());
            i += 1;
        }
    }

    UciCommand::Position { fen, moves }
}

/// Returns true if the token is a known keyword of the "go" command.
/// Used to detect the end of a "searchmoves" list.
#[inline]
fn is_go_keyword(s: &str) -> bool {
    matches!(s,
        "wtime" | "btime" | "winc" | "binc" | "movestogo"
        | "depth" | "movetime" | "infinite" | "ponder" | "searchmoves"
        | "nodes" | "mate"
    )
}

/// Parses the "go" command and its parameters.
/// Format: "go [wtime <ms>] [btime <ms>] [winc <ms>] [binc <ms>]
///               [movestogo <n>] [depth <n>] [movetime <ms>] [infinite]
///               [nodes <n>] [mate <n>]
///               [searchmoves <move1> <move2> ...]"
fn parse_go_command(parts: &[&str]) -> SearchConfig {
    let mut config = SearchConfig::default();
    let mut i = 0;

    while i < parts.len() {
        match parts[i] {
            "wtime" => {
                i += 1;
                if i < parts.len() {
                    config.wtime = parts[i].parse().ok();
                }
            }
            "btime" => {
                i += 1;
                if i < parts.len() {
                    config.btime = parts[i].parse().ok();
                }
            }
            "winc" => {
                i += 1;
                if i < parts.len() {
                    config.winc = parts[i].parse().ok();
                }
            }
            "binc" => {
                i += 1;
                if i < parts.len() {
                    config.binc = parts[i].parse().ok();
                }
            }
            "movestogo" => {
                i += 1;
                if i < parts.len() {
                    config.movestogo = parts[i].parse().ok();
                }
            }
            "depth" => {
                i += 1;
                if i < parts.len() {
                    config.depth = parts[i].parse().ok();
                }
            }
            "movetime" => {
                i += 1;
                if i < parts.len() {
                    config.movetime = parts[i].parse().ok();
                }
            }
            "nodes" => {
                i += 1;
                if i < parts.len() {
                    config.nodes = parts[i].parse().ok();
                }
            }
            "mate" => {
                i += 1;
                if i < parts.len() {
                    config.mate = parts[i].parse().ok();
                }
            }
            "infinite" => {
                config.infinite = true;
            }
            "ponder" => {
                config.ponder = true;
            }
            "searchmoves" => {
                // Consume all following tokens until the next "go" keyword
                // or the end of the line.
                i += 1;
                while i < parts.len() && !is_go_keyword(parts[i]) {
                    // Light validation: a UCI move is 4 or 5 characters long (e.g.: e2e4, e7e8q).
                    // We accept without further validation (legal validation is done on the engine side).
                    if parts[i].len() >= 4 {
                        config.searchmoves.push(parts[i].to_string());
                    }
                    i += 1;
                }
                // i now points to the next keyword (or out of bounds).
                // The `continue` avoids the `i += 1` at the bottom of the loop so as not to skip it.
                continue;
            }
            _ => {}
        }
        i += 1;
    }

    config
}

/// Parses the "setoption" command.
/// Format: "setoption name <name> value <value>"
fn parse_setoption_command(parts: &[&str]) -> UciCommand {
    let mut name  = String::new();
    let mut value = String::new();
    let mut i = 0;

    if i < parts.len() && parts[i] == "name" {
        i += 1;
        // Read the name up to "value"
        let mut name_parts = Vec::new();
        while i < parts.len() && parts[i] != "value" {
            name_parts.push(parts[i]);
            i += 1;
        }
        name = name_parts.join(" ");
    }

    if i < parts.len() && parts[i] == "value" {
        i += 1;
        // Read the value up to the end
        value = parts[i..].join(" ");
    }

    UciCommand::SetOption { name, value }
}

/// Parses the "debug" command.
/// Format: "debug on" or "debug off"
/// Any other value is ignored (we remain in the current state).
fn parse_debug_command(parts: &[&str]) -> UciCommand {
    match parts.first().copied() {
        Some("on")  => UciCommand::Debug { on: true  },
        Some("off") => UciCommand::Debug { on: false },
        _           => UciCommand::Unknown,
    }
}

/// Converts a UCI move notation (e.g.: "e2e4", "e7e8q") into a legal Move.
///
/// Returns None if the notation is invalid OR if the move is not legal
/// in the current position. The flags of the returned Move always come
/// from the legal move generator, which guarantees their accuracy.
pub fn parse_move_uci(mv_str: &str, board: &mut crate::board::state::Board) -> Option<Move> {
    // A UCI move is ALWAYS ASCII (e.g.: "e2e4", "e7e8q").
    // We immediately reject any non-ASCII token: without this safeguard, the
    // byte slicing `&mv_str[0..2]` below would panic
    // ("byte index N is not a char boundary") if a byte boundary
    // fell in the middle of a multi-byte character (e.g.: "🙂e4").
    // mv_str.len() is a length in bytes; on pure ASCII,
    // 1 byte = 1 character, so the indices [0..2] and [2..4] are safe.
    if !mv_str.is_ascii() || mv_str.len() < 4 {
        return None;
    }

    let from = square_from_str(&mv_str[0..2])?;
    let to   = square_from_str(&mv_str[2..4])?;

    // Promotion piece (optional 5th character, case-insensitive)
    let promotion: u8 = if mv_str.len() >= 5 {
        match mv_str.chars().nth(4)?.to_ascii_lowercase() {
            'n' => 1,
            'b' => 2,
            'r' => 3,
            'q' => 4,
            _   => 0,
        }
    } else {
        0
    };

    // Validate the move against the list of legal moves.
    // We look for the legal Move whose (from, to, promotion) match.
    // This approach guarantees:
    //   1. That no illegal move can be played (including moves that
    //      would expose the king or violate the rules of castling / en passant).
    //   2. That the MoveFlags flags are always exact (provided by the generator,
    //      not heuristically reconstructed from the UCI string).
    use crate::moves::generate_legal_moves;
    let legal_moves = generate_legal_moves(board);
    legal_moves.into_iter().find(|mv| {
        mv.from == from && mv.to == to && mv.promotion == promotion
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::state::Board;

    /// Non-regression (stability audit 2026-06-23, bug #1):
    /// a non-ASCII move token must NEVER cause the engine to panic
    /// (the byte slicing `&mv_str[0..2]` used to panic before the fix).
    #[test]
    fn parse_move_uci_non_ascii_ne_panique_pas() {
        let mut board = Board::start_position();
        // 4-byte emoji: without the is_ascii() safeguard, &mv_str[0..2]
        // would cut in the middle of the character → panic.
        assert!(parse_move_uci("🙂e4", &mut board).is_none());
        // Accented character (2 bytes) placed to throw off the byte boundary.
        assert!(parse_move_uci("e2é4", &mut board).is_none());
        assert!(parse_move_uci("♟e2e4", &mut board).is_none());
    }

    #[test]
    fn parse_move_uci_token_trop_court() {
        let mut board = Board::start_position();
        assert!(parse_move_uci("e2", &mut board).is_none());
        assert!(parse_move_uci("", &mut board).is_none());
    }

    #[test]
    fn parse_move_uci_coup_legal_reconnu() {
        let mut board = Board::start_position();
        // e2e4 is legal from the starting position.
        assert!(parse_move_uci("e2e4", &mut board).is_some());
    }

    #[test]
    fn parse_move_uci_coup_illegal_rejete() {
        let mut board = Board::start_position();
        // e2e5 (push of 3 squares) is illegal.
        assert!(parse_move_uci("e2e5", &mut board).is_none());
    }
}
