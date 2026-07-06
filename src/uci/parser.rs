// =============================================================================
// Vendetta Chess Motor — src/uci/parser.rs
//
// Rôle : Analyse (parsing) des commandes UCI reçues sur l'entrée standard.
//        Le protocole UCI communique par échange de texte sur stdin/stdout.
//        Ce module transforme les lignes de texte en commandes structurées.
//
// Commandes UCI gérées :
//   - uci          : identification du moteur
//   - isready      : vérification de disponibilité
//   - ucinewgame   : début d'une nouvelle partie
//   - position     : définition de la position courante
//   - go           : lancement de la recherche
//   - stop         : arrêt de la recherche
//   - setoption    : configuration des options (niveau de difficulté, TT size...)
//   - quit         : fermeture du moteur
//
// Contenu :
//   - UciCommand : enum des commandes UCI
//   - parse_command() : transforme une ligne en UciCommand
//   - parse_go_params() : analyse les paramètres de la commande go
// =============================================================================

use crate::utils::types::square_from_str;
use crate::utils::types::Move;
use crate::search::SearchConfig;

/// Commandes UCI reconnues par Vendetta Chess Motor.
#[derive(Debug)]
pub enum UciCommand {
    /// Identification du moteur : "uci"
    Uci,
    /// Vérification disponibilité : "isready"
    IsReady,
    /// Nouvelle partie : "ucinewgame"
    UciNewGame,
    /// Position à analyser : "position [startpos|fen <fen>] [moves <move1> ...]"
    Position {
        /// FEN de la position de départ (None = position initiale).
        fen: Option<String>,
        /// Liste des coups à jouer depuis la position de départ.
        moves: Vec<String>,
    },
    /// Lancer la recherche : "go [wtime <ms>] [btime <ms>] ..."
    Go(SearchConfig),
    /// Arrêter la recherche : "stop"
    Stop,
    /// L'adversaire a joué le coup prédit : "ponderhit"
    /// Le moteur doit basculer du mode ponder en recherche normale avec gestion du temps.
    PonderHit,
    /// Configurer une option : "setoption name <nom> value <valeur>"
    SetOption {
        name:  String,
        value: String,
    },
    /// Quitter le moteur : "quit"
    Quit,
    /// Activer ou désactiver le mode debug : "debug on|off"
    /// En mode debug, le moteur peut émettre des "info string" supplémentaires.
    Debug {
        /// true = debug activé, false = debug désactivé.
        on: bool,
    },
    /// Enregistrement du moteur : "register later|name <x> code <y>" (ou "register").
    /// Vendetta Chess Motor n'utilise AUCUNE protection anti-copie : la commande
    /// est acceptée sans action. Déclarée explicitement pour couvrir l'intégralité
    /// des commandes de la spec UCI (au lieu de la traiter comme inconnue).
    Register,
    /// Commande non reconnue (on l'ignore silencieusement — requis par UCI).
    Unknown,
}

/// Analyse une ligne de texte et retourne la commande UCI correspondante.
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

/// Analyse la commande "position".
/// Format : "position startpos [moves e2e4 e7e5 ...]"
///       ou "position fen <fen> [moves e2e4 e7e5 ...]"
fn parse_position_command(parts: &[&str]) -> UciCommand {
    if parts.is_empty() {
        return UciCommand::Unknown;
    }

    let mut fen: Option<String> = None;
    let mut moves: Vec<String> = Vec::new();
    let mut i = 0;

    if parts[0] == "startpos" {
        // Position initiale
        fen = None;
        i = 1;
    } else if parts[0] == "fen" {
        // FEN fournie : lire jusqu'au mot "moves" ou la fin
        i = 1;
        let mut fen_parts: Vec<&str> = Vec::new();
        while i < parts.len() && parts[i] != "moves" {
            fen_parts.push(parts[i]);
            i += 1;
        }
        fen = Some(fen_parts.join(" "));
    }

    // Lire les coups si présents
    if i < parts.len() && parts[i] == "moves" {
        i += 1;
        while i < parts.len() {
            moves.push(parts[i].to_string());
            i += 1;
        }
    }

    UciCommand::Position { fen, moves }
}

/// Retourne true si le token est un mot-clé connu de la commande "go".
/// Utilisé pour détecter la fin d'une liste "searchmoves".
#[inline]
fn is_go_keyword(s: &str) -> bool {
    matches!(s,
        "wtime" | "btime" | "winc" | "binc" | "movestogo"
        | "depth" | "movetime" | "infinite" | "ponder" | "searchmoves"
        | "nodes" | "mate"
    )
}

