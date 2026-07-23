// =============================================================================
// Vendetta Chess Motor — src/moves/rook.rs
//
// Rôle : Génère tous les pseudo-coups légaux des tours pour une couleur donnée.
//        Utilise la fonction rook_attacks() qui calcule les cases attaquées
//        en tenant compte des pièces bloquantes (approche classique par boucle).
//
// Contenu :
//   - Coups silencieux (déplacement vers une case vide)
//   - Captures (déplacement vers une case occupée par l'ennemi)
//
// La tour se déplace en ligne droite (horizontal/vertical) sur autant de cases
// que possible, s'arrêtant à la première pièce rencontrée.
// =============================================================================

use crate::utils::types::{Color, Piece, Move};
use crate::board::state::Board;
use crate::board::bitboard::{pop_lsb, rook_attacks};

/// Génère tous les pseudo-coups des tours de la couleur `color`.
/// Les coups sont ajoutés au vecteur `moves`.
pub fn generate_rook_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let mut rooks  = board.pieces[color.index()][Piece::Rook.index()];
    let own_pieces = board.occupancy[color.index()];
    let occupied   = board.all_pieces;

    while rooks != 0 {
        let from    = pop_lsb(&mut rooks);
        let attacks = rook_attacks(from, occupied) & !own_pieces;

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
