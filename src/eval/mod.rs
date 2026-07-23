// =============================================================================
// Vendetta Chess Motor — src/eval/mod.rs
//
// Rôle : Fonction d'évaluation principale. Combine tous les critères
//        d'évaluation en un score unique représentant la qualité de la position
//        du point de vue du joueur qui a le trait.
//
// Critères pris en compte (par ordre d'importance) :
//   1. Matériel (valeur des pièces + bonus paire de fous)
//   2. Positions (piece-square tables)
//   3. Mobilité (cases accessibles par chaque pièce)
//   4. Contrôle du centre (présence et attaques sur d4/d5/e4/e5)
//   5. Structure de pions (doublés, isolés, passés)
//   6. Sécurité du roi (bouclier de pions + danger par l'attaque ennemie)
//   7. Spécificités de finale (mop-up, tour 7ème, roi près des pions passés)
//   8. Menaces / pièces en prise (attaquée par moins cher, ou non défendue)
//   9. Tempo (bonus fixe pour le joueur qui a le trait)
//
// Convention :
//   - Score positif → avantageux pour le joueur actif
//   - Score négatif → désavantageux pour le joueur actif
//   - L'unité est le centipion (100 = valeur d'un pion)
// =============================================================================

pub mod material;
pub mod position;
pub mod tables;
pub mod pawns;
pub mod king_safety;
pub mod phase;
pub mod mobility;
pub mod center;
pub mod endgame;
pub mod threats;

use crate::board::state::Board;
use crate::utils::types::Color;
use material::bishop_pair_eval;
use pawns::pawn_eval;
use king_safety::king_safety_eval;
use phase::compute_phase;
use mobility::mobility_and_center_eval;
use center::center_pawn_eval;
use endgame::endgame_eval;
use threats::threats_eval;

/// Évalue la position courante du point de vue du joueur qui a le trait.
/// Retourne un score en centipions.
///
/// Score positif = bon pour le joueur actif.
/// Score négatif = mauvais pour le joueur actif.
///
/// Optimisation incrémentale :
///   Les composantes matériel et PST ne sont plus recalculées ici.
///   Elles sont maintenues en temps réel dans board.eval_mg / board.eval_eg
///   par place_piece() et remove_piece(), ce qui élimine O(32) itérations
///   sur les pièces à chaque nœud feuille.
pub fn evaluate(board: &Board) -> i32 {
    // Jeu normal : tous les termes actifs, dont la sécurité du roi par l'attaque.
    evaluate_opt(board, true)
}

/// Variante d'`evaluate()` avec interrupteur du terme "king attack" (sécurité du
/// roi par l'attaque). Sert aux tests SPRT du binaire selfplay, qui isolent ce
/// terme : `king_attack = false` ⇒ comportement strictement identique à l'éval
/// d'avant ce terme (bouclier de pions seul). En jeu normal, on passe toujours
/// `true` via `evaluate()`.
pub fn evaluate_opt(board: &Board, king_attack: bool) -> i32 {
    // Calculer la phase de jeu (milieu de partie ou finale).
    let phase      = compute_phase(board);
    let is_endgame = phase.is_endgame();

    // --- 1+3. Matériel + PST (incrémental, O(1)) ---
    // board.eval_mg / board.eval_eg sont en perspective Blanc (Blanc − Noir).
    // On choisit la table appropriée selon la phase, puis on oriente
    // selon le joueur actif.
    let mat_pst = if is_endgame { board.eval_eg } else { board.eval_mg };
    let mat_pst_relative = if board.side_to_move == Color::White { mat_pst } else { -mat_pst };

    // --- 2. Bonus paire de fous (2 × count_ones, négligeable) ---
    let bishop_pair = bishop_pair_eval(board);

    // --- 4+5. Mobilité + Contrôle du centre (pièces), calculés en une passe ---
    // Optimisation : mobility.rs et center.rs calculaient chacun séparément
    // les bitboards d'attaque des cavaliers/fous/tours/dames. Désormais une
    // seule fonction fait les deux à la fois (même bitboard d'attaque brut
    // réutilisé pour les deux bonus) — résultat numérique identique, moitié
    // moins de lookups d'attaque (dont les magic bitboards, les plus coûteux).
    // include_center = !is_endgame reproduit exactement l'ancien comportement
    // (center_eval() jamais appelé en finale).
    let (mobility, center_pieces, king_attack_danger) =
        mobility_and_center_eval(board, is_endgame, king_attack);

    // --- Contrôle du centre (pions) — non concernés par la fusion ci-dessus ---
    let center_pawns = if is_endgame { 0 } else { center_pawn_eval(board) };
    let center       = center_pieces + center_pawns;

    // --- 6. Structure de pions ---
    let pawn_structure = pawn_eval(board);

    // --- 7. Sécurité du roi ---
    let king_safety = king_safety_eval(board, is_endgame);

    // --- 8. Spécificités de finale ---
    let endgame     = endgame_eval(board, is_endgame);

    // --- 9. Menaces / pièces en prise ---
    // Volontairement PAS conditionné par is_endgame (contrairement à
    // mobility/center/king_safety) : une pièce en prise est une faiblesse
    // à tout moment de la partie, pas seulement en milieu de partie.
    let threats = threats_eval(board);

    mat_pst_relative + bishop_pair + mobility + center
        + pawn_structure + king_safety + king_attack_danger + endgame + threats + TEMPO_BONUS
}

