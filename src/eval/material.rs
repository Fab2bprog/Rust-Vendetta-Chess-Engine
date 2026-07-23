// =============================================================================
// Vendetta Chess Motor — src/eval/material.rs
//
// Rôle : Définit les valeurs matérielles des pièces et calcule le déséquilibre
//        matériel entre les deux camps.
//
// Contenu :
//   - Valeurs des pièces en centipions (100 centipions = 1 pion)
//   - Calcul du score matériel pour une couleur
//   - Calcul du différentiel matériel (Blanc - Noir, du point de vue du joueur)
//
// Convention de score :
//   - Score positif → favorable au joueur qui a le trait
//   - Score négatif → défavorable au joueur qui a le trait
//   - Les valeurs sont en centipions (unité standard des moteurs d'échecs)
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;

/// Valeurs des pièces en centipions.
///
/// Calibrées par Texel Tuning (v3, échelle sigmoïde K=748.22 calibrée sur
/// les données avant le tuning des poids — voir src/bin/tuner.rs) sur
/// 2 464 785 positions issues de 302 864 parties Lichess Rapid/Classical,
/// Elo ≥ 2100 (dump mai 2026). Validé : rapports entre pièces cohérents
/// (Fou > Cavalier, Tour bien au-dessus, Dame la plus forte) et bonus de
/// pions passés strictement positifs et croissants — contrairement à deux
/// tentatives de tuning précédentes (sans calibrage K) qui avaient produit
/// un effondrement d'échelle et des bonus de pions passés au signe incohérent.
///
/// Anciennes valeurs (avant tuning) : Pion=100, Cavalier=320, Fou=330,
/// Tour=500, Dame=900 — conservées en commentaire pour rollback si le test
/// A/B en partie réelle ne confirme pas l'amélioration.
pub const PIECE_VALUE: [i32; 6] = [
    100,   // Pion (ancre fixe du tuning, non ajustée)
    216,   // Cavalier (était 320)
    224,   // Fou (était 330)
    382,   // Tour (était 500)
    817,   // Dame (était 900)
    20000, // Roi (valeur très élevée pour forcer sa protection — non concerné par le tuning)
];

/// Retourne la valeur d'une pièce en centipions.
#[inline]
pub fn piece_value(piece: Piece) -> i32 {
    PIECE_VALUE[piece.index()]
}

/// Calcule le score matériel total pour une couleur donnée.
/// Somme les valeurs de toutes les pièces de cette couleur (sans le roi).
pub fn material_score(board: &Board, color: Color) -> i32 {
    let mut score = 0i32;

    // On additionne la valeur de chaque type de pièce (hors roi)
    for piece in [Piece::Pawn, Piece::Knight, Piece::Bishop, Piece::Rook, Piece::Queen] {
        let count = board.pieces[color.index()][piece.index()].count_ones() as i32;
        score += count * piece_value(piece);
    }

    score
}

/// Calcule le différentiel matériel du point de vue du joueur qui a le trait.
/// Score positif → avantage pour le joueur actif.
pub fn material_eval(board: &Board) -> i32 {
    let white_score = material_score(board, Color::White);
    let black_score = material_score(board, Color::Black);
    let diff = white_score - black_score;

    // Retourner du point de vue du joueur actif
    if board.side_to_move == Color::White { diff } else { -diff }
}

/// Calcule le matériel total présent sur le plateau (pour la détection de phase).
/// Retourne la somme des valeurs de toutes les pièces (sans les rois et pions).
pub fn non_pawn_material(board: &Board) -> i32 {
    let mut total = 0i32;
    for color in [Color::White, Color::Black] {
        for piece in [Piece::Knight, Piece::Bishop, Piece::Rook, Piece::Queen] {
            let count = board.pieces[color.index()][piece.index()].count_ones() as i32;
            total += count * piece_value(piece);
        }
    }
    total
}

/// Bonus pour la possession des deux fous (paire de fous).
/// La paire de fous est un avantage stratégique bien établi : ensemble, les deux
/// fous couvrent toutes les couleurs et dominent les positions ouvertes.
/// Calibré par Texel Tuning v3 (était 30) — voir PIECE_VALUE ci-dessus.
const BISHOP_PAIR_BONUS: i32 = 50;

/// Retourne le bonus de paire de fous pour une couleur donnée.
/// Bonus accordé si le camp possède au moins 2 fous.
pub fn bishop_pair_score(board: &Board, color: Color) -> i32 {
    let bishop_count = board.pieces[color.index()][Piece::Bishop.index()].count_ones();
    if bishop_count >= 2 { BISHOP_PAIR_BONUS } else { 0 }
}

/// Calcule le différentiel de paire de fous du point de vue du joueur actif.
pub fn bishop_pair_eval(board: &Board) -> i32 {
    let white_score = bishop_pair_score(board, Color::White);
    let black_score = bishop_pair_score(board, Color::Black);
    let diff        = white_score - black_score;

    if board.side_to_move == Color::White { diff } else { -diff }
}

/// Retourne true si le joueur de la couleur donnée a suffisamment de matériel
/// pour mater (exclut les cas de matériel insuffisant).
pub fn has_mating_material(board: &Board, color: Color) -> bool {
    // Roi seul → impossible de mater
    let non_king = board.occupancy[color.index()]
        & !board.pieces[color.index()][Piece::King.index()];

    if non_king == 0 {
        return false;
    }

    // Roi + cavalier seul ou roi + fou seul → insuffisant
    let count = non_king.count_ones();
    if count == 1 {
        // Une seule pièce autre que le roi
        if board.pieces[color.index()][Piece::Knight.index()] != 0 { return false; }
        if board.pieces[color.index()][Piece::Bishop.index()] != 0 { return false; }
    }

    true
}
