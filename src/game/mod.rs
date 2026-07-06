// =============================================================================
// Vendetta Chess Motor — src/game/mod.rs
//
// Rôle : Gestion de la partie en cours. Coordonne l'état du plateau, l'historique
//        des positions, et la vérification des conditions de fin de partie.
//
// Contenu :
//   - Game : structure principale de la partie
//   - Jouer/annuler des coups avec suivi de l'historique
//   - Détection des conditions de fin de partie
// =============================================================================

pub mod history;
pub mod rules;

use crate::board::state::Board;
use crate::utils::types::Move;
use history::PositionHistory;
use rules::{GameResult, check_draw};
use crate::moves::{is_checkmate, is_stalemate};

/// Représentation d'une partie d'échecs en cours.
pub struct Game {
    /// L'état actuel du plateau.
    pub board: Board,
    /// Historique des hashs de position pour la détection de répétition.
    pub position_history: PositionHistory,
}

impl Game {
    /// Crée une nouvelle partie à la position initiale.
    pub fn new() -> Game {
        let board = Board::start_position();
        let hash  = board.hash;
        let mut history = PositionHistory::new();
        history.push(hash);

        Game {
            board,
            position_history: history,
        }
    }

    /// Crée une partie depuis une FEN.
    pub fn from_fen(fen: &str) -> Result<Game, String> {
        let board = Board::from_fen(fen)?;
        let hash  = board.hash;
        let mut history = PositionHistory::new();
        history.push(hash);

        Ok(Game {
            board,
            position_history: history,
        })
    }

    /// Joue un coup et met à jour l'historique.
    pub fn make_move(&mut self, mv: Move) {
        self.board.make_move(mv);
        self.position_history.push(self.board.hash);
    }

    /// Annule le dernier coup joué.
    pub fn unmake_move(&mut self, mv: Move) {
        self.position_history.pop();
        self.board.unmake_move(mv);
    }

    /// Vérifie le résultat actuel de la partie.
    pub fn result(&mut self) -> GameResult {
        // Vérifier d'abord les conditions de nulle simples (sans génération de coups)
        if let Some(result) = check_draw(&self.board, &self.position_history) {
            return result;
        }

        // Vérifier mat et pat (nécessite la génération des coups)
        if is_checkmate(&mut self.board) {
            return GameResult::Checkmate;
        }

        if is_stalemate(&mut self.board) {
            return GameResult::DrawStalemate;
        }

        GameResult::Ongoing
    }

    /// Remet à zéro pour une nouvelle partie.
    pub fn reset(&mut self) {
        self.board = Board::start_position();
        self.position_history.clear();
        self.position_history.push(self.board.hash);
    }
}

impl Default for Game {
    fn default() -> Self {
        Game::new()
    }
}
