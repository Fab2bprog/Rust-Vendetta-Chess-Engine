// =============================================================================
// Vendetta Chess Motor — src/bin/extract_positions.rs
//
// Role: Step 1 of Texel Tuning. Reads the filtered PGN file (lichess_filtered_
//        2026-05.pgn), replays each game move by move with the engine it-
//        self (Board::make_move + legal generation to resolve the
//        SAN notation), and samples positions at regular intervals.
//
// Why a separate step:
//   Replaying 300,000 games (parsing SAN, generating legal moves to
//   disambiguate, etc.) is the most expensive step of the tuning pipeline.
//   By saving the result (FEN + game result) to an intermediate
//   file, this cost is paid only ONCE — all subsequent
//   iterations of the tuner (coordinate descent over dozens of passes) reread
//   this file directly, without ever reparsing PGN or regenerating moves.
//
// Output format (text, one position per line):
//   <FEN>;<result>
//   where result ∈ {1.0, 0.5, 0.0} from WHITE's point of view
//   (1.0 = White win, 0.5 = draw, 0.0 = Black win)
//
// Sampling:
//   - The first SKIP_PLIES half-moves are ignored (opening theory,
//     not very representative of the engine's own judgment)
//   - A position is kept every SAMPLE_INTERVAL half-moves after that
//   - No position is kept after an obvious checkmate/stalemate at the end of the PGN
//     (the last move has no useful "after" position to evaluate)
//
// Usage:
//   cargo run --release --bin extract_positions -- <input.pgn> <output.txt>
// =============================================================================

use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::time::Instant;

use vendetta_chess_motor::board::bitboard::init_attack_tables;
use vendetta_chess_motor::board::state::Board;
use vendetta_chess_motor::moves::generate_legal_moves;
use vendetta_chess_motor::utils::types::{Move, MoveFlags, Piece};

/// Ignore the first SKIP_PLIES half-moves of each game (known
/// opening theory, not very informative about evaluation quality).
const SKIP_PLIES: usize = 10;

/// Sample a position every SAMPLE_INTERVAL half-moves after
/// the opening zone ignored above.
const SAMPLE_INTERVAL: usize = 8;

// =============================================================================
// SAN parsing — resolving a move in standard algebraic notation
// =============================================================================

/// Converts a SAN piece character ('N','B','R','Q','K') into a Piece.
fn piece_from_san_char(c: char) -> Option<Piece> {
    match c {
        'N' => Some(Piece::Knight),
        'B' => Some(Piece::Bishop),
        'R' => Some(Piece::Rook),
        'Q' => Some(Piece::Queen),
        'K' => Some(Piece::King),
        _   => None,
    }
}

