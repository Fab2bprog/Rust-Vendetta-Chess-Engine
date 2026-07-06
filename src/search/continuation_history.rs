// =============================================================================
// Vendetta Chess Motor — src/search/continuation_history.rs
//
// Rôle : Gestion de la "Continuation History" (historique de continuation,
//        parfois appelée "history à 2 coups").
//
// Différence avec le Countermove Heuristic (countermove.rs) :
//   - Countermove  : "EN RÉPONSE à TEL coup adverse précis, TEL coup est LE
//                     MEILLEUR observé" — un seul slot par contexte, écrasé
//                     à chaque nouvelle réfutation trouvée.
//   - Continuation : "EN RÉPONSE à TEL coup adverse précis, TEL coup a été
//                     EN MOYENNE bon ou mauvais" — un score CUMULATIF par
//                     contexte, exactement comme l'History Heuristic
//                     classique mais avec un contexte supplémentaire (le
//                     coup adverse précédent).
//
//   Les deux sont complémentaires : countermove capture "LA" meilleure
//   réponse connue, continuation history capture une tendance statistique
//   plus fine. Utilisées ensemble, comme dans ce moteur.
//
// Indexation : table[pièce_adverse][case_adverse][pièce][case_arrivée].
//   6 × 64 × 6 × 64 = 147 456 entrées i32 ≈ 576 Kio.
//
// Choix de stockage — Vec<i32> à plat plutôt que [[[[i32; 64]; 6]; 64]; 6] :
//   Les autres tables de ce module (killers, history, countermove) sont
//   minuscules (quelques Kio) et stockées comme des tableaux fixes sans
//   risque. Celle-ci est ~1500× plus grosse. Un tableau imbriqué de cette
//   taille construit PAR VALEUR (ex: dans une fonction new() qui le retourne)
//   pourrait transiter par une grosse allocation temporaire sur la pile avant
//   d'être déplacé — risque réel avec de nombreux threads Lazy SMP qui créent
//   chacun leur propre instance. Un Vec<i32> est alloué directement sur le
//   tas dès sa création (vec![0; N]) : aucun risque de dépassement de pile,
//   quelle que soit la taille.
// =============================================================================

use crate::board::state::Board;
use crate::utils::types::{Move, Piece};

const PIECES:  usize = 6;
const SQUARES: usize = 64;
const TABLE_SIZE: usize = PIECES * SQUARES * PIECES * SQUARES;

/// Table de continuation history : un score cumulatif par
/// (pièce adverse, case adverse, pièce, case d'arrivée).
pub struct ContinuationHistoryTable {
    table: Vec<i32>,
}

impl ContinuationHistoryTable {
    /// Crée une nouvelle table de continuation history (vide, allouée sur le tas).
    pub fn new() -> ContinuationHistoryTable {
        ContinuationHistoryTable {
            table: vec![0i32; TABLE_SIZE],
        }
    }

    #[inline]
    fn index(prev_piece: Piece, prev_to: u8, piece: Piece, to: u8) -> usize {
        ((prev_piece.index() * SQUARES + prev_to as usize) * PIECES + piece.index())
            * SQUARES
            + to as usize
    }

    /// Augmente le score du coup qui a causé une coupure bêta, dans le
    /// contexte du dernier coup adverse (prev_piece, prev_to).
    /// Le bonus est proportionnel à depth² — même convention que
    /// HistoryTable::update_good().
    ///
    /// `board` doit être dans l'état D'AVANT le coup `mv` (c'est-à-dire après
    /// un unmake_move() — exactement comme HistoryTable::update_good(), pour
    /// pouvoir lire la pièce encore présente sur mv.from).
    pub fn update_good(
        &mut self,
        prev_piece: Piece,
        prev_to:    u8,
        board:      &Board,
        mv:         Move,
        depth:      i32,
    ) {
        if mv.flags.is_capture() { return; }

        if let Some((piece, _)) = board.piece_at(mv.from) {
            let idx   = Self::index(prev_piece, prev_to, piece, mv.to);
            let bonus = depth * depth;
            self.table[idx] = (self.table[idx] + bonus).min(10_000);
        }
    }

    /// Réduit légèrement le score d'un coup qui n'a pas causé de coupure,
    /// dans le même contexte. Même convention que HistoryTable::update_bad().
    pub fn update_bad(
        &mut self,
        prev_piece: Piece,
        prev_to:    u8,
        board:      &Board,
        mv:         Move,
        depth:      i32,
    ) {
        if mv.flags.is_capture() { return; }

        if let Some((piece, _)) = board.piece_at(mv.from) {
            let idx = Self::index(prev_piece, prev_to, piece, mv.to);
            self.table[idx] = (self.table[idx] - depth).max(-10_000);
        }
    }

    /// Retourne le score de continuation history pour (pièce, case
    /// d'arrivée) dans le contexte (prev_piece, prev_to).
    #[inline]
    pub fn get(&self, prev_piece: Piece, prev_to: u8, piece: Piece, to: u8) -> i32 {
        self.table[Self::index(prev_piece, prev_to, piece, to)]
    }

    /// Remet à zéro toute la table (entre deux parties).
    pub fn clear(&mut self) {
        self.table.iter_mut().for_each(|v| *v = 0);
    }

    /// Divise tous les scores par 2 (vieillissement entre les recherches) —
    /// même politique que HistoryTable::age(), pas un clear() complet comme
    /// les killers/countermove : un score cumulatif garde un intérêt à se
    /// transmettre, atténué, d'une recherche à l'autre.
    pub fn age(&mut self) {
        self.table.iter_mut().for_each(|v| *v /= 2);
    }
}

impl Default for ContinuationHistoryTable {
    fn default() -> Self {
        ContinuationHistoryTable::new()
    }
}
