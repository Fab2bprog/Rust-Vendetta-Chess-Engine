// =============================================================================
// Vendetta Chess Motor — src/game/rules.rs
//
// Rôle : Vérification des règles de fin de partie qui ne sont pas gérées
//        directement par la génération des coups.
//
// Contenu :
//   - Règle des 50 coups (nulle si 100 demi-coups sans capture ni mouvement de pion)
//   - Répétition de position (nulle si 3 répétitions)
//   - Matériel insuffisant (nulle si impossible de mater)
//   - GameResult : résultat final d'une partie
//
// Note : La règle des 50 coups utilise le halfmove_clock du plateau.
//        La répétition utilise l'historique des positions (PositionHistory).
// =============================================================================

use crate::board::state::Board;
use crate::eval::is_insufficient_material;
use super::history::PositionHistory;

/// Résultat possible d'une partie.
#[derive(Debug, Clone, PartialEq)]
pub enum GameResult {
    /// La partie continue.
    Ongoing,
    /// Nulle par règle des 50 coups.
    DrawFiftyMoves,
    /// Nulle par répétition de position (3 fois).
    DrawRepetition,
    /// Nulle par matériel insuffisant.
    DrawInsufficientMaterial,
    /// Pat.
    DrawStalemate,
    /// Échec et mat : la couleur donnée a perdu.
    Checkmate,
}

/// Vérifie si la partie est nulle par la règle des 50 coups.
/// La règle dit : après 50 coups complets (100 demi-coups) sans pion joué
/// ni capture, la partie est nulle.
pub fn is_fifty_move_draw(board: &Board) -> bool {
    board.halfmove_clock >= 100
}

/// Vérifie si la partie est nulle par répétition de position.
/// Nulle si la position actuelle a déjà été rencontrée 2 fois (3e occurrence).
pub fn is_repetition_draw(board: &Board, history: &PositionHistory) -> bool {
    history.is_threefold_repetition(board.hash)
}

/// Vérifie toutes les conditions de nulle (hors pat et mat qui nécessitent
/// la génération des coups, gérée dans moves/mod.rs).
pub fn check_draw(board: &Board, history: &PositionHistory) -> Option<GameResult> {
    if is_fifty_move_draw(board) {
        return Some(GameResult::DrawFiftyMoves);
    }

    if is_repetition_draw(board, history) {
        return Some(GameResult::DrawRepetition);
    }

    if is_insufficient_material(board) {
        return Some(GameResult::DrawInsufficientMaterial);
    }

    None
}
