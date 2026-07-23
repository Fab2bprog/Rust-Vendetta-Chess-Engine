// =============================================================================
// Vendetta Chess Motor — src/eval/mobility.rs
//
// Rôle : Évaluation de la mobilité des pièces ET du contrôle du centre par
//        les pièces (cavalier, fou, tour, dame) — les deux critères sont
//        calculés en une seule passe par pièce, voir note d'optimisation
//        ci-dessous.
//
// Méthode :
//   Pour chaque pièce (cavalier, fou, tour, dame), on compte le nombre de
//   cases qu'elle peut atteindre sans être bloquée par ses propres pièces.
//   Les pions et le roi sont exclus : leur mobilité est gérée ailleurs
//   (structure de pions, sécurité du roi).
//
// Bonus de mobilité par case accessible (en centipions) :
//   - Cavalier : 4  (très sensible à sa mobilité, un cavalier en coin est faible)
//   - Fou      : 3  (sa force dépend de ses diagonales ouvertes)
//   - Tour     : 2  (besoin de colonnes ouvertes)
//   - Dame     : 1  (déjà très mobile, peu de bonus marginal)
//
// Optimisation — fusion avec le contrôle du centre (eval/center.rs) :
//   Avant : mobility.rs ET center.rs calculaient CHACUN, indépendamment,
//   le bitboard d'attaque de chaque cavalier/fou/tour/dame (knight_attacks,
//   bishop_attacks, rook_attacks, queen_attacks — ce dernier coûteux car il
//   combine deux lookups magic bitboard). Résultat : le même calcul fait
//   deux fois par pièce et par nœud d'évaluation.
//   Après : le bitboard d'attaque brut de chaque pièce est calculé UNE SEULE
//   FOIS ici, puis réutilisé pour les DEUX bonus :
//     - mobilité : attaques ET cases occupées par une pièce amie (!own_pieces)
//     - centre   : attaques ET les 4 cases centrales (CENTER_SQUARES),
//                  SANS exclure les cases amies — comportement identique à
//                  l'ancien center.rs (défendre le centre compte autant que
//                  l'attaquer).
//   Le résultat numérique de chaque bonus est rigoureusement inchangé par
//   rapport aux deux fonctions séparées — seul le nombre de calculs d'attaque
//   diminue de moitié. Les pions ne sont pas concernés (gérés uniquement par
//   center::center_pawn_eval(), aucune mobilité de pion évaluée ici).
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::{knight_attacks, bishop_attacks, rook_attacks, queen_attacks, king_attacks};
use super::center::{CENTER_SQUARES, CENTER_ATTACK_BONUS};
use super::king_safety::{
    king_attack_danger,
    KING_ATTACK_WEIGHT_KNIGHT, KING_ATTACK_WEIGHT_BISHOP,
    KING_ATTACK_WEIGHT_ROOK, KING_ATTACK_WEIGHT_QUEEN,
};

/// Résultat de la passe par pièce pour une couleur : mobilité, contrôle du
/// centre, et pression sur le roi adverse (unités d'attaque + nb d'attaquants).
pub struct PieceActivity {
    pub mobility:        i32,
    pub center:          i32,
    pub king_units:      i32, // somme pondérée des cases de la zone du roi adverse attaquées
    pub king_attackers:  i32, // nombre de pièces distinctes touchant cette zone
}

/// Construit le bitboard de la "zone du roi" du camp `defender` : cases
/// adjacentes au roi (+ sa case), étendues d'un rang vers l'avant (côté d'où
/// l'ennemi attaque). Les décalages verticaux écartent naturellement les bits
/// hors échiquier ; aucun débordement est-ouest possible.
#[inline]
fn king_zone(board: &Board, defender: Color) -> u64 {
    let ksq = board.king_square(defender);
    let adj = king_attacks(ksq) | (1u64 << ksq);
    adj | if defender == Color::White { adj << 8 } else { adj >> 8 }
}

/// Pression d'une pièce sur la zone du roi adverse, à partir de son bitboard
/// d'attaque DÉJÀ calculé pour la mobilité. Retourne (attaquant ? 1 : 0,
/// unités d'attaque) — un AND + un popcount, quasi gratuit.
#[inline]
fn king_pressure(attacks: u64, enemy_zone: u64, weight: i32) -> (i32, i32) {
    let hits = (attacks & enemy_zone).count_ones() as i32;
    if hits > 0 { (1, hits * weight) } else { (0, 0) }
}

/// Bonus de mobilité par case accessible, selon le type de pièce.
/// Calibrés par Texel Tuning v3 (étaient 4, 3, 2, 1) — voir
/// material.rs::PIECE_VALUE pour le contexte complet du tuning.
const KNIGHT_MOBILITY_BONUS: i32 = 11;
const BISHOP_MOBILITY_BONUS: i32 = 10;
const ROOK_MOBILITY_BONUS:   i32 = 10;
const QUEEN_MOBILITY_BONUS:  i32 = 5;

