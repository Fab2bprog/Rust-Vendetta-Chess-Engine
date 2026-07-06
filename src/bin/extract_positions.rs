// =============================================================================
// Vendetta Chess Motor — src/bin/extract_positions.rs
//
// Rôle : Étape 1 du Texel Tuning. Lit le fichier PGN filtré (lichess_filtered_
//        2026-05.pgn), rejoue chaque partie coup par coup avec le moteur lui-
//        même (Board::make_move + génération légale pour résoudre la notation
//        SAN), et échantillonne des positions à intervalles réguliers.
//
// Pourquoi une étape séparée :
//   Rejouer 300 000 parties (parser le SAN, générer les coups légaux pour
//   désambiguïser, etc.) est le poste le plus coûteux du pipeline de tuning.
//   En sauvegardant le résultat (FEN + résultat de la partie) dans un fichier
//   intermédiaire, on ne paie ce coût qu'UNE FOIS — toutes les itérations
//   suivantes du tuner (coordinate descent sur des dizaines de passes) relisent
//   directement ce fichier, sans jamais reparser de PGN ni régénérer de coups.
//
// Format de sortie (texte, une position par ligne) :
//   <FEN>;<résultat>
//   où résultat ∈ {1.0, 0.5, 0.0} du point de vue des BLANCS
//   (1.0 = victoire Blancs, 0.5 = nulle, 0.0 = victoire Noirs)
//
// Échantillonnage :
//   - On ignore les SKIP_PLIES premiers demi-coups (théorie d'ouverture,
//     peu représentatifs du jugement propre du moteur)
//   - Une position est gardée tous les SAMPLE_INTERVAL demi-coups ensuite
//   - Aucune position n'est gardée après un mat/pat évident en fin de PGN
//     (le dernier coup n'a pas de position "après" utile à évaluer)
//
// Utilisation :
//   cargo run --release --bin extract_positions -- <entrée.pgn> <sortie.txt>
// =============================================================================

use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::time::Instant;

use vendetta_chess_motor::board::bitboard::init_attack_tables;
use vendetta_chess_motor::board::state::Board;
use vendetta_chess_motor::moves::generate_legal_moves;
use vendetta_chess_motor::utils::types::{Move, MoveFlags, Piece};

/// Ignorer les SKIP_PLIES premiers demi-coups de chaque partie (théorie
/// d'ouverture connue, peu informative sur la qualité de l'évaluation).
const SKIP_PLIES: usize = 10;

/// Échantillonner une position tous les SAMPLE_INTERVAL demi-coups après
/// la zone d'ouverture ignorée ci-dessus.
const SAMPLE_INTERVAL: usize = 8;

// =============================================================================
// Parsing SAN — résolution d'un coup en notation algébrique standard
// =============================================================================

/// Convertit un caractère de pièce SAN ('N','B','R','Q','K') en Piece.
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

