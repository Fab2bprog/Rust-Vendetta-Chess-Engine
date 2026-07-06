// =============================================================================
// Vendetta Chess Motor — src/eval/phase.rs
//
// Rôle : Détection de la phase de jeu (ouverture, milieu de partie, finale).
//        La phase influence plusieurs aspects de l'évaluation :
//        - Le roi doit être en sécurité en milieu de partie mais actif en finale
//        - Les tables de positions changent selon la phase
//        - La sécurité du roi est moins importante en finale
//
// Contenu :
//   - Calcul du score de phase basé sur le matériel restant
//   - Interpolation lisse entre milieu de partie et finale (tapered eval)
//
// Méthode :
//   On attribue un "poids de phase" à chaque type de pièce.
//   Quand le plateau est plein, on est en milieu de partie.
//   Quand les pièces lourdes disparaissent, on entre en finale.
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;

/// Poids de chaque type de pièce pour le calcul de la phase.
/// Les poids sont calibrés pour que 24 = milieu de partie complet.
const PHASE_WEIGHT: [i32; 6] = [
    0,  // Pion (ne compte pas pour la phase)
    1,  // Cavalier
    1,  // Fou
    2,  // Tour
    4,  // Dame
    0,  // Roi (ne compte pas)
];

/// Score de phase maximum (partie complète en milieu de partie).
/// 2 cavaliers + 2 fous + 4 tours + 2 dames par camp :
/// (2*1 + 2*1 + 4*2 + 2*4) * 2 camps = (2+2+8+8)*2 = 40
const MAX_PHASE: i32 = 40;

/// Représentation de la phase de jeu.
#[derive(Clone, Copy, Debug)]
pub struct GamePhase {
    /// Score de phase : 0 = finale pure, MAX_PHASE = milieu de partie pur.
    pub phase_score: i32,
}

impl GamePhase {
    /// Retourne true si on est en finale (peu de matériel).
    pub fn is_endgame(self) -> bool {
        self.phase_score < MAX_PHASE / 2
    }

    /// Retourne un facteur entre 0.0 (finale) et 1.0 (milieu de partie).
    /// Utilisé pour l'interpolation tapered.
    pub fn middlegame_factor(self) -> f32 {
        (self.phase_score as f32 / MAX_PHASE as f32).clamp(0.0, 1.0)
    }

    /// Interpolation lisse entre score milieu de partie et score finale.
    /// Permet une transition graduelle plutôt qu'un saut brutal.
    pub fn taper(self, middlegame_score: i32, endgame_score: i32) -> i32 {
        let mg = self.middlegame_factor();
        let eg = 1.0 - mg;
        (middlegame_score as f32 * mg + endgame_score as f32 * eg) as i32
    }
}

/// Calcule la phase de jeu de la position courante.
///
/// Optimisation — calcul plus léger, résultat identique :
///   Avant : 8 popcounts sur des bitboards (count_ones() sur 64 bits) ×
///           2 couleurs × 4 types de pièces.
///   Après : 8 simples lectures de board.piece_count, un tableau [u8; 6]
///           déjà maintenu en temps réel par place_piece()/remove_piece()
///           (utilisé par ailleurs pour is_insufficient_material()).
///   Même valeur garantie : piece_count est synchronisé avec les bitboards
///   à chaque coup, c'est juste une lecture directe au lieu d'un recalcul.
pub fn compute_phase(board: &Board) -> GamePhase {
    let mut phase_score = 0i32;

    for color in [Color::White, Color::Black] {
        for piece in [Piece::Knight, Piece::Bishop, Piece::Rook, Piece::Queen] {
            let count = board.piece_count[color.index()][piece.index()] as i32;
            phase_score += count * PHASE_WEIGHT[piece.index()];
        }
    }

    // Limiter entre 0 et MAX_PHASE
    phase_score = phase_score.clamp(0, MAX_PHASE);

    GamePhase { phase_score }
}
