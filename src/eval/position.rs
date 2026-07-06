// =============================================================================
// Vendetta Chess Motor — src/eval/position.rs
//
// Rôle : Tables de positions (Piece-Square Tables).
//        Chaque table donne un bonus ou malus positionnel à une pièce selon
//        la case sur laquelle elle se trouve. Ces tables encodent la sagesse
//        positionnel classique des échecs.
//
// Contenu :
//   - Tables pour chaque type de pièce (ouverture/milieu de partie)
//   - Tables pour la finale (le roi doit aller au centre)
//   - Fonction d'évaluation positionnelle
//
// Convention :
//   - Les tables sont définies du point de vue des Blancs (rang 1 en bas)
//   - Pour les Noirs, on lit la table à l'envers (rang miroir)
//   - Les valeurs sont en centipions
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::pop_lsb;
use crate::eval::tables::{
    PAWN_TABLE, KNIGHT_TABLE, BISHOP_TABLE, ROOK_TABLE, QUEEN_TABLE,
    KING_MIDDLEGAME_TABLE, KING_ENDGAME_TABLE, mirror_square,
};

/// Retourne le bonus positionnel d'une pièce sur une case selon la phase.
/// Délègue aux tables centralisées dans eval::tables.
pub fn piece_square_value(piece: Piece, color: Color, sq: u8, is_endgame: bool) -> i32 {
    let idx = if color == Color::White {
        sq as usize
    } else {
        mirror_square(sq) as usize
    };

    match piece {
        Piece::Pawn   => PAWN_TABLE[idx],
        Piece::Knight => KNIGHT_TABLE[idx],
        Piece::Bishop => BISHOP_TABLE[idx],
        Piece::Rook   => ROOK_TABLE[idx],
        Piece::Queen  => QUEEN_TABLE[idx],
        Piece::King   => {
            if is_endgame { KING_ENDGAME_TABLE[idx] } else { KING_MIDDLEGAME_TABLE[idx] }
        }
    }
}

/// Calcule le score positionnel total pour une couleur donnée.
pub fn positional_score(board: &Board, color: Color, is_endgame: bool) -> i32 {
    let mut score = 0i32;

    for piece in [Piece::Pawn, Piece::Knight, Piece::Bishop,
                  Piece::Rook, Piece::Queen, Piece::King] {
        let mut bb = board.pieces[color.index()][piece.index()];
        while bb != 0 {
            let sq = pop_lsb(&mut bb);
            score += piece_square_value(piece, color, sq, is_endgame);
        }
    }

    score
}

/// Calcule le différentiel positionnel du point de vue du joueur actif.
pub fn positional_eval(board: &Board, is_endgame: bool) -> i32 {
    let white_score = positional_score(board, Color::White, is_endgame);
    let black_score = positional_score(board, Color::Black, is_endgame);
    let diff = white_score - black_score;

    if board.side_to_move == Color::White { diff } else { -diff }
}
