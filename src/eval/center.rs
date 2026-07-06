// =============================================================================
// Vendetta Chess Motor — src/eval/center.rs
//
// Rôle : Évaluation du contrôle du centre de l'échiquier.
//        Contrôler le centre (d4, d5, e4, e5) est un principe fondamental
//        aux échecs : les pièces au centre ont plus de mobilité et peuvent
//        intervenir sur les deux ailes.
//
// Deux critères sont évalués :
//   1. Présence physique : pions sur les cases centrales (bonus direct)
//   2. Attaques : nombre de fois que le camp attaque les 4 cases centrales
//
// Cases centrales :
//   d4 = case 27, e4 = case 28, d5 = case 35, e5 = case 36
//
// Bonus (en centipions) :
//   - Pion présent sur une case centrale   : +15
//   - Attaque d'une case centrale          : +5
//
// Remarque : les attaques multiples sur la même case sont comptées
// séparément (une tour et un fou attaquant e4 = 2×5 = +10).
//
// Répartition du calcul (optimisation — voir eval/mobility.rs) :
//   Ce fichier ne gère plus QUE la partie "pions" (présence + attaques).
//   La partie "attaques des pièces" (cavalier/fou/tour/dame) sur le centre
//   a été fusionnée dans mobility.rs : ces pièces y sont déjà parcourues et
//   leur bitboard d'attaque déjà calculé pour la mobilité — réutiliser ce
//   même bitboard pour le bonus de centre évite de le recalculer une seconde
//   fois (même lookup magic bitboard, deux fois, pour rien). Les pions ne
//   sont jamais traités par mobility.rs, donc aucune fusion possible/utile
//   ici : ce module reste seul responsable de leur contribution au centre.
//   CENTER_SQUARES et CENTER_ATTACK_BONUS sont exposés (pub(crate)) pour
//   que mobility.rs les réutilise sans dupliquer ces constantes.
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::{Bitboard, white_pawn_attacks, black_pawn_attacks};

/// Masque des 4 cases centrales : d4(27), e4(28), d5(35), e5(36).
/// Visible depuis mobility.rs pour le calcul fusionné du bonus de centre
/// des pièces (cavalier/fou/tour/dame).
pub(crate) const CENTER_SQUARES: Bitboard =
    (1u64 << 27) | (1u64 << 28) | (1u64 << 35) | (1u64 << 36);

/// Bonus pour un pion physiquement présent sur une case centrale.
/// Calibré par Texel Tuning v3 (était 15) — voir material.rs::PIECE_VALUE.
const CENTER_PAWN_BONUS: i32 = 9;

/// Bonus par attaque sur une case centrale.
/// Visible depuis mobility.rs (voir note ci-dessus).
/// Calibré par Texel Tuning v3 (était 5).
pub(crate) const CENTER_ATTACK_BONUS: i32 = 6;

/// Calcule le score de contrôle du centre par les PIONS uniquement pour
/// une couleur donnée (présence + attaques). La contribution des autres
/// pièces est calculée dans mobility.rs (voir en-tête du fichier).
/// Retourne un score positif = bon contrôle du centre pour cette couleur.
pub fn center_pawn_score(board: &Board, color: Color) -> i32 {
    let mut score = 0i32;

    // --- Pions physiquement au centre (bonus direct fort) ---
    let pawns = board.pieces[color.index()][Piece::Pawn.index()];
    score += (pawns & CENTER_SQUARES).count_ones() as i32 * CENTER_PAWN_BONUS;

    // --- Attaques de pions sur le centre ---
    let pawn_attacks = if color == Color::White {
        white_pawn_attacks(pawns)
    } else {
        black_pawn_attacks(pawns)
    };
    score += (pawn_attacks & CENTER_SQUARES).count_ones() as i32 * CENTER_ATTACK_BONUS;

    score
}

/// Calcule le différentiel de contrôle du centre (pions uniquement) du
/// point de vue du joueur actif. La contribution des pièces est ajoutée
/// séparément par mobility::mobility_and_center_eval() — voir eval/mod.rs.
/// Score positif = meilleur contrôle du centre pour le joueur actif.
pub fn center_pawn_eval(board: &Board) -> i32 {
    let white_score = center_pawn_score(board, Color::White);
    let black_score = center_pawn_score(board, Color::Black);
    let diff        = white_score - black_score;

    if board.side_to_move == Color::White { diff } else { -diff }
}
