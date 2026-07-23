// =============================================================================
// Vendetta Chess Motor — src/search/see.rs
//
// Rôle : Static Exchange Evaluation (SEE).
//        Évalue le résultat net d'une séquence de captures sur une case,
//        chaque camp jouant toujours sa pièce la moins précieuse (LVA =
//        Least Valuable Attacker). Chaque camp peut choisir d'arrêter la
//        séquence s'il perdrait du matériel en continuant.
//
// Utilisation dans Vendetta Chess Motor :
//   1. Ordonnancement des captures dans move_score()
//      → Captures gagnantes (SEE ≥ 0) avant les coups silencieux.
//      → Captures perdantes (SEE < 0) après les coups silencieux.
//   2. Élagage en quiescence
//      → On ignore les captures avec SEE < 0 (trop coûteuses).
//
// Algorithme (itératif, gains[32] sur la pile) :
//
//   Passe avant :
//     gains[d] = valeur de la pièce que le camp courant pourrait capturer
//                à la profondeur d (la pièce adverse sur `to` à ce moment).
//     On retire chaque LVA de l'occupancy → révèle les rayons X automatiquement.
//
//   Passe arrière (minimax + stand-pat) :
//     result = 0
//     for d in (0..depth).rev():
//       result = max(0, gains[d] - result)
//
//   Résultat final = captured_value - result
//
// Gestion des rayons X (X-ray) :
//   En retirant la pièce qui vient de capturer de l'`occupied`, les nouvelles
//   attaques de pièces glissantes placées derrière elle sont naturellement
//   révélées lors du recalcul bishop_attacks / rook_attacks / queen_attacks.
//
// Limitations acceptées (standard dans tous les moteurs) :
//   - Les clouages ne sont PAS vérifiés (trop coûteux, effet négligeable sur la qualité)
//   - En passant → retour 0 (pion échange pion, résultat supposé neutre)
//   - Promotion → le pion est traité comme une dame (promotion la plus courante)
// =============================================================================

use crate::utils::types::{Color, Piece, Move, MoveFlags};
use crate::board::state::Board;
use crate::board::bitboard::{
    bishop_attacks, rook_attacks, queen_attacks, knight_attacks, king_attacks,
};
use crate::eval::material::piece_value;

// =============================================================================
// Helpers internes
// =============================================================================

/// Retourne le bitboard des pions de `side` présents dans `side_pawns` qui
/// attaquent la case `to`.
///
/// Un pion blanc sur la case `sq` attaque `sq+7` et `sq+9`.
/// Un pion noir sur la case `sq` attaque `sq-7` et `sq-9`.
/// On cherche donc quels pions sont en position d'attaquer `to`.
fn pawn_attackers_to(to: u8, side: Color, side_pawns: u64) -> u64 {
    let file = to % 8;
    let mut mask = 0u64;

    match side {
        Color::White => {
            // Pion blanc sur (to - 9) attaque `to` (diagonale bas-gauche)
            // Conditions : to >= 9 et file(to) > 0 (évite le débordement de colonne)
            if file > 0 {
                if let Some(sq) = to.checked_sub(9) {
                    mask |= 1u64 << sq;
                }
            }
            // Pion blanc sur (to - 7) attaque `to` (diagonale bas-droite)
            // Conditions : to >= 7 et file(to) < 7
            if file < 7 {
                if let Some(sq) = to.checked_sub(7) {
                    mask |= 1u64 << sq;
                }
            }
        }
        Color::Black => {
            // Utiliser u16 pour éviter le débordement (to est u8, max 63)
            let to_u16 = to as u16;
            // Pion noir sur (to + 7) attaque `to` (diagonale haut-gauche)
            // Conditions : to + 7 < 64 et file(to) > 0
            if file > 0 && to_u16 + 7 < 64 {
                mask |= 1u64 << (to + 7);
            }
            // Pion noir sur (to + 9) attaque `to` (diagonale haut-droite)
            // Conditions : to + 9 < 64 et file(to) < 7
            if file < 7 && to_u16 + 9 < 64 {
                mask |= 1u64 << (to + 9);
            }
        }
    }

    mask & side_pawns
}

