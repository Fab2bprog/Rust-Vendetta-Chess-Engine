// =============================================================================
// Vendetta Chess Motor — src/eval/threats.rs
//
// Rôle : Détection de menaces statiques — pénalise une pièce attaquée par une
//        pièce adverse de moindre valeur, ou une pièce non défendue attaquée
//        ("pièce en prise" / "hanging piece").
//
//        Indépendant de la recherche tactique : aide l'évaluation à reconnaître
//        une vulnérabilité réelle même quand la quiescence n'a pas encore (ou
//        ne pourra jamais, à la profondeur courante) exploré la capture qui la
//        sanctionnerait. Réduit les positions où le moteur sous-estime une
//        pièce en danger simplement parce que la ligne tactique exacte est
//        hors de portée de la recherche à cet instant.
//
// Principe (deux signaux distincts, cumulables) :
//   1. "Menacée par une pièce moins chère" — un Cavalier/Fou attaqué par un
//      pion, une Tour attaquée par un pion ou une pièce mineure, une Dame
//      attaquée par n'importe quoi de moins cher. Dans ces cas, le SEE
//      favorise presque toujours l'attaquant : peu importe la défense au
//      sens du contrôle de case, l'échange reste mauvais pour le camp menacé.
//   2. "En prise" — une pièce (hors pion et roi) attaquée et qu'AUCUNE pièce
//      amie ne peut reprendre sur sa case. Signal plus généraliste, qui
//      capture les pièces isolées même quand l'attaquant n'est pas moins cher.
//
// Simplification volontaire — PAS de SEE complet ici :
//   Un SEE complet par pièce à CHAQUE appel d'evaluate() serait bien trop
//   coûteux (evaluate() est appelé à chaque nœud feuille). On se contente
//   d'un contrôle de case bon marché (bitboards d'attaque déjà nécessaires
//   ailleurs), cohérent avec le niveau de simplicité de mobility.rs,
//   king_safety.rs et center.rs. Les pénalités sont volontairement modestes
//   (un "nudge" évaluatif, pas une ré-estimation du matériel) : la recherche
//   tactique (SEE, quiescence) reste responsable de la précision réelle
//   quand elle peut voir la ligne.
// =============================================================================

use crate::board::state::Board;
use crate::board::bitboard::{
    knight_attacks, bishop_attacks, rook_attacks, queen_attacks, king_attacks,
    white_pawn_attacks, black_pawn_attacks,
};
use crate::utils::types::{Color, Piece};

/// Pénalité pour une pièce attaquée par une pièce adverse de moindre valeur
/// (indépendamment de toute défense). Valeur initiale raisonnable — comme le
/// reste de l'évaluation positionnelle, à affiner par Texel Tuning (v5
/// éventuelle, voir CLAUDE.md "Chantiers futurs").
const THREATENED_BY_LESSER_PENALTY: i32 = 25;

/// Pénalité supplémentaire pour une pièce attaquée et totalement non
/// défendue ("en prise"). Cumulable avec la pénalité ci-dessus si les deux
/// conditions sont réunies (ex : Tour attaquée par un Cavalier ET non
/// défendue par ailleurs).
const HANGING_PENALTY: i32 = 20;

/// Bitboard des cases attaquées par TOUTES les pièces de `color` (pions
/// inclus). Sert de proxy bon marché de "défense" : si `color` attaque une
/// case, elle peut y reprendre si une de ses pièces y est capturée.
///
/// Volontairement SANS masquer les cases occupées par les propres pièces de
/// `color` (contrairement à mobility.rs, qui exclut `!own_pieces` pour ne
/// compter que les cases réellement accessibles) : ici, une case occupée par
/// une pièce amie ET attaquée par une autre pièce amie est précisément ce qui
/// définit une case "défendue".
fn own_attack_bitboard(board: &Board, color: Color) -> u64 {
    let occupied = board.all_pieces;
    let mut attacks = 0u64;

    let pawns = board.pieces[color.index()][Piece::Pawn.index()];
    attacks |= if color == Color::White {
        white_pawn_attacks(pawns)
    } else {
        black_pawn_attacks(pawns)
    };

    let mut bb = board.pieces[color.index()][Piece::Knight.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        attacks |= knight_attacks(sq);
    }

    let mut bb = board.pieces[color.index()][Piece::Bishop.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        attacks |= bishop_attacks(sq, occupied);
    }

    let mut bb = board.pieces[color.index()][Piece::Rook.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        attacks |= rook_attacks(sq, occupied);
    }

    let mut bb = board.pieces[color.index()][Piece::Queen.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        attacks |= queen_attacks(sq, occupied);
    }

    let king_bb = board.pieces[color.index()][Piece::King.index()];
    if king_bb != 0 {
        attacks |= king_attacks(king_bb.trailing_zeros() as u8);
    }

    attacks
}