/// Calcule, pour une couleur et en UNE passe sur les pièces, la mobilité, le
/// contrôle du centre, et la pression sur le roi adverse (voir PieceActivity) —
/// tout à partir du même bitboard d'attaque par pièce.
///
/// `include_center` : si false, le bonus de centre n'est ni accumulé ni calculé
/// (finale — reproduit l'ancien comportement, voir eval/mod.rs).
/// `king_attack` : si false, la pression sur le roi n'est ni accumulée ni
/// calculée (finale, ou terme désactivé pour un test SPRT) → zéro travail.
pub fn mobility_and_center_score(
    board: &Board,
    color: Color,
    include_center: bool,
    king_attack: bool,
) -> PieceActivity {
    let own_pieces   = board.occupancy[color.index()];
    let occupied     = board.all_pieces;
    let mut mobility = 0i32;
    let mut center   = 0i32;
    let mut king_units     = 0i32;
    let mut king_attackers = 0i32;

    // Zone du roi ADVERSE (ce que `color` menace). Calculée seulement si le terme
    // king-attack est actif (hors finale / non désactivé) — sinon zéro travail.
    let enemy_zone = if king_attack { king_zone(board, color.opposite()) } else { 0 };

    // --- Cavaliers ---
    // Un cavalier coincé dans un coin perd beaucoup de puissance.
    let mut knights = board.pieces[color.index()][Piece::Knight.index()];
    while knights != 0 {
        let sq      = knights.trailing_zeros() as u8;
        knights    &= knights - 1;
        let attacks = knight_attacks(sq);
        mobility   += (attacks & !own_pieces).count_ones() as i32 * KNIGHT_MOBILITY_BONUS;
        if include_center {
            center += (attacks & CENTER_SQUARES).count_ones() as i32 * CENTER_ATTACK_BONUS;
        }
        if king_attack {
            let (a, u) = king_pressure(attacks, enemy_zone, KING_ATTACK_WEIGHT_KNIGHT);
            king_attackers += a; king_units += u;
        }
    }

    // --- Fous ---
    // Les diagonales ouvertes sont la force du fou.
    let mut bishops = board.pieces[color.index()][Piece::Bishop.index()];
    while bishops != 0 {
        let sq      = bishops.trailing_zeros() as u8;
        bishops    &= bishops - 1;
        let attacks = bishop_attacks(sq, occupied);
        mobility   += (attacks & !own_pieces).count_ones() as i32 * BISHOP_MOBILITY_BONUS;
        if include_center {
            center += (attacks & CENTER_SQUARES).count_ones() as i32 * CENTER_ATTACK_BONUS;
        }
        if king_attack {
            let (a, u) = king_pressure(attacks, enemy_zone, KING_ATTACK_WEIGHT_BISHOP);
            king_attackers += a; king_units += u;
        }
    }

    // --- Tours ---
    // Les tours ont besoin de colonnes et de rangs ouverts pour être actives.
    let mut rooks = board.pieces[color.index()][Piece::Rook.index()];
    while rooks != 0 {
        let sq      = rooks.trailing_zeros() as u8;
        rooks      &= rooks - 1;
        let attacks = rook_attacks(sq, occupied);
        mobility   += (attacks & !own_pieces).count_ones() as i32 * ROOK_MOBILITY_BONUS;
        if include_center {
            center += (attacks & CENTER_SQUARES).count_ones() as i32 * CENTER_ATTACK_BONUS;
        }
        if king_attack {
            let (a, u) = king_pressure(attacks, enemy_zone, KING_ATTACK_WEIGHT_ROOK);
            king_attackers += a; king_units += u;
        }
    }

    // --- Dames ---
    // La dame est déjà puissante : faible bonus marginal par case supplémentaire.
    let mut queens = board.pieces[color.index()][Piece::Queen.index()];
    while queens != 0 {
        let sq      = queens.trailing_zeros() as u8;
        queens     &= queens - 1;
        let attacks = queen_attacks(sq, occupied);
        mobility   += (attacks & !own_pieces).count_ones() as i32 * QUEEN_MOBILITY_BONUS;
        if include_center {
            center += (attacks & CENTER_SQUARES).count_ones() as i32 * CENTER_ATTACK_BONUS;
        }
        if king_attack {
            let (a, u) = king_pressure(attacks, enemy_zone, KING_ATTACK_WEIGHT_QUEEN);
            king_attackers += a; king_units += u;
        }
    }

    PieceActivity { mobility, center, king_units, king_attackers }
}

/// Calcule les différentiels de mobilité et de centre (pièces) du point de
/// vue du joueur actif. Score positif = avantage pour le joueur actif.
///
/// Remplace les anciens appels séparés à mobility_eval() et à la partie
/// "pièces" de center_eval() — voir eval/mod.rs pour l'assemblage avec
/// center::center_pawn_eval() (pions, non concernés par cette fusion).
pub fn mobility_and_center_eval(
    board: &Board,
    is_endgame: bool,
    king_attack: bool,
) -> (i32, i32, i32) {
    // include_center et king-attack sont tous deux inactifs en finale (le
    // contrôle du centre et la sécurité du roi n'y sont plus pertinents).
    let include_center = !is_endgame;
    let do_king_attack = king_attack && !is_endgame;

    let white = mobility_and_center_score(board, Color::White, include_center, do_king_attack);
    let black = mobility_and_center_score(board, Color::Black, include_center, do_king_attack);

    let mobility_diff = white.mobility - black.mobility;
    let center_diff   = white.center   - black.center;

    // Danger king-attack : la pression des Blancs vise le roi NOIR (bonus Blanc),
    // et inversement. Différentiel en perspective Blanc.
    let white_danger_to_black = king_attack_danger(white.king_units, white.king_attackers);
    let black_danger_to_white = king_attack_danger(black.king_units, black.king_attackers);
    let king_attack_diff = white_danger_to_black - black_danger_to_white;

    if board.side_to_move == Color::White {
        (mobility_diff, center_diff, king_attack_diff)
    } else {
        (-mobility_diff, -center_diff, -king_attack_diff)
    }
}