/// Trouve la pièce la moins précieuse (LVA) du camp `side` qui attaque la case
/// `to` dans l'occupancy courante `occupied`.
///
/// Retourne `Some((square_du_lva, valeur_du_lva))` ou `None` si aucun attaquant.
///
/// L'ordre de test (du moins précieux au plus précieux) garantit le LVA :
///   Pion → Cavalier → Fou → Tour → Dame → Roi
///
/// Note : `occupied` est passé explicitement (et peut différer du plateau
/// initial) pour prendre en compte les rayons X après chaque capture.
fn find_lva(board: &Board, to: u8, side: Color, occupied: u64) -> Option<(u8, i32)> {
    let idx = side.index();

    // --- Pions ---
    let pawns = board.pieces[idx][Piece::Pawn.index()] & occupied;
    let pawn_atk = pawn_attackers_to(to, side, pawns);
    if pawn_atk != 0 {
        let sq = pawn_atk.trailing_zeros() as u8;
        // Si le pion capture sur le dernier rang, il promeut → valeur de dame
        let is_promo = match side {
            Color::White => to / 8 == 7,
            Color::Black => to / 8 == 0,
        };
        let val = if is_promo {
            piece_value(Piece::Queen)
        } else {
            piece_value(Piece::Pawn)
        };
        return Some((sq, val));
    }

    // --- Cavaliers ---
    let knights = board.pieces[idx][Piece::Knight.index()] & occupied;
    let knight_atk = knight_attacks(to) & knights;
    if knight_atk != 0 {
        let sq = knight_atk.trailing_zeros() as u8;
        return Some((sq, piece_value(Piece::Knight)));
    }

    // --- Fous ---
    // Utilise `occupied` pour les attaques réelles (rayons X).
    let bishops = board.pieces[idx][Piece::Bishop.index()] & occupied;
    let bishop_atk = bishop_attacks(to, occupied) & bishops;
    if bishop_atk != 0 {
        let sq = bishop_atk.trailing_zeros() as u8;
        return Some((sq, piece_value(Piece::Bishop)));
    }

    // --- Tours ---
    let rooks = board.pieces[idx][Piece::Rook.index()] & occupied;
    let rook_atk = rook_attacks(to, occupied) & rooks;
    if rook_atk != 0 {
        let sq = rook_atk.trailing_zeros() as u8;
        return Some((sq, piece_value(Piece::Rook)));
    }

    // --- Dames ---
    let queens = board.pieces[idx][Piece::Queen.index()] & occupied;
    let queen_atk = queen_attacks(to, occupied) & queens;
    if queen_atk != 0 {
        let sq = queen_atk.trailing_zeros() as u8;
        return Some((sq, piece_value(Piece::Queen)));
    }

    // --- Roi (en dernier : valeur très haute pour éviter de le sacrifier) ---
    let kings = board.pieces[idx][Piece::King.index()] & occupied;
    let king_atk = king_attacks(to) & kings;
    if king_atk != 0 {
        let sq = king_atk.trailing_zeros() as u8;
        return Some((sq, piece_value(Piece::King)));
    }

    None
}

// =============================================================================
// Point d'entrée public — SEE itératif
// =============================================================================