/// Resolves a SAN token (e.g. "Nf3", "exd5", "O-O", "e8=Q+") into a legal Move,
/// relying on generate_legal_moves() for disambiguation.
///
/// Principle: chess rules are not reimplemented here — instead we generate
/// all legal moves of the current position, then filter those that
/// match the SAN token (piece type, destination square, promotion,
/// file/rank disambiguation hints). The engine has already guaranteed that
/// this list is correct (perft 6/6) — SAN resolution therefore only needs to
/// find the matching move, not revalidate its legality.
fn resolve_san(board: &mut Board, raw: &str) -> Option<Move> {
    // Strip trailing annotations (+, #, !, ?) and spaces.
    let san: String = raw.trim().chars()
        .filter(|c| !matches!(c, '+' | '#' | '!' | '?'))
        .collect();
    if san.is_empty() { return None; }

    let legal = generate_legal_moves(board);
    let side  = board.side_to_move;

    // --- Castling ---
    if san == "O-O" || san == "0-0" {
        return legal.into_iter().find(|m| m.flags == MoveFlags::CastleKingside);
    }
    if san == "O-O-O" || san == "0-0-0" {
        return legal.into_iter().find(|m| m.flags == MoveFlags::CastleQueenside);
    }

    let chars: Vec<char> = san.chars().collect();

    // --- Promotion: "=X" suffix ---
    let mut promo_piece: Option<Piece> = None;
    let mut body = san.clone();
    if let Some(eq_pos) = san.find('=') {
        let promo_char = chars.get(eq_pos + 1).copied().unwrap_or('Q');
        promo_piece = piece_from_san_char(promo_char);
        body = san[..eq_pos].to_string();
    }

    let body_chars: Vec<char> = body.chars().collect();
    if body_chars.len() < 2 { return None; }

    // --- Destination square: always the last 2 characters of the body ---
    let dest_str: String = body_chars[body_chars.len() - 2..].iter().collect();
    let dest_sq = vendetta_chess_motor::utils::types::square_from_str(&dest_str)?;

    // --- Type of piece moved: leading uppercase letter, otherwise Pawn ---
    let (piece_type, rest_start) = match body_chars[0] {
        'N' | 'B' | 'R' | 'Q' | 'K' => (piece_from_san_char(body_chars[0])?, 1),
        _ => (Piece::Pawn, 0),
    };

    // --- Disambiguation characters between the piece and the destination square ---
    // (origin file and/or rank, capture 'x' ignored — already handled by
    // filtering on the bitboard of the piece that actually moves)
    let disambig: Vec<char> = body_chars[rest_start..body_chars.len() - 2]
        .iter()
        .copied()
        .filter(|c| *c != 'x')
        .collect();

    let disambig_file = disambig.iter().find(|c| ('a'..='h').contains(c)).copied();
    let disambig_rank = disambig.iter().find(|c| ('1'..='8').contains(c)).copied();

    // --- Filter legal moves matching this token ---
    let candidates: Vec<Move> = legal.into_iter().filter(|mv| {
        if mv.to != dest_sq { return false; }

        // The moving piece must be of the right type and the right color.
        match board.piece_at(mv.from) {
            Some((p, c)) if p == piece_type && c == side => {}
            _ => return false,
        }

        // Promotion: must match if specified.
        if let Some(pp) = promo_piece {
            if mv.promotion_piece() != Some(pp) { return false; }
        }

        // File/rank disambiguation if provided in the SAN.
        if let Some(f) = disambig_file {
            if (b'a' + (mv.from % 8)) as char != f { return false; }
        }
        if let Some(r) = disambig_rank {
            if (b'1' + (mv.from / 8)) as char != r { return false; }
        }

        true
    }).collect();

    if candidates.len() == 1 {
        Some(candidates[0])
    } else {
        // Insufficient disambiguation (rare, malformed SAN) — take the
        // first candidate rather than failing the whole game.
        candidates.into_iter().next()
    }
}

// =============================================================================
// Cleanup of Lichess annotations ({ [%eval ...] [%clk ...] })
// =============================================================================

/// Removes all brace-delimited blocks from a movetext line.
///
/// Lichess inserts annotations after each move, e.g.:
///   "1. c3 { [%eval 0.0] [%clk 0:10:00] } 1... e5 { [%eval 0.15] [...] } ..."
/// PGN braces never nest (no "{ { } }"), so a
/// simple depth counter is enough: characters are copied as long
/// as we are not inside a { } block, and everything else is ignored.
fn strip_braced_comments(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut depth = 0u32;
    for c in line.chars() {
        match c {
            '{' => depth += 1,
            '}' => { depth = depth.saturating_sub(1); }
            _ if depth == 0 => out.push(c),
            _ => {} // inside a comment: ignored
        }
    }
    out
}

