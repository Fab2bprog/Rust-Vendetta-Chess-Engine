// =============================================================================
// Vendetta Chess Motor — src/eval/endgame.rs
//
// Rôle : Évaluations spécifiques aux finales de partie.
//        Ces six critères donnent au moteur une compréhension profonde
//        des positions gagnées, nulles ou perdues en finale.
//
// Contenu :
//   1. Bonus de distance des pions passés
//      Un pion passé à 2 rangs de la promotion vaut bien plus qu'à 6 rangs.
//      Ce bonus s'ajoute au bonus de base de pawns.rs et est amplifié en finale.
//
//   2. Proximité du roi aux pions passés
//      En finale, le roi doit escorter ses pions passés (les pousser)
//      et intercepter ceux de l'adversaire (les bloquer).
//      Critère : distance Chebyshev roi allié vs roi ennemi par rapport au pion.
//
//   3. Mop-up evaluation
//      Quand un camp a un avantage matériel décisif (≥ une tour), le roi gagnant
//      doit pousser le roi adverse vers un coin ET se rapprocher de lui.
//      Sans ce critère, le moteur peut errer et gâcher l'avantage.
//
//   4. Tour sur la 7ème rangée
//      Une tour sur la 7ème (2ème pour les Noirs) coupe le roi adverse,
//      menace les pions non avancés et domine la finale.
//
//   5. Tour derrière un pion passé (règle de Tarrasch)
//      "La tour doit toujours être placée derrière un pion passé, qu'il soit
//      ami ou ennemi." — Siegbert Tarrasch.
//      Bonus pour une tour alliée derrière un pion passé allié.
//
//   6. Fous de couleurs opposées
//      Quand chaque camp a exactement un fou sur des cases de couleurs opposées,
//      la position est fortement drawish. On réduit l'avantage matériel de 50 %
//      pour refléter cette réalité stratégique.
//
// Géométrie :
//   - Distance de Chebyshev : nombre de coups de roi minimum entre deux cases.
//   - Rang d'une case sq : sq / 8  (0 = rang 1, 7 = rang 8)
//   - Colonne d'une case sq : sq % 8  (0 = colonne a, 7 = colonne h)
//   - Couleur de case : (rang + colonne) % 2  (0 = sombre, 1 = claire)
//
// Convention de score interne :
//   - Positif = avantage Blancs, négatif = avantage Noirs.
//   - Le retournement du point de vue (joueur actif) est fait dans endgame_eval().
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::{Bitboard, file_mask, rank_mask};
use super::material::material_score;

// =============================================================================
// Utilitaires géométriques
// =============================================================================

/// Distance de Chebyshev entre deux cases (nombre minimum de coups de roi).
/// Exemples : a1→h8 = 7, e4→d5 = 1, e4→d6 = 2.
#[inline]
fn chebyshev(sq1: u8, sq2: u8) -> i32 {
    let r1 = (sq1 / 8) as i32;
    let c1 = (sq1 % 8) as i32;
    let r2 = (sq2 / 8) as i32;
    let c2 = (sq2 % 8) as i32;
    (r1 - r2).abs().max((c1 - c2).abs())
}

/// Calcule le bitboard des pions passés pour une couleur.
/// Un pion passé n'a aucun pion adverse sur sa colonne ni les colonnes adjacentes,
/// dans les rangs qui le séparent de la promotion.
/// Cette détection est identique à celle de pawns.rs mais autonome ici
/// pour ne pas modifier les fichiers existants et rester sans dépendance.
fn passed_pawns_bb(board: &Board, color: Color) -> Bitboard {
    let pawns       = board.pieces[color.index()][Piece::Pawn.index()];
    let enemy_pawns = board.pieces[color.opposite().index()][Piece::Pawn.index()];
    let mut result  = 0u64;

    let mut bb = pawns;
    while bb != 0 {
        let sq   = bb.trailing_zeros() as u8;
        bb      &= bb - 1;

        let file = sq % 8;
        let rank = sq / 8;

        let col  = file_mask(file);
        let left = if file > 0 { file_mask(file - 1) } else { 0 };
        let rght = if file < 7 { file_mask(file + 1) } else { 0 };
        let zone = col | left | rght;

        let front: Bitboard = match color {
            Color::White => {
                let mut mask = 0u64;
                for r in (rank + 1)..8 { mask |= rank_mask(r) & zone; }
                mask
            }
            Color::Black => {
                let mut mask = 0u64;
                for r in 0..rank { mask |= rank_mask(r) & zone; }
                mask
            }
        };

        if enemy_pawns & front == 0 {
            result |= 1u64 << sq;
        }
    }

    result
}

