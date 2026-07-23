// =============================================================================
// Vendetta Chess Motor — src/moves/queen.rs
//
// Rôle : Génère tous les pseudo-coups légaux des dames pour une couleur donnée.
//        La dame combine les mouvements du fou et de la tour : elle peut se
//        déplacer en ligne droite ET en diagonale.
//
// Contenu :
//   - Coups silencieux
//   - Captures
//
// Implémentation : on réutilise queen_attacks() qui combine rook_attacks()
// et bishop_attacks(). Simple, correct, et maintenable.
// =============================================================================

use crate::utils::types::{Color, Piece, Move};
use crate::board::state::Board;
use crate::board::bitboard::{pop_lsb, queen_attacks};

/// Génère tous les pseudo-coups des dames de la couleur `color`.
/// Les coups sont ajoutés au vecteur `moves`.
pub fn generate_queen_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let mut queens = board.pieces[color.index()][Piece::Queen.index()];
    let own_pieces = board.occupancy[color.index()];
    let occupied   = board.all_pieces;

    while queens != 0 {
        let from    = pop_lsb(&mut queens);
        // La dame attaque comme la tour + le fou
        let attacks = queen_attacks(from, occupied) & !own_pieces;

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
