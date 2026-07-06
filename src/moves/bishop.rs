// =============================================================================
// Vendetta Chess Motor — src/moves/bishop.rs
//
// Rôle : Génère tous les pseudo-coups légaux des fous pour une couleur donnée.
//        Utilise la fonction bishop_attacks() qui calcule les cases attaquées
//        en tenant compte des pièces bloquantes (approche classique par boucle).
//
// Contenu :
//   - Coups silencieux (déplacement vers une case vide)
//   - Captures (déplacement vers une case occupée par l'ennemi)
//
// Le fou se déplace en diagonale sur autant de cases que possible,
// s'arrêtant à la première pièce rencontrée (qu'il peut capturer si ennemie).
// =============================================================================

use crate::utils::types::{Color, Piece, Move};
use crate::board::state::Board;
use crate::board::bitboard::{pop_lsb, bishop_attacks};

/// Génère tous les pseudo-coups des fous de la couleur `color`.
/// Les coups sont ajoutés au vecteur `moves`.
pub fn generate_bishop_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let mut bishops = board.pieces[color.index()][Piece::Bishop.index()];
    let own_pieces  = board.occupancy[color.index()];
    let occupied    = board.all_pieces;

    while bishops != 0 {
        let from    = pop_lsb(&mut bishops);
        // Cases attaquées par le fou depuis cette case, en excluant nos propres pièces.
        let attacks = bishop_attacks(from, occupied) & !own_pieces;

        let mut bb = attacks;
        while bb != 0 {
            let to = pop_lsb(&mut bb);
            if board.all_pieces & (1u64 << to) != 0 {
                moves.push(Move::capture(from, to));
            } else {
                moves.push(Move::quiet(from, to));
            }
        }
    }
}