// =============================================================================
// 1. Bonus de distance des pions passés
// =============================================================================

/// Bonus supplémentaire pour les pions passés proches de la promotion.
/// Le bonus de base (pawns.rs) croît déjà avec l'avancement, mais pas assez
/// pour refléter la menace réelle d'un pion à 1 ou 2 rangs de la dame.
///
/// Ces bonus s'ajoutent au bonus de base et sont amplifiés en finale
/// (moins de pièces pour arrêter le pion = menace plus concrète).
///
/// Valeurs (extra au-dessus du bonus de base de pawns.rs) :
///   - Rang 7 / rang 2 (1 case de la dame)  : +60 mg / +120 eg
///   - Rang 6 / rang 3 (2 cases de la dame) : +25 mg / +50  eg
///   - Rang 5 / rang 4 (3 cases de la dame) : +0  mg / +15  eg
fn passed_pawn_advancement_bonus(passed: Bitboard, color: Color, is_endgame: bool) -> i32 {
    let mut score = 0i32;

    let mut bb = passed;
    while bb != 0 {
        let sq   = bb.trailing_zeros() as u8;
        bb      &= bb - 1;
        let rank = sq / 8;

        // Distance en rangs jusqu'à la case de promotion (1 = une case, 6 = loin)
        let dist = match color {
            Color::White => 7 - rank,
            Color::Black => rank,
        } as i32;

        // Bonus de pion passé selon la distance à la promotion et la phase.
        // Table (distance, finale ?) → bonus : plus le pion est avancé et plus
        // on est en finale, plus le bonus est élevé. (3, false) tombe sur 0.
        let extra = match (dist, is_endgame) {
            (1, true)  => 120,
            (1, false) => 60,
            (2, true)  => 50,
            (2, false) => 25,
            (3, true)  => 15,
            _          => 0,
        };

        score += extra;
    }

    score
}

// =============================================================================
// 2. Proximité du roi aux pions passés
// =============================================================================

/// En finale, le roi doit escorter ses pions passés vers la promotion
/// et intercepter les pions passés adverses avant qu'ils promeuvent.
///
/// Pour chaque pion passé allié, on mesure :
///   - dist_allié   : distance Chebyshev entre le roi allié et le pion
///   - dist_ennemi  : distance Chebyshev entre le roi ennemi et le pion
///   - avantage     : dist_ennemi - dist_allié
///
/// Un avantage positif (roi allié plus proche) vaut 5 cp par unité.
fn king_passed_pawn_proximity(board: &Board, passed: Bitboard, color: Color) -> i32 {
    let own_king   = board.king_square(color);
    let enemy_king = board.king_square(color.opposite());
    let mut score  = 0i32;

    let mut bb = passed;
    while bb != 0 {
        let sq  = bb.trailing_zeros() as u8;
        bb     &= bb - 1;

        let own_dist   = chebyshev(own_king, sq);
        let enemy_dist = chebyshev(enemy_king, sq);

        // Positif = roi allié plus proche = bonne escorte
        score += (enemy_dist - own_dist) * 5;
    }

    score
}

// =============================================================================
// 3. Mop-up evaluation
// =============================================================================

/// Évaluation de "nettoyage" quand un camp a un avantage matériel décisif.
///
/// Sans ce critère, le moteur peut errer inutilement quand il a largement
/// gagné, gâchant du temps ou provoquant un pat par inattention.
///
/// Deux composantes :
///   a) Confinement : pousser le roi adverse vers le bord ou le coin.
///      dist_to_edge : 0 = roi au bord, 3 = roi au centre exact.
///      Score = (3 - dist_to_edge) × 15  → max 45 cp.
///   b) Proximité : rapprocher le roi gagnant du roi perdant.
///      Score = (7 - chebyshev) × 5      → max 30 cp.
///   Total max : 75 cp (suffisant pour guider le roi sans perturber le matériel).
///
/// Condition d'activation : avantage matériel ≥ 500 cp (environ une tour).
/// En dessous, la position est trop incertaine pour imposer ce comportement.
fn mop_up_eval(board: &Board) -> i32 {
    let white_mat = material_score(board, Color::White);
    let black_mat = material_score(board, Color::Black);
    let advantage = white_mat - black_mat;

    const MOPUP_THRESHOLD: i32 = 500;
    if advantage.abs() < MOPUP_THRESHOLD {
        return 0;
    }

    let (winning, losing) = if advantage > 0 {
        (Color::White, Color::Black)
    } else {
        (Color::Black, Color::White)
    };

    let winning_king = board.king_square(winning);
    let losing_king  = board.king_square(losing);

    // a) Confinement du roi perdant vers le bord/coin
    let lk_rank      = (losing_king / 8) as i32;
    let lk_file      = (losing_king % 8) as i32;
    let dist_to_edge = lk_rank.min(7 - lk_rank).min(lk_file).min(7 - lk_file);
    let confinement  = (3 - dist_to_edge) * 15;   // 0–45 cp

    // b) Proximité du roi gagnant au roi perdant
    let king_dist  = chebyshev(winning_king, losing_king);
    let proximity  = (7 - king_dist) * 5;          // 0–30 cp

    let score = confinement + proximity;

    if winning == Color::White { score } else { -score }
}