/// Analyse la commande "go" et ses paramètres.
/// Format : "go [wtime <ms>] [btime <ms>] [winc <ms>] [binc <ms>]
///               [movestogo <n>] [depth <n>] [movetime <ms>] [infinite]
///               [nodes <n>] [mate <n>]
///               [searchmoves <coup1> <coup2> ...]"
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
                // Consommer tous les tokens suivants jusqu'au prochain mot-clé "go"
                // ou la fin de la ligne.
                i += 1;
                while i < parts.len() && !is_go_keyword(parts[i]) {
                    // Validation légère : un coup UCI fait 4 ou 5 caractères (ex: e2e4, e7e8q).
                    // On accepte sans valider davantage (la validation légale se fait côté moteur).
                    if parts[i].len() >= 4 {
                        config.searchmoves.push(parts[i].to_string());
                    }
                    i += 1;
                }
                // i pointe maintenant sur le prochain mot-clé (ou hors bornes).
                // Le `continue` évite le `i += 1` en bas de boucle pour ne pas le sauter.
                continue;
            }
            _ => {}
        }
        i += 1;
    }

    config
}

/// Analyse la commande "setoption".
/// Format : "setoption name <nom> value <valeur>"
fn parse_setoption_command(parts: &[&str]) -> UciCommand {
    let mut name  = String::new();
    let mut value = String::new();
    let mut i = 0;

    if i < parts.len() && parts[i] == "name" {
        i += 1;
        // Lire le nom jusqu'à "value"
        let mut name_parts = Vec::new();
        while i < parts.len() && parts[i] != "value" {
            name_parts.push(parts[i]);
            i += 1;
        }
        name = name_parts.join(" ");
    }

    if i < parts.len() && parts[i] == "value" {
        i += 1;
        // Lire la valeur jusqu'à la fin
        value = parts[i..].join(" ");
    }

    UciCommand::SetOption { name, value }
}

/// Analyse la commande "debug".
/// Format : "debug on" ou "debug off"
/// Tout autre valeur est ignorée (on reste dans l'état courant).
fn parse_debug_command(parts: &[&str]) -> UciCommand {
    match parts.first().copied() {
        Some("on")  => UciCommand::Debug { on: true  },
        Some("off") => UciCommand::Debug { on: false },
        _           => UciCommand::Unknown,
    }
}

/// Convertit une notation UCI de coup (ex: "e2e4", "e7e8q") en Move légal.
///
/// Retourne None si la notation est invalide OU si le coup n'est pas légal
/// dans la position courante. Les flags du Move retourné proviennent toujours
/// du générateur de coups légaux, ce qui garantit leur exactitude.
pub fn parse_move_uci(mv_str: &str, board: &mut crate::board::state::Board) -> Option<Move> {
    // Un coup UCI est TOUJOURS de l'ASCII (ex: "e2e4", "e7e8q").
    // On rejette d'emblée tout token non-ASCII : sans ce garde-fou, le
    // découpage par octets `&mv_str[0..2]` plus bas paniquerait
    // ("byte index N is not a char boundary") si une frontière d'octet
    // tombait au milieu d'un caractère multi-octets (ex: "🙂e4").
    // mv_str.len() est une longueur en octets ; sur de l'ASCII pur,
    // 1 octet = 1 caractère, donc les indices [0..2] et [2..4] sont sûrs.
    if !mv_str.is_ascii() || mv_str.len() < 4 {
        return None;
    }

    let from = square_from_str(&mv_str[0..2])?;
    let to   = square_from_str(&mv_str[2..4])?;

    // Pièce de promotion (5e caractère optionnel, insensible à la casse)
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

    // Valider le coup contre la liste des coups légaux.
    // On cherche le Move légal dont (from, to, promotion) correspondent.
    // Cette approche garantit :
    //   1. Qu'aucun coup illégal ne peut être joué (y compris les coups qui
    //      exposeraient le roi ou violeraient les règles du roque / en passant).
    //   2. Que les flags MoveFlags sont toujours exacts (fournis par le générateur,
    //      pas reconstruits heuristiquement depuis la chaîne UCI).
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

    /// Non-régression (audit stabilité 2026-06-23, bug n°1) :
    /// un token de coup non-ASCII ne doit JAMAIS faire paniquer le moteur
    /// (le découpage par octets `&mv_str[0..2]` paniquait avant le correctif).
    #[test]
    fn parse_move_uci_non_ascii_ne_panique_pas() {
        let mut board = Board::start_position();
        // Emoji 4 octets : sans le garde-fou is_ascii(), &mv_str[0..2]
        // couperait au milieu du caractère → panic.
        assert!(parse_move_uci("🙂e4", &mut board).is_none());
        // Caractère accentué (2 octets) placé pour fausser la frontière d'octet.
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
        // e2e4 est légal depuis la position de départ.
        assert!(parse_move_uci("e2e4", &mut board).is_some());
    }

    #[test]
    fn parse_move_uci_coup_illegal_rejete() {
        let mut board = Board::start_position();
        // e2e5 (poussée de 3 cases) est illégal.
        assert!(parse_move_uci("e2e5", &mut board).is_none());
    }
}
