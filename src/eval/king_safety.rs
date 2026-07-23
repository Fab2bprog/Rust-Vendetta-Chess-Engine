// =============================================================================
// Vendetta Chess Motor — src/eval/king_safety.rs
//
// Rôle : Évaluation de la sécurité du roi.
//        Un roi mal protégé est une faiblesse majeure aux échecs.
//        Ce module évalue la qualité du bouclier de pions devant le roi
//        et les menaces directes.
//
// Contenu :
//   - Évaluation du bouclier de pions (pions devant le roi après roque)
//   - Pénalité pour un roi au centre en milieu de partie
//   - Détection des colonnes ouvertes près du roi (danger)
//
// Approche simplifiée mais efficace :
//   - On regarde les pions du camp devant le roi
//   - Plus il y a de pions proches, plus le roi est en sécurité
//   - Un roi au centre en milieu de partie est pénalisé
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::{king_attacks, file_mask};

/// Bonus par pion du bouclier présent (pion devant le roi).
/// Calibré par Texel Tuning v3 (était 10) — voir material.rs::PIECE_VALUE.
const SHIELD_PAWN_BONUS: i32 = 14;

/// Pénalité pour un roi au centre (colonnes c, d, e, f) en milieu de partie.
/// Calibré par Texel Tuning v3 (était -30). Changement le plus marqué du
/// tuning — à surveiller particulièrement lors du test A/B en partie réelle.
const KING_CENTER_PENALTY: i32 = -7;

/// Pénalité pour une colonne ouverte ou semi-ouverte à côté du roi.
/// Calibré par Texel Tuning v3 (était -15).
const OPEN_FILE_NEAR_KING_PENALTY: i32 = -21;

/// Évalue la sécurité du roi pour une couleur donnée.
/// Retourne un score (positif = bon pour cette couleur).
pub fn king_safety_score(board: &Board, color: Color, is_endgame: bool) -> i32 {
    // En finale, la sécurité du roi est moins importante
    if is_endgame {
        return 0;
    }

    let mut score = 0i32;
    let king_sq   = board.king_square(color);
    let king_file = king_sq % 8;
    let pawns     = board.pieces[color.index()][Piece::Pawn.index()];

    // --- Bouclier de pions ---
    // Les cases directement devant le roi (et en diagonale) doivent avoir des pions.
    // Compter les pions dans la zone du roi (bouclier)
    let shield_pawns_area = king_attacks(king_sq) | (1u64 << king_sq);
    let shield_count = (shield_pawns_area & pawns).count_ones() as i32;
    score += shield_count * SHIELD_PAWN_BONUS;

    // --- Pénalité pour roi au centre ---
    // Colonnes c(2), d(3), e(4), f(5) sont centrales
    if (2..=5).contains(&king_file) {
        score += KING_CENTER_PENALTY;
    }

    // --- Colonnes ouvertes près du roi ---
    // Vérifier les colonnes adjacentes au roi
    let enemy_rooks_queens = board.pieces[color.opposite().index()][Piece::Rook.index()]
                           | board.pieces[color.opposite().index()][Piece::Queen.index()];

    if enemy_rooks_queens != 0 {
        for f in king_file.saturating_sub(1)..=(king_file + 1).min(7) {
            let col = file_mask(f);
            // Colonne ouverte si pas de pion ami dessus
            if pawns & col == 0 {
                score += OPEN_FILE_NEAR_KING_PENALTY;
            }
        }
    }

    score
}

/// Calcule le différentiel de sécurité du roi du point de vue du joueur actif.
pub fn king_safety_eval(board: &Board, is_endgame: bool) -> i32 {
    let white_score = king_safety_score(board, Color::White, is_endgame);
    let black_score = king_safety_score(board, Color::Black, is_endgame);
    let diff = white_score - black_score;

    if board.side_to_move == Color::White { diff } else { -diff }
}

// =============================================================================
// Sécurité du roi par l'ATTAQUE (king danger) — nouveau terme
// =============================================================================
//
// Complète le bouclier de pions ci-dessus, qui ne voit PAS les pièces ennemies
// massées autour du roi. Idée classique (CPW "king safety" / Stockfish king
// danger) : plus il y a de pièces ennemies qui attaquent la ZONE du roi, et
// plus elles sont lourdes, plus le roi est en danger — et ce danger monte de
// façon NON LINÉAIRE (deux attaquants coordonnés valent bien plus que le double
// d'un seul). Les "unités d'attaque" (somme pondérée des cases de la zone
// attaquées) sont accumulées dans la passe mobilité (eval/mobility.rs), qui
// calcule déjà les bitboards d'attaque — voir king_attack_danger() ci-dessous
// pour la conversion unités → pénalité.

/// Poids d'attaque par type de pièce (par case de la zone du roi attaquée).
/// Une pièce lourde près du roi est bien plus menaçante qu'une pièce légère.
pub const KING_ATTACK_WEIGHT_KNIGHT: i32 = 2;
pub const KING_ATTACK_WEIGHT_BISHOP: i32 = 2;
pub const KING_ATTACK_WEIGHT_ROOK:   i32 = 3;
pub const KING_ATTACK_WEIGHT_QUEEN:  i32 = 5;

/// Diviseur de la montée quadratique du danger (plus grand = plus prudent).
/// Réglage retenu : 16 (conservateur). Le v2 plus agressif (10) a été testé
/// SPRT et FAIL (−0.5 vs +3.1) → pousser le terme sur-attaque et perd l'Elo.
const KING_DANGER_DIV: i32 = 16;
/// Plafond du danger (centipions) pour éviter des évaluations délirantes qui
/// pousseraient à des sacrifices douteux. Réglage retenu : 100 (v2 à 150 : FAIL).
const KING_DANGER_CAP: i32 = 100;

/// Convertit des "unités d'attaque" (et le nombre d'attaquants) en pénalité de
/// danger pour le camp dont le roi est visé (= bonus pour l'attaquant), en
/// centipions, positive.
///
/// Exige AU MOINS 2 attaquants : une seule pièce qui touche la zone du roi
/// n'est pas une vraie attaque (sinon on pénaliserait du bruit). Montée
/// quadratique bornée (CONSERVATRICE — à tuner par SPRT).
#[inline]
pub fn king_attack_danger(units: i32, attackers: i32) -> i32 {
    if attackers < 2 || units <= 0 {
        return 0;
    }
    (units * units / KING_DANGER_DIV).min(KING_DANGER_CAP)
}