/// Évalue les menaces subies par `color` (toujours une pénalité, jamais un
/// bonus — être menacé n'est jamais positif). Retourne un score négatif ou
/// nul, du point de vue de `color`.
pub fn threats_score(board: &Board, color: Color) -> i32 {
    let enemy    = color.opposite();
    let occupied = board.all_pieces;

    // --- Attaques adverses, PAR TYPE de pièce (pour comparer les valeurs) ---
    let enemy_pawns = board.pieces[enemy.index()][Piece::Pawn.index()];
    let enemy_pawn_attacks = if enemy == Color::White {
        white_pawn_attacks(enemy_pawns)
    } else {
        black_pawn_attacks(enemy_pawns)
    };

    let mut enemy_minor_attacks = 0u64; // Cavaliers + Fous adverses combinés
    let mut bb = board.pieces[enemy.index()][Piece::Knight.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        enemy_minor_attacks |= knight_attacks(sq);
    }
    let mut bb = board.pieces[enemy.index()][Piece::Bishop.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        enemy_minor_attacks |= bishop_attacks(sq, occupied);
    }

    let mut enemy_rook_attacks = 0u64;
    let mut bb = board.pieces[enemy.index()][Piece::Rook.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        enemy_rook_attacks |= rook_attacks(sq, occupied);
    }

    let mut enemy_queen_attacks = 0u64;
    let mut bb = board.pieces[enemy.index()][Piece::Queen.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        enemy_queen_attacks |= queen_attacks(sq, occupied);
    }

    let enemy_king_bb = board.pieces[enemy.index()][Piece::King.index()];
    let enemy_king_attacks = if enemy_king_bb != 0 {
        king_attacks(enemy_king_bb.trailing_zeros() as u8)
    } else {
        0
    };

    let enemy_all_attacks = enemy_pawn_attacks | enemy_minor_attacks
        | enemy_rook_attacks | enemy_queen_attacks | enemy_king_attacks;

    // --- Cases défendues par MON propre camp ---
    let my_defended = own_attack_bitboard(board, color);

    let mut score = 0i32;

    // --- Signal 1 : menacée par une pièce de moindre valeur ---

    // Cavaliers/Fous menacés par un pion adverse.
    let my_minors = board.pieces[color.index()][Piece::Knight.index()]
        | board.pieces[color.index()][Piece::Bishop.index()];
    score -= (my_minors & enemy_pawn_attacks).count_ones() as i32 * THREATENED_BY_LESSER_PENALTY;

    // Tours menacées par un pion ou une pièce mineure adverse.
    let my_rooks = board.pieces[color.index()][Piece::Rook.index()];
    let rook_threats = enemy_pawn_attacks | enemy_minor_attacks;
    score -= (my_rooks & rook_threats).count_ones() as i32 * THREATENED_BY_LESSER_PENALTY;

    // Dame menacée par n'importe quelle pièce moins chère qu'elle.
    let my_queens = board.pieces[color.index()][Piece::Queen.index()];
    let queen_threats = enemy_pawn_attacks | enemy_minor_attacks | enemy_rook_attacks;
    score -= (my_queens & queen_threats).count_ones() as i32 * THREATENED_BY_LESSER_PENALTY;

    // --- Signal 2 : pièce en prise (attaquée ET non défendue), hors pion/roi ---
    let my_pieces_no_king_pawn = board.occupancy[color.index()]
        & !board.pieces[color.index()][Piece::Pawn.index()]
        & !board.pieces[color.index()][Piece::King.index()];
    let hanging = my_pieces_no_king_pawn & enemy_all_attacks & !my_defended;
    score -= hanging.count_ones() as i32 * HANGING_PENALTY;

    score
}

/// Calcule le différentiel de menaces du point de vue du joueur actif.
/// Score positif = avantage pour le joueur actif (donc ici, presque toujours
/// "l'adversaire est plus menacé que moi" — les deux composantes sont des
/// pénalités, jamais des bonus).
pub fn threats_eval(board: &Board) -> i32 {
    let white_score = threats_score(board, Color::White);
    let black_score = threats_score(board, Color::Black);
    let diff = white_score - black_score;

    if board.side_to_move == Color::White { diff } else { -diff }
}