// =============================================================================
// 4. Tour sur la 7ème rangée
// =============================================================================

/// Bonus pour une tour sur la 7ème rangée (2ème pour les Noirs).
///
/// Une tour sur la 7ème :
///   - Coupe le roi adverse sur sa rangée de départ
///   - Menace tous les pions non avancés de cette rangée
///   - S'active en milieu de partie ET en finale
///
/// Valeur : 25 cp par tour (significatif mais prudent).
fn rook_on_seventh(board: &Board, color: Color) -> i32 {
    let rooks   = board.pieces[color.index()][Piece::Rook.index()];
    let seventh = match color {
        Color::White => rank_mask(6),  // Rang 7 (index 6 en base 0)
        Color::Black => rank_mask(1),  // Rang 2 (index 1 en base 0)
    };
    (rooks & seventh).count_ones() as i32 * 25
}

// =============================================================================
// 5. Tour derrière un pion passé (règle de Tarrasch)
// =============================================================================

/// "La tour doit toujours être placée derrière le pion passé, qu'il soit
/// ami ou ennemi." — Siegbert Tarrasch.
///
/// Pour une tour alliée derrière un pion passé allié :
///   - Blancs : tour sur rang inférieur au pion passé blanc (pion monte)
///   - Noirs  : tour sur rang supérieur au pion passé noir  (pion descend)
///
/// La tour derrière son propre pion passé peut pousser celui-ci indéfiniment
/// sans se bloquer, c'est la configuration idéale.
///
/// Valeur : 20 cp par paire (tour, pion passé) conforme à la règle.
fn rook_behind_passed_pawn(board: &Board, passed: Bitboard, color: Color) -> i32 {
    let rooks  = board.pieces[color.index()][Piece::Rook.index()];
    let mut score = 0i32;

    let mut pawn_bb = passed;
    while pawn_bb != 0 {
        let pawn_sq   = pawn_bb.trailing_zeros() as u8;
        pawn_bb      &= pawn_bb - 1;
        let pawn_file = pawn_sq % 8;
        let pawn_rank = pawn_sq / 8;

        // Tours alliées sur la même colonne que le pion passé
        let file_rooks = rooks & file_mask(pawn_file);

        let mut rook_bb = file_rooks;
        while rook_bb != 0 {
            let rook_sq   = rook_bb.trailing_zeros() as u8;
            rook_bb      &= rook_bb - 1;
            let rook_rank = rook_sq / 8;

            // "Derrière" selon la direction de marche du pion
            let behind = match color {
                Color::White => rook_rank < pawn_rank,  // Pion blanc monte → tour en dessous
                Color::Black => rook_rank > pawn_rank,  // Pion noir descend → tour au-dessus
            };

            if behind {
                score += 20;
            }
        }
    }

    score
}

// =============================================================================
// 6. Fous de couleurs opposées
// =============================================================================

