// =============================================================================
// Vendetta Chess Motor — src/utils/mod.rs
//
// Rôle : Module utilitaires. Point d'entrée pour tous les types communs et
//        fonctions partagées utilisées dans l'ensemble du projet.
//
// Contenu :
//   - Réexporte le module types (Color, Piece, Move, MoveFlags, constantes...)
// =============================================================================

pub mod types;

// Réexportation des types les plus utilisés pour simplifier les imports.
pub use types::{
    Color, Piece, Move, MoveFlags,
    SCORE_INF, SCORE_MATE, SCORE_DRAW,
    NUM_PIECES, NUM_COLORS, NUM_SQUARES,
    file_of, rank_of, make_square, square_from_str, square_to_str,
};
