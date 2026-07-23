// =============================================================================
// Vendetta Chess Motor — src/eval/tables.rs
//
// Rôle : Tables de positions (PST) et fonctions de lookup pures.
//        Ce fichier N'IMPORTE JAMAIS board::state — il doit rester accessible
//        depuis board::state sans créer de dépendance circulaire.
//
// Contenu :
//   - 7 tables PST (pion, cavalier, fou, tour, dame, roi×2)
//   - mirror_square() — symétrie verticale pour les Noirs
//   - piece_square_values() — retourne (mg, eg) pour une pièce sur une case
//
// Utilisation :
//   - board::state   → piece_square_values() pour la mise à jour incrémentale
//   - eval::position → importe les tables pour positional_eval() (debug/test)
// =============================================================================

use crate::utils::types::{Color, Piece};

// =============================================================================
// Tables de positions — du point de vue des Blancs
// Index 0 = a1, index 63 = h8 (rang × 8 + colonne).
// Les tables sont lues rang par rang de bas en haut (rang 1 → rang 8).
// =============================================================================

/// Table positionnelle des pions.
pub const PAWN_TABLE: [i32; 64] = [
     0,  0,  0,  0,  0,  0,  0,  0,
    50, 50, 50, 50, 50, 50, 50, 50,
    10, 10, 20, 30, 30, 20, 10, 10,
     5,  5, 10, 25, 25, 10,  5,  5,
     0,  0,  0, 20, 20,  0,  0,  0,
     5, -5,-10,  0,  0,-10, -5,  5,
     5, 10, 10,-20,-20, 10, 10,  5,
     0,  0,  0,  0,  0,  0,  0,  0,
];

/// Table positionnelle des cavaliers.
pub const KNIGHT_TABLE: [i32; 64] = [
    -50,-40,-30,-30,-30,-30,-40,-50,
    -40,-20,  0,  0,  0,  0,-20,-40,
    -30,  0, 10, 15, 15, 10,  0,-30,
    -30,  5, 15, 20, 20, 15,  5,-30,
    -30,  0, 15, 20, 20, 15,  0,-30,
    -30,  5, 10, 15, 15, 10,  5,-30,
    -40,-20,  0,  5,  5,  0,-20,-40,
    -50,-40,-30,-30,-30,-30,-40,-50,
];

/// Table positionnelle des fous.
pub const BISHOP_TABLE: [i32; 64] = [
    -20,-10,-10,-10,-10,-10,-10,-20,
    -10,  0,  0,  0,  0,  0,  0,-10,
    -10,  0,  5, 10, 10,  5,  0,-10,
    -10,  5,  5, 10, 10,  5,  5,-10,
    -10,  0, 10, 10, 10, 10,  0,-10,
    -10, 10, 10, 10, 10, 10, 10,-10,
    -10,  5,  0,  0,  0,  0,  5,-10,
    -20,-10,-10,-10,-10,-10,-10,-20,
];

/// Table positionnelle des tours.
pub const ROOK_TABLE: [i32; 64] = [
     0,  0,  0,  0,  0,  0,  0,  0,
     5, 10, 10, 10, 10, 10, 10,  5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
     0,  0,  0,  5,  5,  0,  0,  0,
];

/// Table positionnelle des dames.
pub const QUEEN_TABLE: [i32; 64] = [
    -20,-10,-10, -5, -5,-10,-10,-20,
    -10,  0,  0,  0,  0,  0,  0,-10,
    -10,  0,  5,  5,  5,  5,  0,-10,
     -5,  0,  5,  5,  5,  5,  0, -5,
      0,  0,  5,  5,  5,  5,  0, -5,
    -10,  5,  5,  5,  5,  5,  0,-10,
    -10,  0,  5,  0,  0,  0,  0,-10,
    -20,-10,-10, -5, -5,-10,-10,-20,
];

/// Table positionnelle du roi en milieu de partie.
/// Le roi doit rester protégé (de préférence après un roque).
pub const KING_MIDDLEGAME_TABLE: [i32; 64] = [
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -20,-30,-30,-40,-40,-30,-30,-20,
    -10,-20,-20,-20,-20,-20,-20,-10,
     20, 20,  0,  0,  0,  0, 20, 20,
     20, 30, 10,  0,  0, 10, 30, 20,
];

/// Table positionnelle du roi en finale.
/// En finale, le roi doit centraliser.
pub const KING_ENDGAME_TABLE: [i32; 64] = [
    -50,-40,-30,-20,-20,-30,-40,-50,
    -30,-20,-10,  0,  0,-10,-20,-30,
    -30,-10, 20, 30, 30, 20,-10,-30,
    -30,-10, 30, 40, 40, 30,-10,-30,
    -30,-10, 30, 40, 40, 30,-10,-30,
    -30,-10, 20, 30, 30, 20,-10,-30,
    -30,-30,  0,  0,  0,  0,-30,-30,
    -50,-30,-30,-30,-30,-30,-30,-50,
];

// =============================================================================
// Fonctions de lookup
// =============================================================================

/// Retourne l'index miroir d'une case (pour les Noirs).
/// Les Blancs voient le rang 1 en bas (index 0–7), les Noirs en haut.
/// Exemple : a1 (0) ↔ a8 (56).
#[inline]
pub fn mirror_square(sq: u8) -> u8 {
    (7 - sq / 8) * 8 + sq % 8
}

/// Retourne les contributions PST (midgame, endgame) d'une pièce sur une case,
/// du point de vue de sa couleur.
///
/// Pour toutes les pièces sauf le roi, mg == eg (même table).
/// Pour le roi : mg = KING_MIDDLEGAME_TABLE, eg = KING_ENDGAME_TABLE.
///
/// Le signe ±1 (Blanc = +1, Noir = −1) est appliqué par l'appelant
/// (`place_piece` / `remove_piece`), pas ici — séparation claire des responsabilités.
#[inline]
pub fn piece_square_values(piece: Piece, color: Color, sq: u8) -> (i32, i32) {
    // Les tables sont orientées Blanc (rang 1 en bas).
    // Pour les Noirs, on lit la case miroir.
    let idx = if color == Color::White {
        sq as usize
    } else {
        mirror_square(sq) as usize
    };

    let mg = match piece {
        Piece::Pawn   => PAWN_TABLE[idx],
        Piece::Knight => KNIGHT_TABLE[idx],
        Piece::Bishop => BISHOP_TABLE[idx],
        Piece::Rook   => ROOK_TABLE[idx],
        Piece::Queen  => QUEEN_TABLE[idx],
        Piece::King   => KING_MIDDLEGAME_TABLE[idx],
    };

    // Le roi est la seule pièce avec deux tables distinctes.
    let eg = if piece == Piece::King {
        KING_ENDGAME_TABLE[idx]
    } else {
        mg // Toutes les autres pièces : même table MG et EG.
    };

    (mg, eg)
}
