// =============================================================================
// Vendetta Chess Motor — src/search/history.rs
//
// Rôle : Gestion de l'heuristique d'historique (History Heuristic).
//        Chaque coup qui a causé une coupure bêta voit son score augmenter
//        dans une table [pièce][case_destination]. Ce score est utilisé pour
//        trier les coups silencieux (plus un coup a été bon dans le passé,
//        plus tôt on le teste).
//
// Contenu :
//   - HistoryTable : table 2D [pièce 0-5][case 0-63]
//   - Mise à jour du score lors d'une coupure bêta
//   - Réduction du score pour les coups qui n'ont pas causé de coupure
//     (aging — pour éviter que les vieux scores dominent)
//
// Note : L'heuristique d'historique est complémentaire aux killer moves.
//   Killers : "ce coup spécifique à ce niveau a été bon"
//   History : "ce type de coup a globalement souvent été bon"
// =============================================================================

use crate::utils::types::{Move, Piece};
use crate::board::state::Board;

/// Table d'historique : history[type_pièce][case_destination].
/// Les valeurs positives indiquent que ce coup a souvent causé des coupures.
pub struct HistoryTable {
    table: [[i32; 64]; 6],
}

impl HistoryTable {
    /// Crée une nouvelle table d'historique vide.
    pub fn new() -> HistoryTable {
        HistoryTable { table: [[0; 64]; 6] }
    }

    /// Augmente le score du coup qui a causé une coupure bêta.
    /// Le bonus est proportionnel à la profondeur (depth^2).
    pub fn update_good(&mut self, board: &Board, mv: Move, depth: i32) {
        if mv.flags.is_capture() { return; } // On ne met à jour que les coups silencieux

        if let Some((piece, _)) = board.piece_at(mv.from) {
            let bonus = depth * depth;
            self.table[piece.index()][mv.to as usize] += bonus;
            // Limiter pour éviter les débordements
            if self.table[piece.index()][mv.to as usize] > 10_000 {
                self.table[piece.index()][mv.to as usize] = 10_000;
            }
        }
    }

    /// Réduit légèrement le score des coups qui n'ont pas causé de coupure.
    /// Évite que les vieux bons scores dominent indéfiniment.
    pub fn update_bad(&mut self, board: &Board, mv: Move, depth: i32) {
        if mv.flags.is_capture() { return; }

        if let Some((piece, _)) = board.piece_at(mv.from) {
            let penalty = depth;
            self.table[piece.index()][mv.to as usize] -= penalty;
            if self.table[piece.index()][mv.to as usize] < -10_000 {
                self.table[piece.index()][mv.to as usize] = -10_000;
            }
        }
    }

    /// Retourne le score d'historique d'un coup (pour l'ordonnancement).
    pub fn get(&self, piece: Piece, to: u8) -> i32 {
        self.table[piece.index()][to as usize]
    }

    /// Remet à zéro toute la table (entre deux parties).
    pub fn clear(&mut self) {
        self.table = [[0; 64]; 6];
    }

    /// Divise tous les scores par 2 (vieillissement entre les itérations).
    pub fn age(&mut self) {
        for row in &mut self.table {
            for val in row.iter_mut() {
                *val /= 2;
            }
        }
    }
}

impl Default for HistoryTable {
    fn default() -> Self {
        HistoryTable::new()
    }
}