// =============================================================================
// Entry point
// =============================================================================

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage : {} <entrée.pgn> <sortie.txt>", args[0]);
        std::process::exit(1);
    }
    let input_path  = &args[1];
    let output_path = &args[2];

    init_attack_tables();

    let in_file = File::open(input_path).expect("Impossible d'ouvrir le fichier PGN d'entrée");
    let reader  = BufReader::with_capacity(4 << 20, in_file);

    let out_file = File::create(output_path).expect("Impossible de créer le fichier de sortie");
    let mut writer = BufWriter::with_capacity(4 << 20, out_file);

    let mut result: Option<f32> = None;     // result of the current game (from White's point of view)
    let mut moves_san: Vec<String> = Vec::with_capacity(128);

    let mut games_seen:      u64 = 0;
    let mut games_replayed:  u64 = 0;
    let mut games_failed:    u64 = 0;
    let mut positions_kept:  u64 = 0;

    let start = Instant::now();

    // Counter of displayed diagnostics (limited to 5, to avoid flooding the output).
    let mut debug_shown: u32 = 0;

    let flush_game = |result: Option<f32>, moves_san: &[String], writer: &mut BufWriter<File>,
                          games_replayed: &mut u64, games_failed: &mut u64, positions_kept: &mut u64,
                          debug_shown: &mut u32| {
        let Some(res) = result else { return };
        if moves_san.is_empty() { return; }

        let mut board = Board::start_position();
        let mut ply = 0usize;
        let mut ok = true;

        for san in moves_san {
            let mv = match resolve_san(&mut board, san) {
                Some(m) => m,
                None => {
                    if *debug_shown < 5 {
                        eprintln!(
                            "DEBUG échec — coup #{} = \"{}\" — coups de la partie : {:?}",
                            ply + 1, san, moves_san
                        );
                        *debug_shown += 1;
                    }
                    ok = false;
                    break;
                }
            };
            board.make_move(mv);
            ply += 1;

            if ply > SKIP_PLIES && (ply - SKIP_PLIES).is_multiple_of(SAMPLE_INTERVAL) {
                let fen = board.to_fen();
                writeln!(writer, "{};{}", fen, res).expect("Erreur d'écriture");
                *positions_kept += 1;
            }
        }

        if ok { *games_replayed += 1; } else { *games_failed += 1; }
    };

    for line in reader.lines() {
        let line = line.expect("Erreur de lecture du fichier PGN");

        if line.starts_with("[Result ") {
            // New game: first close out the previous one.
            flush_game(result, &moves_san, &mut writer, &mut games_replayed, &mut games_failed, &mut positions_kept, &mut debug_shown);
            games_seen += 1;
            moves_san.clear();

            result = if line.contains("1-0") {
                Some(1.0)
            } else if line.contains("0-1") {
                Some(0.0)
            } else if line.contains("1/2-1/2") {
                Some(0.5)
            } else {
                None // unknown result ("*") — game skipped
            };
        } else if !line.starts_with('[') && !line.trim().is_empty() {
            // Move line: Lichess inserts annotations between braces
            // after each move, e.g.: "1. c3 { [%eval 0.0] [%clk 0:10:00] } 1... e5 ...".
            // Without removing them BEFORE splitting on spaces, each word
            // inside ("{", "[%eval", "0.0]", "[%clk"...) would be inserted
            // into the move list as if it were a SAN move —
            // this is what caused nearly all games to fail.
            let cleaned = strip_braced_comments(&line);

            // Tokens separated by spaces, stripping move numbers
            // ("12." or "12...", whether or not attached to the following move).
            for token in cleaned.split_whitespace() {
                let mut t = token.trim();
                if t.is_empty() { continue; }

                // IMPORTANT: check the result BEFORE stripping the
                // numeric prefix. "1-0"/"0-1" start with a digit — if they
                // went through the move-number cleanup first, they
                // would be mangled ("1-0" → "-0") and would no longer match
                // any of the strings below, ending up pushed into
                // moves_san as a fake move at the end of the game (which would then
                // cause the replay to fail despite all real moves being correct).
                if t == "1-0" || t == "0-1" || t == "1/2-1/2" || t == "*" {
                    continue;
                }

                // Possible move number: "12.", "12...", or attached without a
                // space to the move itself ("12.Nf3", "12...Nf3" — some
                // PGN exports don't put a space after the period). Only the
                // numeric prefix + dots is discarded, never the move.
                if t.chars().next().unwrap().is_ascii_digit() {
                    let after_digits = t.trim_start_matches(|c: char| c.is_ascii_digit());
                    let after_dots   = after_digits.trim_start_matches('.');
                    if after_dots.is_empty() { continue; } // "12." alone, nothing to keep
                    t = after_dots;
                }

                moves_san.push(t.to_string());
            }
        }

        if games_seen > 0 && games_seen.is_multiple_of(20_000) {
            eprintln!(
                "{:>8} parties vues — {:>8} rejouées — {:>6} échouées — {:>9} positions — {:.1}s",
                games_seen, games_replayed, games_failed, positions_kept, start.elapsed().as_secs_f64()
            );
        }
    }
    // Last game in the file.
    flush_game(result, &moves_san, &mut writer, &mut games_replayed, &mut games_failed, &mut positions_kept, &mut debug_shown);

    writer.flush().expect("Erreur de vidage du buffer d'écriture");

    eprintln!();
    eprintln!("Terminé en {:.1}s", start.elapsed().as_secs_f64());
    eprintln!("Parties vues      : {}", games_seen);
    eprintln!("Parties rejouées  : {}", games_replayed);
    eprintln!("Parties échouées  : {}", games_failed);
    eprintln!("Positions gardées : {}", positions_kept);
}