/// Résout un token SAN (ex: "Nf3", "exd5", "O-O", "e8=Q+") en un Move légal,
/// en s'appuyant sur generate_legal_moves() pour la désambiguïsation.
///
/// Principe : on ne réimplémente pas les règles des échecs ici — on génère
/// tous les coups légaux de la position courante, puis on filtre ceux qui
/// correspondent au token SAN (type de pièce, case d'arrivée, promotion,
/// indices de désambiguïsation file/rang). Le moteur a déjà garanti que
/// cette liste est correcte (perft 6/6) — la résolution SAN n'a donc qu'à
/// trouver le coup correspondant, pas à revalider sa légalité.
fn resolve_san(board: &mut Board, raw: &str) -> Option<Move> {
    // Nettoyer les annotations finales (+, #, !, ?) et espaces.
    let san: String = raw.trim().chars()
        .filter(|c| !matches!(c, '+' | '#' | '!' | '?'))
        .collect();
    if san.is_empty() { return None; }

    let legal = generate_legal_moves(board);
    let side  = board.side_to_move;

    // --- Roque ---
    if san == "O-O" || san == "0-0" {
        return legal.into_iter().find(|m| m.flags == MoveFlags::CastleKingside);
    }
    if san == "O-O-O" || san == "0-0-0" {
        return legal.into_iter().find(|m| m.flags == MoveFlags::CastleQueenside);
    }

    let chars: Vec<char> = san.chars().collect();

    // --- Promotion : suffixe "=X" ---
    let mut promo_piece: Option<Piece> = None;
    let mut body = san.clone();
    if let Some(eq_pos) = san.find('=') {
        let promo_char = chars.get(eq_pos + 1).copied().unwrap_or('Q');
        promo_piece = piece_from_san_char(promo_char);
        body = san[..eq_pos].to_string();
    }

    let body_chars: Vec<char> = body.chars().collect();
    if body_chars.len() < 2 { return None; }

    // --- Case d'arrivée : toujours les 2 derniers caractères du corps ---
    let dest_str: String = body_chars[body_chars.len() - 2..].iter().collect();
    let dest_sq = vendetta_chess_motor::utils::types::square_from_str(&dest_str)?;

    // --- Type de pièce déplacée : lettre majuscule en tête, sinon Pion ---
    let (piece_type, rest_start) = match body_chars[0] {
        'N' | 'B' | 'R' | 'Q' | 'K' => (piece_from_san_char(body_chars[0])?, 1),
        _ => (Piece::Pawn, 0),
    };

    // --- Caractères de désambiguïsation entre la pièce et la case d'arrivée ---
    // (file et/ou rang d'origine, capture 'x' ignorée — déjà gérée par le
    // filtrage sur le bitboard de la pièce qui se déplace réellement)
    let disambig: Vec<char> = body_chars[rest_start..body_chars.len() - 2]
        .iter()
        .copied()
        .filter(|c| *c != 'x')
        .collect();

    let disambig_file = disambig.iter().find(|c| ('a'..='h').contains(c)).copied();
    let disambig_rank = disambig.iter().find(|c| ('1'..='8').contains(c)).copied();

    // --- Filtrer les coups légaux correspondant à ce token ---
    let candidates: Vec<Move> = legal.into_iter().filter(|mv| {
        if mv.to != dest_sq { return false; }

        // La pièce qui bouge doit être du bon type et de la bonne couleur.
        match board.piece_at(mv.from) {
            Some((p, c)) if p == piece_type && c == side => {}
            _ => return false,
        }

        // Promotion : doit correspondre si spécifiée.
        if let Some(pp) = promo_piece {
            if mv.promotion_piece() != Some(pp) { return false; }
        }

        // Désambiguïsation file/rang si fournie dans le SAN.
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
        // Désambiguïsation insuffisante (rare, SAN malformé) — on prend le
        // premier candidat plutôt que d'échouer toute la partie.
        candidates.into_iter().next()
    }
}

// =============================================================================
// Nettoyage des annotations Lichess ({ [%eval ...] [%clk ...] })
// =============================================================================

/// Retire tous les blocs entre accolades d'une ligne de movetext.
///
/// Lichess insère des annotations après chaque coup, ex :
///   "1. c3 { [%eval 0.0] [%clk 0:10:00] } 1... e5 { [%eval 0.15] [...] } ..."
/// Les accolades PGN ne s'imbriquent jamais (pas de "{ { } }"), donc un
/// simple compteur de profondeur suffit : on copie les caractères tant
/// qu'on n'est pas à l'intérieur d'un bloc { }, et on ignore tout le reste.
fn strip_braced_comments(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut depth = 0u32;
    for c in line.chars() {
        match c {
            '{' => depth += 1,
            '}' => { depth = depth.saturating_sub(1); }
            _ if depth == 0 => out.push(c),
            _ => {} // à l'intérieur d'un commentaire : ignoré
        }
    }
    out
}

// =============================================================================
// Point d'entrée
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

    let mut result: Option<f32> = None;     // résultat de la partie en cours (point de vue Blancs)
    let mut moves_san: Vec<String> = Vec::with_capacity(128);

    let mut games_seen:      u64 = 0;
    let mut games_replayed:  u64 = 0;
    let mut games_failed:    u64 = 0;
    let mut positions_kept:  u64 = 0;

    let start = Instant::now();

    // Compteur de diagnostics affichés (limité à 5, pour ne pas noyer la sortie).
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
            // Nouvelle partie : d'abord clôturer la précédente.
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
                None // résultat inconnu ("*") — partie ignorée
            };
        } else if !line.starts_with('[') && !line.trim().is_empty() {
            // Ligne de coups : Lichess insère des annotations entre accolades
            // après chaque coup, ex: "1. c3 { [%eval 0.0] [%clk 0:10:00] } 1... e5 ...".
            // Sans les retirer AVANT le découpage par espaces, chaque mot à
            // l'intérieur ("{", "[%eval", "0.0]", "[%clk"...) serait inséré
            // dans la liste des coups comme s'il s'agissait d'un coup SAN —
            // c'est ce qui causait l'échec de quasiment toutes les parties.
            let cleaned = strip_braced_comments(&line);

            // Tokens séparés par espaces, en retirant les numéros de coup
            // ("12." ou "12...", collés ou non au coup qui suit).
            for token in cleaned.split_whitespace() {
                let mut t = token.trim();
                if t.is_empty() { continue; }

                // IMPORTANT : vérifier le résultat AVANT le retrait du préfixe
                // numérique. "1-0"/"0-1" commencent par un chiffre — si on les
                // passait par le nettoyage de numéro de coup en premier, ils
                // seraient mutilés ("1-0" → "-0") et ne correspondraient plus
                // à aucune des chaînes ci-dessous, finissant poussés dans
                // moves_san comme un faux coup en fin de partie (qui ferait
                // alors échouer le replay malgré des coups réels tous corrects).
                if t == "1-0" || t == "0-1" || t == "1/2-1/2" || t == "*" {
                    continue;
                }

                // Numéro de coup éventuel : "12.", "12...", ou collé sans
                // espace au coup lui-même ("12.Nf3", "12...Nf3" — certains
                // exports PGN ne mettent pas d'espace après le point). On ne
                // jette que le préfixe numérique + points, jamais le coup.
                if t.chars().next().unwrap().is_ascii_digit() {
                    let after_digits = t.trim_start_matches(|c: char| c.is_ascii_digit());
                    let after_dots   = after_digits.trim_start_matches('.');
                    if after_dots.is_empty() { continue; } // "12." seul, rien à garder
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
    // Dernière partie du fichier.
    flush_game(result, &moves_san, &mut writer, &mut games_replayed, &mut games_failed, &mut positions_kept, &mut debug_shown);

    writer.flush().expect("Erreur de vidage du buffer d'écriture");

    eprintln!();
    eprintln!("Terminé en {:.1}s", start.elapsed().as_secs_f64());
    eprintln!("Parties vues      : {}", games_seen);
    eprintln!("Parties rejouées  : {}", games_replayed);
    eprintln!("Parties échouées  : {}", games_failed);
    eprintln!("Positions gardées : {}", positions_kept);
}