/// Évalue statiquement l'échange de captures déclenché par le coup `mv`.
///
/// Retourne le gain net (en centipions) pour le camp qui effectue `mv` :
///   - Positif  → capture gagnante  (on gagne du matériel net)
///   - Zéro     → capture neutre    (échange exact)
///   - Négatif  → capture perdante  (on perd du matériel net)
///
/// Algorithme itératif avec tableau de gains sur la pile (`gains[32]`).
/// Remplace l'ancienne version récursive (profondeur max ~8) — zéro overhead
/// d'appel de fonction, zéro stack frame récursif.
///
/// Principe :
///   1. Passe avant : on simule chaque recapture successive en trouvant le LVA
///      de chaque camp à tour de rôle. On stocke dans `gains[d]` la valeur de
///      la pièce que le camp courant pourrait capturer à la profondeur d.
///   2. Passe arrière : on propage de la profondeur maximale vers 0 en appliquant
///      `result = max(0, gains[d] - result)` — chaque camp peut refuser de
///      capturer si l'échange lui est défavorable (stand-pat).
///   3. Résultat final = `captured_value - result`.
///
/// Gestion des rayons X (X-ray) :
///   Chaque LVA est retiré de l'occupancy avant la prochaine itération.
///   Les pièces glissantes placées derrière lui sont ainsi naturellement révélées
///   lors du prochain appel à bishop_attacks / rook_attacks / queen_attacks.
///
/// Cas spéciaux :
///   - En passant          → retourne 0 (pion ×× pion, supposé neutre)
///   - Non-capture         → retourne 0 (SEE non applicable)
///   - Pion qui promeut    → traité comme une dame (hypothèse standard)
pub fn see(board: &Board, mv: Move) -> i32 {
    // En passant : la case `to` est vide (le pion capturé est sur une autre case).
    // On suppose l'échange équitable (pion contre pion).
    if mv.flags == MoveFlags::EnPassant {
        return 0;
    }

    // SEE applicable uniquement aux captures (Capture ou PromotionCapture)
    if !mv.flags.is_capture() {
        return 0;
    }

    let to   = mv.to;
    let from = mv.from;

    // Valeur de la pièce capturée (sur la case `to`)
    let captured_value = match board.piece_at(to) {
        Some((p, _)) => piece_value(p),
        None         => return 0,
    };

    // Valeur de la pièce capturante (sur la case `from`)
    // Si c'est une promotion-capture, le pion promeut → on le traite comme une dame
    let attacker_value = match board.piece_at(from) {
        Some((Piece::Pawn, _)) if mv.flags == MoveFlags::PromotionCapture => {
            piece_value(Piece::Queen)
        }
        Some((p, _)) => piece_value(p),
        None         => return 0,
    };

    // Retirer la pièce capturante de l'occupancy initiale.
    // (Elle s'est déplacée sur `to` ; les rayons X derrière elle seront révélés.)
    let mut occ  = (board.occupancy[0] | board.occupancy[1]) & !(1u64 << from);
    let mut side = board.side_to_move.opposite(); // Premier à recapturer = adversaire

    // --- Passe avant : simulation de la séquence d'échanges ---
    //
    // gains[d] = valeur de la pièce que le camp courant POURRAIT capturer à la
    //            profondeur d (c'est-à-dire la valeur de la pièce adverse qui vient
    //            de capturer, et que ce camp peut maintenant prendre).
    //
    // Invariant : `next_target` est la valeur de la pièce qui occupe `to` après
    //             la capture précédente et qui constitue la nouvelle cible.
    let mut gains       = [0i32; 32];
    let mut depth       = 0usize;
    let mut next_target = attacker_value; // La première pièce à recapturer = notre attaquant

    while depth < 32 {
        match find_lva(board, to, side, occ) {
            None => break, // Plus d'attaquant pour ce camp → échange terminé
            Some((lva_sq, lva_val)) => {
                // Ce camp peut capturer `next_target` avec son LVA.
                gains[depth] = next_target;

                // Le LVA quitte sa case → retrait de l'occupancy (révèle les rayons X).
                occ         &= !(1u64 << lva_sq);
                // La prochaine cible sera ce LVA (l'adversaire pourra le prendre).
                next_target  = lva_val;
                side         = side.opposite();
                depth       += 1;
            }
        }
    }

    // --- Passe arrière : minimax avec stand-pat ---
    //
    // On remonte de la profondeur maximale vers 0.
    // `result` représente le gain net que le camp courant peut espérer s'il capture.
    // Chaque camp choisit : capturer (`gains[d] - result`) ou passer (0).
    //
    // max(0, gains[d] - result) = stand-pat : on ne capture pas si c'est perdant.
    let mut result = 0i32;
    for d in (0..depth).rev() {
        result = (gains[d] - result).max(0);
    }

    // Résultat final pour le joueur qui effectue `mv` :
    //   on prend `captured_value`, l'adversaire peut recapturer (coût = `result`).
    //
    // Note : on NE fait PAS max(0, ...) ici.
    // Une valeur négative signifie une capture perdante → utile pour
    // l'ordonnancement et l'élagage en quiescence.
    captured_value - result
}