/// Reconnaît les finales avec fous de couleurs opposées et réduit l'avantage.
///
/// Quand les deux camps ont exactement un fou chacun sur des cases de couleurs
/// différentes, le fou du camp gagnant ne peut jamais influencer les cases
/// contrôlées par le fou ennemi. Cette configuration est fortement drawish.
///
/// Détection :
///   - Chaque camp a exactement 1 fou (plus = pas de problème)
///   - Les deux fous sont sur des cases de couleurs différentes
///   - Couleur d'une case sq : (sq/8 + sq%8) % 2  (0=sombre, 1=claire)
///
/// Action : réduire l'avantage matériel de 50 % pour ce nœud.
/// On soustrait la moitié du différentiel matériel (rapproche l'éval de 0).
///
/// Note : retourne un score du point de vue absolu (positif = Blancs).
fn opposite_colored_bishops_eval(board: &Board, is_endgame: bool) -> i32 {
    // Ce critère est spécifique aux finales
    if !is_endgame { return 0; }

    let white_bbs = board.pieces[Color::White.index()][Piece::Bishop.index()];
    let black_bbs = board.pieces[Color::Black.index()][Piece::Bishop.index()];

    // Uniquement si chaque camp a exactement un fou
    if white_bbs.count_ones() != 1 || black_bbs.count_ones() != 1 {
        return 0;
    }

    let ws = white_bbs.trailing_zeros() as u8;
    let bs = black_bbs.trailing_zeros() as u8;

    // Couleur de la case : (rang + colonne) % 2
    let wc = (ws / 8 + ws % 8) % 2;
    let bc = (bs / 8 + bs % 8) % 2;

    // Si même couleur de case → pas de fous de couleurs opposées
    if wc == bc { return 0; }

    // Fous de couleurs opposées confirmés : réduire l'avantage de 50 %
    let white_mat = material_score(board, Color::White);
    let black_mat = material_score(board, Color::Black);
    let advantage = white_mat - black_mat;

    // On retourne l'opposé de la moitié de l'avantage
    // Exemple : Blancs gagnent de 200 cp → on retourne -100
    // L'évaluation finale sera 200 (matériel) - 100 (ici) = 100 net → drawish
    -advantage / 2
}

// =============================================================================
// Fonction d'entrée — combinaison des 6 critères
// =============================================================================

/// Calcule l'évaluation complète des finales du point de vue du joueur actif.
///
/// Critères actifs en toutes phases :
///   - Tour sur la 7ème rangée
///   - Mop-up (conditionnel sur l'avantage matériel)
///   - Tour derrière pion passé (Tarrasch)
///
/// Critères actifs uniquement en finale :
///   - Bonus de distance des pions passés (amplifié en finale)
///   - Proximité du roi aux pions passés
///   - Fous de couleurs opposées
pub fn endgame_eval(board: &Board, is_endgame: bool) -> i32 {
    // --- Calcul unique des pions passés, partagé par tous les critères ci-dessous ---
    //
    // Optimisation — calcul plus léger, résultat identique :
    //   Avant : passed_pawns_bb(board, color) était appelée séparément dans
    //   passed_pawn_advancement_bonus(), king_passed_pawn_proximity() et
    //   rook_behind_passed_pawn() — jusqu'à 3 fois par couleur (6 fois au total
    //   en finale) pour calculer EXACTEMENT le même bitboard à chaque fois.
    //   Après : calculé une seule fois par couleur ici, puis transmis en
    //   paramètre aux trois fonctions. Même définition, même résultat —
    //   uniquement le nombre d'appels diminue.
    let white_passed = passed_pawns_bb(board, Color::White);
    let black_passed = passed_pawns_bb(board, Color::Black);

    // --- Toutes phases ---

    // Tour sur la 7ème (pertinente dès le milieu de partie)
    let rook_7th   = rook_on_seventh(board, Color::White)
                   - rook_on_seventh(board, Color::Black);

    // Mop-up (s'active uniquement si avantage matériel ≥ 500 cp)
    let mop_up     = mop_up_eval(board);

    // Tarrasch : tour derrière pion passé (profitable à toute phase)
    let tarrasch   = rook_behind_passed_pawn(board, white_passed, Color::White)
                   - rook_behind_passed_pawn(board, black_passed, Color::Black);

    // --- Bonus supplémentaires pour les pions passés avancés ---
    // Présents en toutes phases mais plus forts en finale (paramètre is_endgame)
    let pawn_adv   = passed_pawn_advancement_bonus(white_passed, Color::White, is_endgame)
                   - passed_pawn_advancement_bonus(black_passed, Color::Black, is_endgame);

    // --- Finale uniquement ---
    let endgame_only = if is_endgame {
        // Proximité du roi aux pions passés (escorte ou blocage)
        let king_prox  = king_passed_pawn_proximity(board, white_passed, Color::White)
                       - king_passed_pawn_proximity(board, black_passed, Color::Black);

        // Fous de couleurs opposées (réduction de l'avantage)
        // Déjà en perspective absolue (Blancs positif) → pas de retournement ici
        let ocb        = opposite_colored_bishops_eval(board, is_endgame);

        king_prox + ocb
    } else {
        0
    };

    // Score total du point de vue absolu (Blancs positif)
    let total = rook_7th + mop_up + tarrasch + pawn_adv + endgame_only;

    // Retournement du point de vue : joueur actif
    if board.side_to_move == Color::White { total } else { -total }
}