/// Bonus de tempo : avoir le trait est un avantage en soi (initiative,
/// menaces potentielles, l'adversaire doit répondre). evaluate() est déjà
/// "du point de vue du joueur qui a le trait" (voir convention en en-tête
/// de ce fichier) — une simple constante ajoutée à la fin suffit donc à
/// représenter cet avantage, sans calcul supplémentaire.
///
/// Valeur initiale raisonnable, cohérente avec les valeurs typiques utilisées
/// par d'autres moteurs (10-30 cp) — à affiner par Texel Tuning si une
/// version future étend le tuner à ce paramètre (actuellement non inclus,
/// comme threats.rs — voir CLAUDE.md "Chantiers futurs").
const TEMPO_BONUS: i32 = 10;

/// Vérifie si la position est nulle par matériel insuffisant pour mater.
///
/// Optimisation : utilise board.piece_count (u8, mis à jour incrémentalement)
/// au lieu d'appeler count_ones() sur 10 bitboards à chaque nœud.
/// Coût : 10 lectures de u8 au lieu de 10 opérations popcnt sur u64.
pub fn is_insufficient_material(board: &Board) -> bool {
    use crate::utils::types::Piece;

    let pc = &board.piece_count;
    let w  = 0usize; // index Blanc
    let b  = 1usize; // index Noir
    let p  = Piece::Pawn.index();
    let n  = Piece::Knight.index();
    let bi = Piece::Bishop.index();
    let r  = Piece::Rook.index();
    let q  = Piece::Queen.index();

    // Bail-out immédiat (cas le plus courant — ~99 % des nœuds) :
    // pions, tours ou dames présentes → matériel suffisant pour mater.
    if pc[w][p] + pc[b][p] + pc[w][r] + pc[b][r] + pc[w][q] + pc[b][q] > 0 {
        return false;
    }

    let wn = pc[w][n]; let wb = pc[w][bi];
    let bn = pc[b][n]; let bb = pc[b][bi];

    // KvK
    if wn + wb + bn + bb == 0 { return true; }

    // K+N vs K  ou  K+B vs K
    if wn + wb <= 1 && bn + bb == 0 { return true; }
    if bn + bb <= 1 && wn + wb == 0 { return true; }

    // KN/KB vs KN/KB (max 1 pièce légère par camp — aucun mat forcé)
    if wn + wb <= 1 && bn + bb <= 1 { return true; }

    // KNN vs K (deux cavaliers ne peuvent pas forcer le mat)
    if wn == 2 && wb == 0 && bn + bb == 0 { return true; }
    if bn == 2 && bb == 0 && wn + wb == 0 { return true; }

    false
}
