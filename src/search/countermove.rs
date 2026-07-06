// =============================================================================
// Vendetta Chess Motor — src/search/countermove.rs
//
// Rôle : Gestion de l'heuristique "Countermove" (coup de réfutation).
//        Idée : si un certain type de coup adverse (ex : "le cavalier va en
//        e5") a déjà été efficacement réfuté par un coup précis ailleurs dans
//        l'arbre, ce même coup est probablement encore une bonne réponse ici
//        — même position de pièces différente, même "motif" de réfutation.
//
// Différence avec les killer moves et l'history heuristic :
//   - Killers  : "CE coup précis a été bon à CE niveau de profondeur"
//   - History  : "CE TYPE de coup (pièce + destination) a globalement
//                 souvent été bon, toutes profondeurs confondues"
//   - Countermove : "EN RÉPONSE à TEL coup adverse précis (pièce + case
//                 d'arrivée), TEL coup a déjà été une bonne réfutation"
//
// Indexation : countermoves[pièce_adverse][case_arrivée_adverse] → coup.
//   La pièce et la case d'arrivée du DERNIER COUP JOUÉ (celui qui a mené au
//   nœud courant) sont dérivées via board.piece_at(prev_move.to) — déjà
//   appliqué sur le plateau au moment où ce nœud est atteint, donc lisible
//   directement sans paramètre supplémentaire à propager à part le Move lui-même.
//
// Un seul countermove par clé (pas 2 comme les killers) : la littérature
// (Stockfish historique, Crafty) montre qu'un seul suffit largement, le gain
// d'un deuxième slot étant marginal comparé à la complexité ajoutée.
// =============================================================================

use crate::utils::types::{Move, Piece};

/// Table countermove : un coup de réfutation par (pièce, case d'arrivée) du
/// dernier coup adverse joué.
pub struct CountermoveTable {
    /// table[pièce_index 0-5][case_arrivée 0-63] → coup de réfutation.
    table: [[Move; 64]; 6],
}

impl CountermoveTable {
    /// Crée une nouvelle table countermove (vide).
    pub fn new() -> CountermoveTable {
        CountermoveTable {
            table: [[Move::NULL; 64]; 6],
        }
    }

    /// Enregistre `mv` comme réfutation du dernier coup joué, identifié par
    /// (`prev_piece`, `prev_to`).
    ///
    /// Ne stocke que des coups silencieux — cohérent avec killers/history :
    /// les captures sont déjà bien ordonnées par SEE, inutile de les dupliquer
    /// ici (et un countermove de capture serait souvent illégal ou hors-sujet
    /// dans une autre position).
    pub fn store(&mut self, prev_piece: Piece, prev_to: u8, mv: Move) {
        if mv.flags.is_capture() { return; }
        self.table[prev_piece.index()][prev_to as usize] = mv;
    }

    /// Retourne le countermove enregistré pour (`prev_piece`, `prev_to`).
    /// `Move::NULL` si aucun n'est enregistré.
    pub fn get(&self, prev_piece: Piece, prev_to: u8) -> Move {
        self.table[prev_piece.index()][prev_to as usize]
    }

    /// Remet à zéro toute la table (entre deux parties, ou entre deux "go"
    /// comme les killers — un countermove pertinent dans une recherche n'a
    /// aucune raison de l'être dans la position suivante après que l'adversaire
    /// a réellement joué un coup).
    pub fn clear(&mut self) {
        self.table = [[Move::NULL; 64]; 6];
    }
}

impl Default for CountermoveTable {
    fn default() -> Self {
        CountermoveTable::new()
    }
}
