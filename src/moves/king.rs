// =============================================================================
// Vendetta Chess Motor — src/moves/king.rs
//
// Rôle : Génère tous les pseudo-coups légaux du roi pour une couleur donnée.
//        Gère les déplacements normaux ET les roques (petit et grand).
//
// Contenu :
//   - Déplacements normaux (1 case dans toutes les directions)
//   - Petit roque (côté roi)
//   - Grand roque (côté dame)
//
// Règles du roque :
//   - Le roi ne doit pas être en échec avant le roque
//   - Les cases traversées par le roi ne doivent pas être attaquées
//   - Les cases entre le roi et la tour doivent être vides
//   - Le droit de roque doit être disponible (non perdu)
//
// IMPORTANT : On ne vérifie PAS ici si le roi est en échec après le roque.
//   Ce filtrage est fait dans generate_legal_moves() de mod.rs.
//   En revanche, on vérifie bien que le roi ne TRAVERSE PAS une case en échec.
// =============================================================================

use crate::utils::types::{Color, Move};
use crate::board::state::Board;
use crate::board::bitboard::{pop_lsb, king_attacks, get_bit};

/// Génère tous les pseudo-coups du roi de la couleur `color`.
/// Les coups sont ajoutés au vecteur `moves`.
pub fn generate_king_moves(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let king_sq    = board.king_square(color);
    let own_pieces = board.occupancy[color.index()];

    // --- Déplacements normaux ---
    let attacks = king_attacks(king_sq) & !own_pieces;

    let mut bb = attacks;
    while bb != 0 {
        let to = pop_lsb(&mut bb);
        if board.all_pieces & (1u64 << to) != 0 {
            moves.push(Move::capture(king_sq, to));
        } else {
            moves.push(Move::quiet(king_sq, to));
        }
    }

    // --- Roque ---
    // On génère les roques ici, mais la vérification finale (roi en échec,
    // cases traversées attaquées) se fait dans generate_legal_moves().
    generate_castling_moves(board, color, king_sq, moves);
}

/// Génère les coups de roque disponibles pour la couleur `color`.
/// Vérifie que les cases intermédiaires sont vides et que les droits de roque
/// sont disponibles. La vérification des cases attaquées est dans mod.rs.
fn generate_castling_moves(board: &Board, color: Color, king_sq: u8, moves: &mut crate::moves::MoveList) {
    match color {
        Color::White => {
            // Petit roque blanc : e1 (4) → g1 (6), tour en h1 (7)
            // Les cases f1 (5) et g1 (6) doivent être vides
            if board.castling.can_castle_kingside(Color::White)
                && !get_bit(board.all_pieces, 5)
                && !get_bit(board.all_pieces, 6)
            {
                moves.push(Move::castle_kingside(king_sq, 6));
            }

            // Grand roque blanc : e1 (4) → c1 (2), tour en a1 (0)
            // Les cases d1 (3), c1 (2) et b1 (1) doivent être vides
            if board.castling.can_castle_queenside(Color::White)
                && !get_bit(board.all_pieces, 3)
                && !get_bit(board.all_pieces, 2)
                && !get_bit(board.all_pieces, 1)
            {
                moves.push(Move::castle_queenside(king_sq, 2));
            }
        }

        Color::Black => {
            // Petit roque noir : e8 (60) → g8 (62), tour en h8 (63)
            // Les cases f8 (61) et g8 (62) doivent être vides
            if board.castling.can_castle_kingside(Color::Black)
                && !get_bit(board.all_pieces, 61)
                && !get_bit(board.all_pieces, 62)
            {
                moves.push(Move::castle_kingside(king_sq, 62));
            }

            // Grand roque noir : e8 (60) → c8 (58), tour en a8 (56)
            // Les cases d8 (59), c8 (58) et b8 (57) doivent être vides
            if board.castling.can_castle_queenside(Color::Black)
                && !get_bit(board.all_pieces, 59)
                && !get_bit(board.all_pieces, 58)
                && !get_bit(board.all_pieces, 57)
            {
                moves.push(Move::castle_queenside(king_sq, 58));
            }
        }
    }
}
