// =============================================================================
// Vendetta Chess Motor — src/moves/knight.rs
//
// Rôle : Génère tous les pseudo-coups légaux des cavaliers pour une couleur.
//        Utilise la table d'attaque précalculée (knight_attacks) pour une
//        génération rapide et simple.
//
// Contenu :
//   - Coups silencieux (déplacement vers une case vide)
//   - Captures (déplacement vers une case occupée par l'ennemi)
//
// Note : Le cavalier est la seule pièce pouvant sauter par-dessus d'autres
//        pièces. Sa génération est donc particulièrement simple.
// =============================================================================

use crate::utils::types::{Color, Piece, Move};
use crate::board::state::Board;
use crate::board::bitboard::{pop_lsb, knight_attacks};

/// Génère tous les pseudo-coups des cavaliers de la couleur `color`.
/// Les coups sont ajoutés au vecteur `moves`.
pub fn generate_knight_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let mut knights = board.pieces[color.index()][Piece::Knight.index()];
    let own_pieces  = board.occupancy[color.index()];

    // Pour chaque cavalier, on génère ses attaques depuis la table précalculée.
    while knights != 0 {
        let from    = pop_lsb(&mut knights);
        // Cases attaquées par le cavalier, en excluant nos propres pièces.
        let attacks = knight_attacks(from) & !own_pieces;

        let mut bb = attacks;
        while bb != 0 {
            let to = pop_lsb(&mut bb);
            // Si une pièce ennemie est sur la case cible, c'est une capture.
            if board.all_pieces & (1u64 << to) != 0 {
                moves.push(Move::capture(from, to));
            } else {
                moves.push(Move::quiet(from, to));
            }
        }
    }
}
