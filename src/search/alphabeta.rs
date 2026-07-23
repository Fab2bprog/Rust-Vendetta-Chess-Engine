// =============================================================================
// Vendetta Chess Motor — src/search/alphabeta.rs
//
// Rôle : Algorithme de recherche alpha-bêta avec toutes ses heuristiques.
//        C'est le cœur de l'intelligence du moteur.
//
// Contenu :
//   - alpha_beta() : recherche principale avec élagage alpha-bêta
//   - quiescence() : recherche de quiescence (captures ; gestion correcte des
//     positions où le camp au trait subit un échec — voir doc de la fonction)
//   - order_moves() : ordonnancement des coups pour maximiser les coupures
//   - Principal Variation Search (PVS)
//   - Null move pruning
//   - Late Move Reduction (LMR)
//   - Late Move Pruning (LMP)
//   - Internal Iterative Reduction (IIR)
//   - Mate Distance Pruning
//   - Reverse Futility Pruning (Static Null Move)
//   - Razoring (anciennement nommé "Futility Pruning" dans ce fichier —
//     renommé pour correspondre à la terminologie standard)
//   - Delta Pruning (futility en quiescence)
//   - Check Extension
//   - Singular Extension (SE)
//   - Killer moves, history heuristic et countermove heuristic intégrés
//
// Principe de l'alpha-bêta :
//   On maintient une fenêtre [alpha, beta].
//   - alpha : meilleur score que le joueur actif est assuré d'obtenir
//   - beta  : meilleur score que l'adversaire est assuré d'obtenir
//   Si un coup dépasse beta, l'adversaire l'éviterait → on coupe (beta cutoff).
//   Si un coup améliore alpha, on met à jour le meilleur coup.
//
// Principal Variation Search (PVS) — principe détaillé :
//   Un bon ordonnancement place le meilleur coup en premier dans la grande
//   majorité des nœuds. On exploite cette propriété :
//     - Coup n°1 (move_index == 0) : recherché à pleine fenêtre [alpha, beta].
//       On a besoin de son score exact pour établir la vraie référence.
//     - Coups suivants : d'abord sondés à fenêtre NULLE [-alpha-1, -alpha].
//       Une fenêtre nulle ne demande qu'une réponse booléenne ("dépasse-t-il
//       alpha ?"), ce qui génère beaucoup plus de coupures bêta internes
//       qu'une fenêtre complète → arbre de recherche nettement plus petit.
//       Si la sonde dépasse alpha, le coup est potentiellement meilleur que
//       prévu : on le re-recherche à pleine fenêtre pour obtenir son score exact.
//   Gain typique mesuré dans la littérature : 10-20 % de nœuds en moins par
//   rapport à un alpha-bêta "naïf" qui rechercherait tous les coups à pleine
//   fenêtre. Se combine naturellement avec LMR (la sonde à fenêtre nulle est
//   aussi l'endroit où la réduction de profondeur s'applique).
//
// Singular Extension — principe détaillé :
//   Un coup TT est "singulier" si, en l'excluant de la recherche, aucun autre
//   coup ne peut atteindre un score proche du sien. Pour le vérifier, on lance
//   une recherche à profondeur réduite (depth/2) avec le coup TT exclu et une
//   fenêtre nulle juste sous son score. Si cette recherche échoue (fail-low),
//   le coup est singulier et on l'explore un niveau de plus.
//   Précaution : la recherche SE ne doit jamais être récursive (on vérifie
//   excluded_move.is_null() avant de lancer SE).
//
// Philosophie : clarté et correction avant optimisation.
//   Chaque heuristique est clairement séparée et documentée.
// =============================================================================

use std::sync::OnceLock;

use crate::utils::types::{Color, Piece, Move, MoveFlags, SCORE_INF, SCORE_MATE, SCORE_DRAW};
use crate::board::state::Board;
use crate::moves::{generate_legal_moves_into, generate_legal_captures_into, is_in_check, MoveList, MAX_MOVE_LIST};
use crate::eval::{evaluate_opt, is_insufficient_material};
use crate::eval::material::piece_value;
use super::transposition::{TranspositionTable, TTFlag};
use super::see::see;
use super::EVAL_HISTORY_NONE;
use super::killers::KillerMoves;
use super::history::HistoryTable;
use super::countermove::CountermoveTable;
use super::continuation_history::ContinuationHistoryTable;
use super::SearchInfo;
use super::CorrKeys;
// EVAL_HISTORY_NONE (défini dans search/mod.rs) sert de sentinelle "aucune
// éval enregistrée à ce ply" pour la pile eval_history du drapeau "improving"
// — voir le bloc "improving" détaillé dans alpha_beta().

// Nombre maximum théorique de coups LÉGAUX dans une position d'échecs.
// (position record : "R6R/3Q4/1Q4Q1/4Q3/2Q4Q/Q4Q2/pp1Q4/kBNN1KB1 w - - 0 1" → 218 coups)
// Utilisé pour dimensionner les tableaux sur la pile indexés par coup légal
// (`scores`, `lmp_pruned`) dans la boucle de coups.
//
// À ne pas confondre avec `moves::MAX_MOVE_LIST` (= 256) : ce dernier est la
// CAPACITÉ d'une MoveList (tampon de pseudo-coups avant filtrage légal), volontairement
// plus large et arrondie à une puissance de 2 pour la marge. Ici on indexe des coups
// déjà filtrés LÉGAUX, dont le nombre est borné par ce maximum exact de 218.
const MAX_MOVES: usize = 218;

// Profondeur maximale absolue de la recherche en plies depuis la racine.
//
// Borne de sécurité critique pour la Check Extension.
//
// Problème : la Check Extension ajoute +1 à la profondeur de l'enfant quand un coup
// donne échec et que depth ≤ 4 :
//   depth_enfant = depth - 1 + 1 = depth   (la profondeur NE DÉCROÎT PAS)
// Si chaque coup de la branche donne échec, la récursion ne termine jamais.
//
// Correction : la condition d'extension inclut `ply + 1 < MAX_PLY`.
// Au-delà de ce seuil, toute extension est désactivée → depth décroît normalement → terminaison garantie.
//
// Valeur choisie : 128 (max_depth en mode infinite) + 64 niveaux d'extension = 192.
// En pratique, les échecs perpétuels sont détectés bien avant (répétition de position).
// Cette borne est un filet de sécurité absolu, jamais atteint sur des parties réelles.
//
// Visibilité pub(crate) : killers.rs réutilise CETTE constante (au lieu d'en
// définir une seconde) pour dimensionner sa table — corrige une incohérence
// découverte lors d'un audit (killers.rs avait sa propre copie figée à 128,
// soit moins que les 192 plies que la recherche peut théoriquement atteindre
// avec les extensions ; au-delà de 128, les killer moves étaient silencieusement
// désactivés). Une seule source de vérité évite que les deux valeurs divergent
// à nouveau à l'avenir.
pub(crate) const MAX_PLY: usize = 128 + 64;

// Profondeur maximale de la recherche de quiescence.
// En pratique, le SEE filtre les captures perdantes et le stand-pat écrête la récursion,
// mais une position pathologique avec de nombreuses captures gagnantes en chaîne pourrait
// déborder la pile sans cette garde explicite. 64 niveaux = largement suffisant pour tout
// échange concevable, sans coût notable (cas extrêmement rare).
const MAX_QUIESCENCE_PLY: usize = MAX_PLY + 64; // MAX_PLY + 64 niveaux de quiescence

// =============================================================================
// Ordonnancement des coups
// =============================================================================

/// Score d'ordonnancement pour un coup (plus élevé = testé en premier).
/// Un bon ordonnancement est crucial pour l'efficacité de l'alpha-bêta.
// Nombre d'arguments volontairement élevé : l'ordonnancement combine plusieurs
// heuristiques (TT, killers, history, countermove, continuation history), chacune
// dans sa propre structure. Les regrouper masquerait la nature des dépendances.
#[allow(clippy::too_many_arguments)]
fn move_score(
    board:        &Board,
    mv:           Move,
    tt_move:      Move,
    killers:      &KillerMoves,
    history:      &HistoryTable,
    countermoves: &CountermoveTable,
    cont_history: &ContinuationHistoryTable,
    prev_key:     Option<(Piece, u8)>,
    ply:          usize,
) -> i32 {
    // 1. Coup de la table de transposition (meilleur coup connu) → priorité absolue
    if mv == tt_move && !tt_move.is_null() {
        return 2_000_000;
    }

    // 2. Captures : évaluées par SEE (Static Exchange Evaluation).
    //    SEE simule toute la séquence de captures sur la case cible et retourne
    //    le gain net pour le camp qui capture.
    //    - SEE ≥ 0 : capture gagnante ou neutre → priorité haute (1_000_000 + see)
    //    - SEE < 0 : capture perdante → priorité très basse (négative)
    //      Explorées après les coups silencieux, évitées en quiescence.
    if mv.flags.is_capture() {
        let see_score = see(board, mv);
        return if see_score >= 0 {
            1_000_000 + see_score   // Capture gagnante : avant les coups silencieux
        } else {
            see_score               // Capture perdante : après les coups silencieux
        };
    }

    // 3. Promotions dame (sans capture)
    if mv.flags == MoveFlags::Promotion && mv.promotion == 4 {
        return 900_000;
    }

    // 4. Killer moves (coups silencieux qui ont causé des coupures récemment).
    //    Killer 1 (le plus récemment enregistré, voir KillerMoves::store) est
    //    légèrement préféré à killer 2 : à enregistrement égal, le plus récent
    //    est statistiquement le candidat le plus pertinent dans cette branche.
    //    L'écart (10 points) est minime — juste assez pour les départager sans
    //    jamais les faire chuter sous le seuil des promotions dame (900_000).
    let km = killers.get(ply);
    if mv == km[0] {
        return 810_000;
    }
    if mv == km[1] {
        return 800_000;
    }

    // 5. Countermove : coup qui a déjà réfuté le dernier coup adverse joué
    //    (même pièce + même case d'arrivée) ailleurs dans l'arbre. Placé
    //    juste sous les killers : un signal plus spécifique que l'history
    //    générique, mais moins établi qu'un killer testé À CE niveau précis.
    if let Some((prev_piece, prev_to)) = prev_key {
        if mv == countermoves.get(prev_piece, prev_to) {
            return 750_000;
        }
    }

    // 6. Coups silencieux ordonnés par l'heuristique d'historique, enrichie
    //    par la continuation history (contexte du dernier coup adverse) —
    //    simple addition, pas un palier séparé : la continuation history
    //    affine le score "history" existant plutôt que de créer une nouvelle
    //    catégorie de priorité.
    if let Some((piece, _)) = board.piece_at(mv.from) {
        let base = history.get(piece, mv.to);
        let cont = match prev_key {
            Some((prev_piece, prev_to)) => cont_history.get(prev_piece, prev_to, piece, mv.to),
            None => 0,
        };
        return base + cont;
    }

    0
}

/// Clé de hachage des pièces NON-PION (Cavalier..Roi) d'une couleur, pour
/// indexer une table de Correction History. Mélange des bitboards concernés.
/// Une collision d'index est sans conséquence de résultat : au pire une
/// correction approximative (l'éval reste l'éval, seule une marge bouge).
#[inline]
fn nonpawn_key(board: &Board, color: Color) -> u64 {
    // Multiplicateurs impairs distincts (constantes de mélange type splitmix).
    const MULT: [u64; 6] = [
        0,                      // Pion (exclu)
        0xFF51_AFD7_ED55_8CCD,  // Cavalier
        0xC4CE_B9FE_1A85_EC53,  // Fou
        0x9E37_79B9_7F4A_7C15,  // Tour
        0x2545_F491_4F6C_DD1D,  // Dame
        0x1656_67B1_9E37_79F9,  // Roi
    ];
    let c = color.index();
    let mut h = 0u64;
    // Chaque bitboard NON-PION mélangé par son multiplicateur. `.skip(1)` écarte
    // les pions (index 0) ; on couvre ainsi Cavalier..Roi (index 1 à 5).
    for (bb, &mult) in board.pieces[c].iter().zip(MULT.iter()).skip(1) {
        h ^= bb.wrapping_mul(mult);
    }
    h ^ (h >> 31)
}

/// Construit les clés de Correction History d'un nœud (calculées UNE fois, puis
/// réutilisées au site de lecture ET au site d'apprentissage). `prev_move` est
/// le coup qui a mené à ce nœud (pour la table de continuation).
#[inline]
fn corr_keys(board: &Board, prev_move: Move) -> CorrKeys {
    // Clé de structure de pions (les deux bitboards de pions mélangés).
    let wp = board.pieces[Color::White.index()][Piece::Pawn.index()];
    let bp = board.pieces[Color::Black.index()][Piece::Pawn.index()];
    let mut hp = wp.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    hp ^= bp.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    let pawn = hp ^ (hp >> 29);

    // Index de continuation : (type de pièce, case d'arrivée) du dernier coup.
    // Après make_move(prev_move), la pièce qui a bougé est SUR prev_move.to.
    let cont = if prev_move.is_null() {
        None
    } else if let Some((piece, _)) = board.piece_at(prev_move.to) {
        Some(piece.index() * 64 + prev_move.to as usize)
    } else {
        None
    };

    CorrKeys {
        stm:       board.side_to_move.index(),
        pawn,
        nonpawn_w: nonpawn_key(board, Color::White),
        nonpawn_b: nonpawn_key(board, Color::Black),
        cont,
    }
}

// order_moves() a été remplacée par un tri paresseux inliné dans alpha_beta().
// Voir commentaire "Tri paresseux par sélection" dans alpha_beta().

// =============================================================================
// Late Move Reduction — table précalculée (zéro flottant à l'exécution)
// =============================================================================

/// Retourne la réduction LMR pour (depth, move_index) via une table précalculée.
///
/// Formule logarithmique standard (Stockfish-style), calculée UNE SEULE FOIS :
///   réduction = max(1, floor(1 + ln(depth) × ln(move_index) / 2))
///
/// La table est un tableau [64][64] de i32 initialisé via OnceLock au premier appel.
/// Les indices hors-borne (depth > 63 ou move_index > 63) sont clampés à 63.
/// La ligne/colonne 0 vaut 0 (ln(0) = -∞ → pas de réduction à profondeur/indice nul).
///
/// Exemples de valeurs :
///   depth= 3, move_index= 3 → 1   (peu tardif, faible profondeur)
///   depth= 6, move_index= 6 → 2
///   depth=10, move_index=15 → 4
///   depth=15, move_index=20 → 5
///
/// Gain : élimine deux appels f64::ln() + cast par nœud interne (millions de nœuds/s).
#[inline]
fn lmr_reduction(depth: i32, move_index: usize) -> i32 {
    static LMR_TABLE: OnceLock<[[i32; 64]; 64]> = OnceLock::new();

    let table = LMR_TABLE.get_or_init(|| {
        let mut t = [[0i32; 64]; 64];
        // d=0 ou m=0 : ln(0) = -∞ → réduction 0 (pas appliqué en pratique)
        // Boucle indexée volontaire : d ET m servent à la fois d'indices ET de
        // valeurs dans la formule ln(d)·ln(m) — un itérateur serait moins clair.
        #[allow(clippy::needless_range_loop)]
        for d in 1usize..64 {
            for m in 1usize..64 {
                let r = 1.0_f64 + (d as f64).ln() * (m as f64).ln() / 2.0;
                t[d][m] = (r as i32).max(1);
            }
        }
        t
    });

    let d = (depth as usize).min(63);
    let m = move_index.min(63);
    table[d][m]
}

// =============================================================================
// Late Move Pruning — seuil de coups par profondeur
// =============================================================================

/// Profondeur maximale au-delà de laquelle le Late Move Pruning ne s'applique
/// plus jamais : au-delà, le nombre de coups légaux dépasse de toute façon
/// rarement le seuil ci-dessous, et le risque (élaguer un coup réellement bon)
/// l'emporte sur le gain marginal.
const LMP_MAX_DEPTH: i32 = 8;

/// Nombre de coups (toutes catégories confondues, mais en pratique presque
/// toujours dépassé par des coups silencieux mal classés, les captures et
/// killers étant ordonnés bien plus tôt) au-delà duquel un coup silencieux
/// restant est élagué sans recherche à cette profondeur.
///
/// Croissance quadratique : à profondeur 1, seuil très serré (peu de marge
/// d'erreur acceptable) ; à profondeur LMP_MAX_DEPTH, le seuil dépasse le
/// nombre de coups légaux dans l'immense majorité des positions réelles
/// (le record théorique est 218, mais la moyenne réelle est de l'ordre de
/// 30-40) — LMP devient alors un no-op naturel, sans cas particulier à coder.
///
/// Ajustement "improving" (RÉACTIVÉ — le bug d'origine sur `eval_history`
/// est corrigé, voir le bloc "improving" dans alpha_beta()) : le coefficient
/// quadratique vaut 2 si la position s'améliore (on accorde DAVANTAGE de coups
/// avant d'élaguer — une position en progression mérite un examen plus large),
/// 1 sinon (on élague plus tôt, faute de raison de croire qu'un coup tardif
/// sauve une position qui ne progresse pas). Même logique que le diviseur
/// `(2 - improving)` des moteurs modernes.
#[inline]
fn lmp_threshold(depth: i32, improving: bool) -> usize {
    let coeff = if improving { 2 } else { 1 };
    (4 + coeff * depth * depth) as usize
}

// =============================================================================
// Delta Pruning (futility pruning en quiescence)
// =============================================================================

/// Marge de sécurité ajoutée à l'estimation du gain matériel pour le delta
/// pruning. Couvre l'écart entre la valeur matérielle brute d'une capture et
/// l'évaluation complète de la position résultante (mobilité, structure de
/// pions, sécurité du roi, etc., non comptées ici). Valeur standard utilisée
/// par la plupart des moteurs : 150 à 200 cp. On retient 200 pour rester
/// prudent (mieux vaut explorer un coup inutile que rater un coup utile).
const DELTA_MARGIN: i32 = 200;

/// Estime le gain matériel maximal d'une capture, pour le delta pruning.
///
/// Valeur de la pièce capturée + gain de promotion éventuel (valeur de la
/// nouvelle pièce moins celle du pion). C'est une borne SUPÉRIEURE du gain
/// réel : le SEE tiendra compte des recaptures adverses, mais ici on veut
/// seulement savoir si le MEILLEUR cas possible peut atteindre alpha — une
/// borne supérieure suffit, et elle est bien moins coûteuse à calculer que
/// le SEE complet (pas de simulation de la chaîne d'échanges).
///
/// Cas particuliers :
///   - Prise en passant : la pièce capturée n'est pas sur `to` mais sur la
///     case adjacente ; on sait que c'est toujours un pion adverse.
///   - Promotion (avec ou sans capture) : on ajoute le gain net de la
///     promotion (Dame − Pion par défaut, cf. promotion_piece()).
#[inline]
fn capture_gain_estimate(board: &Board, mv: Move) -> i32 {
    let captured_value = if mv.flags == MoveFlags::EnPassant {
        piece_value(Piece::Pawn)
    } else {
        match board.piece_at(mv.to) {
            Some((p, _)) => piece_value(p),
            None => 0, // Ne devrait jamais arriver pour une capture légale.
        }
    };

    let promotion_gain = match mv.promotion_piece() {
        Some(p) => piece_value(p) - piece_value(Piece::Pawn),
        None => 0,
    };

    captured_value + promotion_gain
}

// =============================================================================
// Recherche de quiescence
// =============================================================================

/// Recherche de quiescence : continue la recherche sur les coups "bruyants"
/// (captures), plus les évasions complètes quand le camp au trait est en échec
/// (voir « Gestion de l'échec » ci-dessous). Évite l'effet horizon (s'arrêter
/// sur une position instable) — par exemple, ne pas évaluer une position où la
/// dame vient d'être prise mais où on peut la reprendre au coup suivant.
///
/// Gestion de l'échec : si le camp au trait est EN ÉCHEC à l'entrée, la
/// fonction ne fait PAS de stand-pat (illégal — on ne peut pas passer) et
/// recherche TOUTES les évasions légales (pas seulement les captures),
/// détectant aussi le mat. Voir le bloc `in_check` en tête de fonction.
/// La génération de coups silencieux DONNANT échec n'est, elle, pas faite
/// (trop coûteuse — voir la note en fin de fonction).
///
/// La profondeur depuis la racine (`ply`) sert à la borne MAX_QUIESCENCE_PLY et
/// aux scores de mat. La récursion est naturellement bornée (stand-pat + filtre
/// SEE des captures, évasions limitées en échec) ; aucun compteur de profondeur
/// propre à la quiescence n'est nécessaire.
pub fn quiescence(
    board:     &mut Board,
    mut alpha: i32,
    beta:      i32,
    ply:       usize,
    info:      &mut SearchInfo,
) -> i32 {
    if info.should_stop() {
        return evaluate_opt(board, !info.toggles.disable_king_attack);
    }

    info.nodes += 1;

    // Mettre à jour la profondeur sélective maximale atteinte (seldepth UCI).
    if ply as i32 > info.seldepth {
        info.seldepth = ply as i32;
    }

    // Borne de profondeur de sécurité.
    // Placée AVANT tout le reste pour borner AUSSI la branche "en échec"
    // ci-dessous : une suite d'échecs et d'évasions n'avance pas via des
    // captures et n'est pas détectée comme nulle par la quiescence (pas de
    // détection de répétition ici) — cette garde garantit la terminaison et
    // protège la pile dans les rares positions à échecs quasi perpétuels.
    if ply >= MAX_QUIESCENCE_PLY {
        return evaluate_opt(board, !info.toggles.disable_king_attack);
    }

    // --- Cas particulier : le camp au trait est EN ÉCHEC ---
    //
    // RÉACTIVATION — forme professionnelle et sûre de "échecs en quiescence".
    // L'ancienne tentative générait TOUS les coups légaux à CHAQUE feuille
    // pour y chercher des coups silencieux donnant échec : prohibitif (chemin
    // le plus chaud du moteur), d'où sa désactivation. On implémente ici la
    // partie réellement importante et BIEN MOINS coûteuse — la gestion correcte
    // de la position où le camp au trait est LUI-MÊME en échec.
    //
    // En échec, le "stand-pat" est illégal : on ne peut pas "passer", il faut
    // répondre à l'échec. Évaluer une telle position comme calme (et couper sur
    // stand_pat >= beta, ou ne regarder que les captures) est une ERREUR de
    // CORRECTION : la vraie valeur peut être radicalement différente, jusqu'au
    // mat. On génère donc TOUTES les évasions légales (fuite du roi,
    // interpositions, capture du donneur d'échec — pas seulement les captures)
    // et on les recherche. Coût maîtrisé : ce bloc ne se déclenche QUE sur les
    // nœuds de quiescence réellement en échec (une faible fraction du total),
    // contrairement à l'ancienne version active à chaque feuille.
    let in_check = is_in_check(board, board.side_to_move);
    if in_check {
        // Évasions générées sur la pile (zéro allocation tas).
        let mut evasions = MoveList::new();
        generate_legal_moves_into(board, &mut evasions);
        if evasions.is_empty() {
            // Aucune évasion légale → échec et mat. Score de mat ajusté à la
            // distance (mats rapides préférés), cohérent avec alpha_beta().
            return -(SCORE_MATE - ply as i32);
        }
        for &mv in evasions.iter() {
            board.make_move(mv);
            let score = -quiescence(board, -beta, -alpha, ply + 1, info);
            board.unmake_move(mv);

            if score >= beta {
                return beta;        // coupure bêta (fail-hard, comme le reste du fichier)
            }
            if score > alpha {
                alpha = score;
            }
        }
        return alpha;
    }

    // Score de la position sans jouer de coup (stand pat).
    // (À partir d'ici, le camp au trait n'est PAS en échec.)
    // Si ce score dépasse bêta, l'adversaire éviterait cette branche.
    let stand_pat = evaluate_opt(board, !info.toggles.disable_king_attack);

    if stand_pat >= beta {
        return beta;
    }

    if stand_pat > alpha {
        alpha = stand_pat;
    }

    // Générer les captures, les évaluer par SEE et trier (meilleures en premier).
    // Élagage SEE : les captures perdantes (SEE < 0) sont ignorées en quiescence.
    // Justification : en quiescence on cherche à stabiliser la position ; une
    // capture perdante aggrave la situation et ne vaut pas la peine d'être explorée.
    let mut raw_captures = MoveList::new();
    generate_legal_captures_into(board, &mut raw_captures);

    // Précalculer les scores SEE pour éviter les appels redondants (tri + filtre).
    //
    // Delta Pruning (futility en quiescence) — appliqué AVANT le calcul du SEE :
    //   Si stand_pat + gain_maximal_possible + marge ≤ alpha, ce coup ne peut
    //   structurellement pas améliorer la position, même dans le meilleur cas
    //   (capture réussie sans la moindre recapture adverse). On l'élimine sans
    //   même calculer son SEE — qui simule toute la chaîne d'échanges et coûte
    //   nettement plus cher qu'une simple lecture de piece_value().
    //   Gain attendu : moins de nœuds de quiescence explorés en fin de partie
    //   ou dans les positions très déséquilibrées, sans perte de précision
    //   (le coup éliminé ne pouvait de toute façon pas changer le résultat).
    // Captures retenues + leur score SEE, stockées sur la PILE (tableau fixe,
    // parallèle aux coups) au lieu d'un Vec<(Move,i32)> alloué sur le tas à
    // chaque nœud de quiescence — le chemin le plus chaud du moteur.
    // `n` ≤ raw_captures.len() ≤ MAX_MOVE_LIST : aucun débordement possible.
    let mut scored: [(Move, i32); MAX_MOVE_LIST] = [(Move::NULL, 0); MAX_MOVE_LIST];
    let mut n = 0usize;
    for &mv in raw_captures.iter() {
        let gain_estimate = capture_gain_estimate(board, mv);
        if stand_pat + gain_estimate + DELTA_MARGIN <= alpha {
            continue;
        }
        scored[n] = (mv, see(board, mv));
        n += 1;
    }

    // Trier par score SEE décroissant : meilleures captures en premier.
    scored[..n].sort_unstable_by_key(|entry| std::cmp::Reverse(entry.1));

    for &(mv, see_score) in scored[..n].iter() {
        // Ignorer les captures perdantes (SEE < 0).
        // Comme la tranche est triée, dès qu'on voit SEE < 0 on peut arrêter.
        if see_score < 0 {
            break;
        }

        board.make_move(mv);
        let score = -quiescence(board, -beta, -alpha, ply + 1, info);
        board.unmake_move(mv);

        if score >= beta {
            return beta;
        }
        if score > alpha {
            alpha = score;
        }
    }

    // Note : la GÉNÉRATION de coups silencieux donnant échec (le camp au trait
    // DONNE échec sans capturer) n'est volontairement PAS faite ici — c'est ce
    // qui rendait l'ancienne version prohibitive (generate_legal_moves() à
    // chaque feuille). La gestion des positions où le camp au trait SUBIT un
    // échec, elle, est traitée en tête de fonction (voir le bloc `in_check`),
    // pour un coût bien moindre et un gain de correction réel. Réintroduire la
    // génération de contre-échecs silencieux nécessiterait une détection d'échec
    // bon marché (sans regénérer tous les coups) ET un banc d'essai NPS/Elo —
    // à réserver à une itération ultérieure mesurée.

    alpha
}

// =============================================================================
// Facteur de contempt
// =============================================================================

/// Retourne le score d'une position nulle (50 coups, répétition, matériel
/// insuffisant, pat — toutes causes confondues), ajusté par le contempt
/// configuré via l'option UCI "Contempt" (`info.contempt`, 0 par défaut =
/// comportement inchangé, exactement SCORE_DRAW).
///
/// Principe : le contempt exprime "une nullité est légèrement DÉFAVORABLE
/// du point de vue du camp que le moteur joue actuellement" (utile contre
/// un adversaire plus faible — pas la peine de se contenter d'un partage
/// des points). Ce camp est TOUJOURS celui au trait à la racine de la
/// recherche en cours (ply == 0), puisque "go" n'est appelé que lorsque
/// c'est au moteur de jouer.
///
/// Dérivation de la parité : chaque niveau de recherche renvoie un score du
/// point de vue du camp au trait À CE NŒUD, puis le négamax l'inverse à
/// chaque remontée d'un ply. Le camp alterne à chaque ply, donc le nombre
/// d'inversions entre ce nœud et la racine est exactement `ply`. Pour que
/// la racine perçoive toujours `-contempt` (légèrement défavorable, quel
/// que soit l'endroit de l'arbre où la nullité est détectée) :
///   - ply pair   → ce nœud EST le camp racine → renvoyer directement `-contempt`
///     (un nombre pair d'inversions ne change pas le signe)
///   - ply impair → ce nœud est l'ADVERSAIRE du camp racine → renvoyer
///     `+contempt`, qui devient `-contempt` après l'inversion impaire
///
/// IMPORTANT — cohérence multi-thread : la table de transposition est
/// PARTAGÉE entre tous les threads Lazy SMP. Si un thread appliquait le
/// contempt et un autre non, les scores de nullité mis en cache seraient
/// incohérents selon le thread qui les a calculés. `info.contempt` doit
/// donc être identique sur TOUS les threads d'une même recherche — voir
/// SearchEngine::search() qui le copie sur le thread principal ET sur
/// chaque thread secondaire depuis la même SearchConfig.
#[inline]
fn draw_score(contempt: i32, ply: usize) -> i32 {
    if contempt == 0 {
        return SCORE_DRAW;
    }
    if ply.is_multiple_of(2) { -contempt } else { contempt }
}

// =============================================================================
// Recherche alpha-bêta principale
// =============================================================================

/// Recherche alpha-bêta avec toutes les heuristiques de Vendetta Chess Motor.
///
/// Paramètres :
///   - board         : position courante (modifiée puis restaurée)
///   - depth         : profondeur restante à explorer
///   - alpha         : borne inférieure (meilleur score assuré pour le joueur actif)
///   - beta          : borne supérieure (meilleur score assuré pour l'adversaire)
///   - ply           : distance depuis la racine (0 = racine)
///   - tt            : table de transposition partagée (mutabilité intérieure)
///   - killers       : heuristique des killer moves
///   - history       : heuristique d'historique
///   - countermoves  : heuristique countermove (coup de réfutation)
///   - prev_move     : dernier coup joué pour atteindre ce nœud (celui que
///                     countermoves cherche éventuellement à réfuter).
///                     Move::NULL à la racine ou après un coup nul (Null
///                     Move Pruning) — dans ce cas, aucun lookup countermove
///                     n'est effectué pour les enfants de ce nœud.
///   - cont_history  : continuation history (généralisation cumulative du
///                     countermove) — utilise la même clé prev_key.
///   - info          : statistiques de recherche et signal d'arrêt
///   - excluded_move : coup à exclure de la recherche.
///                     Toujours Move::NULL dans les appels normaux.
///                     Uniquement non-NULL dans la recherche de vérification
///                     de la Singular Extension.
///   - root_moves    : coups pré-filtrés pour la racine (searchmoves UCI).
///                     Vide (&[]) dans tous les appels récursifs internes.
///                     Non-vide uniquement à ply==0 depuis search() : élimine
///                     generate_legal_moves() ET les allocations to_uci() à la racine.
// Nombre d'arguments volontairement élevé : c'est la fonction de recherche
// centrale, qui propage l'état partagé (TT, heuristiques, contexte) à travers
// toute la récursion. Les regrouper dans une struct nuirait à la lisibilité des
// appels récursifs. L'alignement manuel de la doc des paramètres est, lui aussi,
// délibéré (préféré au reformatage automatique).
#[allow(clippy::too_many_arguments, clippy::doc_overindented_list_items)]
pub fn alpha_beta(
    board:         &mut Board,
    mut depth:     i32,
    mut alpha:     i32,
    mut beta:      i32,
    ply:           usize,
    tt:            &TranspositionTable,
    killers:       &mut KillerMoves,
    history:       &mut HistoryTable,
    countermoves:  &mut CountermoveTable,
    cont_history:  &mut ContinuationHistoryTable,
    prev_move:     Move,
    info:          &mut SearchInfo,
    excluded_move: Move,
    root_moves:    &[Move],
) -> i32 {
    // --- Arrêt anticipé ---
    info.check_time();
    if info.should_stop() {
        return 0;
    }

    info.nodes += 1;

    // --- Détection de position nulle ---

    // Règle des 50 coups
    if board.halfmove_clock >= 100 {
        return draw_score(info.contempt, ply);
    }

    // Nulle par répétition (via hash Zobrist)
    //
    // Correction — ordre des adaptateurs : .skip(1).step_by(2), pas l'inverse.
    //   L'ancienne version (.step_by(2).skip(1)) vérifiait les positions aux rangs
    //   3, 5, 7… demi-coups en arrière, c'est-à-dire les positions du TRAIT ADVERSE.
    //   Le hash Zobrist encode le trait → elles ne peuvent jamais correspondre
    //   au hash courant → la détection était silencieusement non-fonctionnelle.
    //
    //   Séquence correcte :
    //     .skip(1)    — ignore la position 1 demi-coup en arrière (trait adverse)
    //     .step_by(2) — de 2 en 2 : positions 2, 4, 6… demi-coups en arrière
    //                   (même trait que la position courante, garantie par le Zobrist)
    //
    // Optimisation — .any() remplace .count() >= 2.
    //   En recherche alpha-bêta, une seule répétition antérieure suffit à déclarer
    //   nulle (l'adversaire peut toujours forcer la répétition au coup suivant).
    //   .any() sort immédiatement à la première correspondance : O(1) en cas
    //   de répétition, au lieu de continuer jusqu'à halfmove_clock/2 entrées.
    if ply > 0 && board.history.len() >= 2 {
        let current_hash  = board.hash;
        let is_repetition = board.history.iter().rev()
            .skip(1)                                     // sauter la pos. 1 ply en arrière (trait adverse)
            .step_by(2)                                  // même trait que la pos. courante
            .take(board.halfmove_clock as usize / 2)     // borné par la règle des 50 coups
            .any(|s| s.hash == current_hash);            // sortie dès la 1re occurrence
        if is_repetition {
            return draw_score(info.contempt, ply);
        }
    }

    // Matériel insuffisant pour mater
    if is_insufficient_material(board) {
        return draw_score(info.contempt, ply);
    }

    // --- Sonde de la table de transposition ---
    //
    // Important : si un coup est exclu (recherche de vérification SE), on NE fait
    // PAS de cutoff TT. Le score stocké en TT a été calculé sans exclusion : il
    // tient compte du coup exclu et serait incorrect ici.
    // En revanche, on récupère quand même tt_move pour l'ordonnancement des coups.
    let tt_entry_opt = tt.probe(board.hash);
    let tt_move = match tt_entry_opt {
        Some(ref entry) => {
            if entry.depth >= depth && excluded_move.is_null() {
                let score = TranspositionTable::adjust_score_from_tt(
                    entry.score, ply as i32,
                );
                match entry.flag {
                    TTFlag::Exact => {
                        // Score exact : on peut retourner directement (sauf à la racine)
                        if ply > 0 { return score; }
                    }
                    TTFlag::LowerBound => {
                        if score >= beta { return beta; }
                    }
                    TTFlag::UpperBound => {
                        if score <= alpha { return alpha; }
                    }
                }
            }
            entry.best_move
        }
        None => Move::NULL,
    };

    // --- Mate Distance Pruning ---
    //
    // Resserre directement [alpha, beta] aux scores de mat atteignables
    // depuis CE nœud, compte tenu de sa profondeur (ply) — sans dépendre
    // d'aucune heuristique ni marge approximative, juste de l'arithmétique
    // exacte des scores de mat. Technique gratuite et sans aucun risque
    // tactique (contrairement à RFP/Razoring/LMP) : si elle coupe, c'est
    // une certitude logique, pas un pari.
    //
    //   - Meilleur cas : mater l'adversaire au coup suivant (ply+1, un coup
    //     de plus que ce nœud) → SCORE_MATE - (ply+1). Si bêta dépasse déjà
    //     cette borne, on l'abaisse : aucun coup ne peut faire mieux qu'un
    //     mat immédiat.
    //   - Pire cas : être maté À ce nœud même (ply) → -SCORE_MATE + ply. Si
    //     alpha est déjà sous cette borne, on le relève : rien de pire ne
    //     peut nous arriver ici qu'un mat immédiat contre nous.
    //   - Si la fenêtre s'effondre (alpha >= bêta) après resserrement, la
    //     position est déjà entièrement déterminée par la distance de mat
    //     seule : on retourne sans générer un seul coup.
    //
    // Placée après la sonde TT (qui peut déjà couper plus tôt dans le cas
    // général) et avant le dispatch vers la quiescence — s'applique donc
    // uniformément à TOUS les nœuds, y compris ceux sur le point de plonger
    // en profondeur 0.
    let mate_score_for_us     = SCORE_MATE - (ply as i32 + 1);
    let mate_score_against_us = -SCORE_MATE + ply as i32;
    if beta  > mate_score_for_us     { beta  = mate_score_for_us; }
    if alpha < mate_score_against_us { alpha = mate_score_against_us; }
    if alpha >= beta {
        return alpha;
    }

    // --- Nœud feuille : recherche de quiescence ---
    if depth <= 0 {
        return quiescence(board, alpha, beta, ply, info);
    }

    let in_check = is_in_check(board, board.side_to_move);

    // Clé countermove du nœud courant : (pièce, case d'arrivée) du dernier
    // coup joué pour ATTEINDRE ce nœud (déjà appliqué sur `board`). None si
    // aucun coup précédent n'est tracé (racine, ou enfant d'un coup nul).
    // board.piece_at(prev_move.to) lit l'état ACTUEL du plateau, qui reflète
    // déjà ce coup — aucune information supplémentaire à propager à part
    // le Move lui-même.
    //
    // CORRIGÉ (audit robustesse, point 2) : pour une promotion, piece_at()
    // lirait la pièce APRÈS promotion (ex: Dame) plutôt que la pièce qui a
    // réellement joué le coup (un Pion — par définition, aucune autre pièce
    // ne peut promouvoir). Cas particulier explicite, sans avoir besoin de
    // relire le plateau ni de propager une information supplémentaire.
    let prev_key: Option<(Piece, u8)> = if !prev_move.is_null() {
        if prev_move.flags.is_promotion() {
            Some((Piece::Pawn, prev_move.to))
        } else {
            board.piece_at(prev_move.to).map(|(piece, _)| (piece, prev_move.to))
        }
    } else {
        None
    };

    // Évaluation statique calculée une seule fois et partagée entre le
    // Reverse Futility Pruning et le Razoring ci-dessous (avant, evaluate()
    // était appelé une seconde fois dans le bloc Razoring — redondant).
    //
    // Désactivée (skip = None) si les conditions communes aux deux techniques
    // ne sont pas réunies : en échec, à la racine, ou en recherche SE
    // (Singular Extension — excluded_move non nul). Ces trois cas rendent
    // l'évaluation statique non pertinente ou les coupes dangereuses.
    // Clés de Correction History du nœud, calculées UNE SEULE FOIS ici puis
    // réutilisées au site d'apprentissage (en fin de fonction). None si la
    // correction n'a pas lieu d'être (mêmes conditions que static_eval_opt) ou
    // si l'interrupteur runtime la désactive (tests SPRT).
    let corr_keys_opt = if !in_check && ply > 0 && excluded_move.is_null() && !info.toggles.disable_correction {
        Some(corr_keys(board, prev_move))
    } else {
        None
    };

    let static_eval_opt = if !in_check && ply > 0 && excluded_move.is_null() {
        // Éval statique CORRIGÉE par la Correction History (⚠️ à valider SPRT) :
        // on ajoute la correction apprise (moyenne pondérée de plusieurs tables :
        // pions, pièces non-pion par couleur, continuation). TOUT l'élagage en
        // aval (RFP, Razoring, NMP, futility) et le drapeau `improving` utilisent
        // cette éval corrigée — une éval mieux calibrée affine les marges de coupe.
        let raw_eval = evaluate_opt(board, !info.toggles.disable_king_attack);
        // corr_keys_opt = None ⇒ correction désactivée (toggle) ou non pertinente :
        // l'éval reste BRUTE.
        let correction = match &corr_keys_opt {
            Some(keys) => info.correction_history.value(keys),
            None => 0,
        };
        Some(raw_eval + correction)
    } else {
        None
    };

    // --- Pile d'évaluations statiques par ply + drapeau "improving" ---
    //
    // (RÉACTIVÉ — le bug qui avait fait désactiver cette feature est corrigé,
    // voir ci-dessous.)
    //
    // "improving" répond à : la position du camp au trait est-elle MEILLEURE
    // qu'il y a 2 plies ? (Le trait alterne à chaque ply ; ply et ply-2 sont
    // donc toujours le MÊME camp au trait, comparaison d'évals directe et
    // valide.) Si oui, on fait davantage confiance aux scores : RFP coupe un
    // peu plus facilement, NMP réduit un peu plus, LMP élague un peu moins vite.
    //
    // CORRECTIF DU BUG D'ORIGINE — l'ancienne version n'écrivait
    // eval_history[ply] QUE lorsqu'une éval statique était disponible (donc
    // PAS en échec). Un nœud en échec laissait alors à cet index la valeur
    // d'une AUTRE branche explorée plus tôt au même ply ; un descendant à
    // ply+2 la lisait comme si c'était celle de son propre grand-parent →
    // décision d'élagage fondée sur une position sans rapport.
    //
    // Le correctif : écrire eval_history[ply] à CHAQUE visite réelle du nœud,
    // INCONDITIONNELLEMENT — la vraie éval si disponible, sinon la sentinelle
    // EVAL_HISTORY_NONE (en échec, racine). Invariant alors garanti : pendant
    // l'exploration du sous-arbre d'un nœud à ply P, eval_history[P] contient
    // toujours exactement ce que CE nœud y a écrit. (Ses descendants n'écrivent
    // qu'aux index >= P+1 ; ses frères déjà explorés sont retournés, et leurs
    // écritures à des index >= P+1 n'altèrent jamais l'index P.) Donc lire
    // eval_history[ply-2] renvoie TOUJOURS l'éval de l'ancêtre situé 2 plies
    // plus haut sur LE chemin courant — jamais celle d'une autre branche.
    // Quand cet ancêtre était en échec (sentinelle), improving = false (réglage
    // le plus prudent). C'est la technique des moteurs modernes (la pile
    // ss->staticEval de Stockfish).
    //
    // EXCEPTION — recherche de Singular Extension : elle rappelle alpha_beta()
    // au MÊME ply (aucun coup joué) avec excluded_move non nul. Si elle écrivait
    // eval_history[ply], elle écraserait la valeur que le nœud ENGLOBANT y a
    // légitimement placée, corrompant l'index pour la suite de l'exploration
    // RÉELLE de ce nœud. On n'écrit donc QUE lorsque excluded_move est nul
    // (visite réelle). La recherche SE porte de toute façon sur la même
    // position : ses descendants lisant eval_history[ply] y trouvent la bonne
    // valeur, déjà en place.
    if excluded_move.is_null() && ply < MAX_PLY {
        info.eval_history[ply] = static_eval_opt.unwrap_or(EVAL_HISTORY_NONE);
    }

    let improving = if info.toggles.disable_improving {
        // Interrupteur runtime (tests SPRT, voir bin/selfplay.rs) : `improving`
        // forcé à false. Aucun effet en jeu normal (disable_improving = false).
        false
    } else {
        match static_eval_opt {
            Some(current_eval) if (2..MAX_PLY).contains(&ply) => {
                let prev = info.eval_history[ply - 2];
                prev != EVAL_HISTORY_NONE && current_eval > prev
            }
            _ => false,
        }
    };

    // --- Reverse Futility Pruning (Static Null Move) ---
    // Si l'évaluation statique dépasse déjà largement bêta, l'adversaire ne
    // laisserait jamais la partie atteindre cette position : on coupe sans
    // même tenter le coup nul. C'est le symétrique du Razoring ci-dessous —
    // celui-ci coupe côté bêta (position trop bonne), Razoring coupe côté
    // alpha (position trop mauvaise).
    //
    // Désactivé en fenêtre nulle non pertinent ici contrairement à Razoring :
    // RFP profite justement le plus des nœuds non-PV (fenêtre nulle), qui
    // sont les plus nombreux dans l'arbre PVS.
    //
    // Marge : 120 centipawns par ply de profondeur restante — une position
    // jugée à +120*depth au-dessus de bêta a très peu de chances d'être
    // retournée par une recherche plus profonde à cette profondeur.
    //
    // Ajustement "improving" : si la position s'améliore (voir plus haut),
    // la marge est réduite de 120 cp (depth - 1 au lieu de depth) — on fait
    // davantage confiance à un score déjà bon quand la tendance le confirme,
    // donc on coupe plus facilement. Si la position NE s'améliore PAS,
    // la marge complète reste en vigueur (plus prudent, coupe moins souvent).
    const RFP_MAX_DEPTH:        i32 = 6;
    const RFP_MARGIN_PER_DEPTH: i32 = 120;

    if let Some(static_eval) = static_eval_opt {
        let rfp_margin = RFP_MARGIN_PER_DEPTH * (depth - improving as i32);
        if depth <= RFP_MAX_DEPTH
            && static_eval - rfp_margin >= beta
            && static_eval.abs() < SCORE_MATE - 200
        {
            // Fail-hard : on retourne `beta`, pas le score brut, par cohérence
            // avec toutes les autres coupures de ce fichier (Null Move Pruning,
            // stand-pat en quiescence, etc.).
            return beta;
        }
    }

    // --- Razoring ---
    // (anciennement nommé "Futility Pruning" dans ce fichier — corrigé :
    // la terminologie standard appelle "Razoring" la coupe au NIVEAU DU
    // NŒUD basée sur alpha, et réserve "Futility Pruning" à une coupe PAR
    // COUP à l'intérieur de la boucle des coups silencieux. Ce fichier
    // n'implémente que la version "nœud" — Razoring est donc le nom exact.)
    //
    // Aux profondeurs 1-2, si l'évaluation statique + une marge de sécurité
    // est encore sous alpha, les coups silencieux ne peuvent pas sauver la
    // position : on plonge directement en quiescence.
    //
    // Désactivé si :
    //   - En échec, racine, ou recherche SE (cf. static_eval_opt ci-dessus)
    //   - Fenêtre nulle (alpha == beta - 1) : sécurité supplémentaire,
    //     contrairement au RFP qui lui profite des fenêtres nulles
    //   - Score proche d'un mat
    if let Some(static_eval) = static_eval_opt {
        if depth <= 2
            && alpha != beta - 1
        {
            let razoring_margin = 150 * depth;

            if static_eval + razoring_margin <= alpha
                && static_eval.abs() < SCORE_MATE - 200
            {
                return quiescence(board, alpha, beta, ply, info);
            }
        }
    }

    // --- Null Move Pruning ---
    // On passe son tour : si le score dépasse quand même bêta, la position est
    // trop bonne pour l'adversaire → coupure sans explorer.
    //
    // Ajustement "improving" : réduction R=4 si la position s'améliore (plus
    // agressif — la dynamique récente rend un piège de zugzwang moins
    // probable), R=3 sinon (réglage d'origine, plus prudent). `.max(0)` au
    // cas où R=4 ramènerait la profondeur de l'enfant sous zéro (possible à
    // depth==3 avec R=4 : 3-4=-1) — la garde existante `depth <= 0 →
    // quiescence` au sommet de la fonction gère 0 sans problème, `.max(0)`
    // évite simplement de transmettre une profondeur négative inutilement.
    //
    // Désactivé si :
    //   - En échec
    //   - Profondeur < 3
    //   - Racine
    //   - Recherche SE : le coup nul interagit mal avec l'exclusion
    //   - Roi + pions seulement (zugzwang possible)
    if !in_check
        && depth >= 3
        && ply > 0
        && excluded_move.is_null()
    {
        let side     = board.side_to_move;
        let non_pawn = board.occupancy[side.index()]
            & !board.pieces[side.index()][Piece::Pawn.index()]
            & !board.pieces[side.index()][Piece::King.index()];

        if non_pawn != 0 {
            let nmp_reduction = if improving { 4 } else { 3 };
            let null_depth    = (depth - nmp_reduction).max(0);
            let prev_ep    = board.make_null_move();
            let null_score = -alpha_beta(
                board,
                null_depth,
                -beta,
                -beta + 1,
                ply + 1,
                tt, killers, history, countermoves, cont_history,
                Move::NULL, // pas de coup réel à tracer après un coup nul
                info,
                Move::NULL,
                &[],
            );
            board.unmake_null_move(prev_ep);

            if null_score >= beta {
                return beta;
            }
        }
    }

    // --- Singular Extension ---
    //
    // Un coup est "singulier" si c'est le seul qui maintient le score.
    // Mécanisme :
    //   1. On prend le coup TT (meilleur coup connu à cette position).
    //   2. On lance une recherche à profondeur réduite en l'EXCLUANT.
    //   3. Si tous les autres coups échouent sous (tt_score - marge) :
    //      → Le coup TT est singulier, on l'étend de +1 dans la boucle principale.
    //   4. Si même sans le coup TT le score dépasse bêta (multi-cut) :
    //      → Il existe plusieurs bons coups, on peut couper directement.
    //
    // Conditions d'activation (toutes requises) :
    //   depth >= 6   : SE est coûteux (~50% de nœuds en plus), inutile en bas
    //   ply > 0      : jamais à la racine
    //   excluded_move.is_null() : jamais dans une recherche SE imbriquée
    //   !in_check    : la Check Extension gère déjà les positions d'échec
    //   tt_move non nul + entrée TT fiable (profondeur et flag)
    //   score TT loin d'un mat
    let mut singular_extension = 0i32;

    if depth >= 6
        && ply > 0
        && excluded_move.is_null()
        && !in_check
        && !tt_move.is_null()
    {
        if let Some(ref entry) = tt_entry_opt {
            // L'entrée TT doit être assez profonde pour être fiable.
            // UpperBound → score TT peut être surestimé → non fiable pour SE.
            if entry.depth >= depth - 3 && entry.flag != TTFlag::UpperBound {
                let tt_score = TranspositionTable::adjust_score_from_tt(
                    entry.score, ply as i32,
                );

                // Ne pas appliquer SE près d'un score de mat
                if tt_score.abs() < SCORE_MATE - 200 {
                    // Marge conservatrice : 2 cp × profondeur.
                    // Trop petite → trop d'extensions → explosion du temps.
                    // Trop grande → trop peu d'extensions → SE inutile.
                    let se_margin = 2 * depth;

                    // se_beta : plancher que les autres coups doivent franchir
                    // pour infirmer la singularité du coup TT.
                    let se_beta  = (tt_score - se_margin).max(-SCORE_MATE + 1);

                    // Profondeur de vérification : environ la moitié.
                    // Suffisante pour détecter la singularité sans coût excessif.
                    let se_depth = (depth - 1) / 2;

                    // Recherche de vérification.
                    // La position sur le plateau N'EST PAS modifiée (aucun make_move).
                    // Le coup TT est passé comme excluded_move → il sera sauté.
                    let se_score = alpha_beta(
                        board,
                        se_depth,
                        se_beta - 1,  // Fenêtre nulle : [se_beta-1, se_beta]
                        se_beta,
                        ply,          // Même ply : aucun coup n'a été joué
                        tt, killers, history, countermoves, cont_history,
                        prev_move,    // Aucun coup joué ici : même contexte que le nœud courant
                        info,
                        tt_move,      // ← Exclusion du coup TT
                        &[],
                    );

                    if !info.should_stop() {
                        if se_score < se_beta {
                            // Singulier confirmé : le coup TT est le seul bon coup
                            // dans cette position → on l'étendra de +1 dans la boucle.
                            singular_extension = 1;
                        } else if se_score >= beta {
                            // Multi-cut : même sans le coup TT, le score dépasse bêta.
                            // Il y a donc plusieurs bons coups → on peut couper.
                            return beta;
                        }
                    }
                }
            }
        }
    }

    // --- Internal Iterative Reduction (IIR) ---
    //
    // Si la TT n'a AUCUN coup pour ce nœud (jamais visité, ou visité à une
    // profondeur insuffisante pour avoir stocké un coup utile), c'est le
    // signe que cette branche est peu explorée — on réduit la profondeur
    // d'1 avant de continuer, plutôt que de la traiter avec la même
    // confiance qu'un nœud déjà bien documenté par la TT.
    //
    // Remplace l'ancienne Internal Iterative Deepening (IID) — qui lançait
    // une recherche à profondeur réduite UNIQUEMENT pour deviner un bon coup
    // avant de continuer (coût : un appel récursif supplémentaire). L'IIR
    // n'effectue AUCUN appel récursif : une simple soustraction conditionnelle
    // sur `depth`, qui se propage ensuite naturellement à tout le reste du
    // traitement de ce nœud (boucle de coups, profondeurs des enfants,
    // stockage TT en fin de fonction). C'est la version utilisée par les
    // moteurs modernes (dont Stockfish), bien moins coûteuse que l'IID
    // classique pour un effet comparable.
    //
    // Désactivé si :
    //   - Coup TT présent (tt_move non nul) : rien à corriger, le nœud est
    //     déjà bien renseigné
    //   - depth < IIR_MIN_DEPTH : la réduction n'a plus d'intérêt à très
    //     faible profondeur (la quiescence ou les autres pruning gèrent déjà ça)
    //   - Recherche SE (excluded_move non nul) : ne pas perturber la
    //     vérification de singularité avec une profondeur déjà réduite par
    //     autre chose qu'elle-même
    const IIR_MIN_DEPTH: i32 = 4;
    if tt_move.is_null() && depth >= IIR_MIN_DEPTH && excluded_move.is_null() {
        depth -= 1;
    }

    // --- Génération des coups légaux ---
    //
    // Racine avec searchmoves pré-filtrés (root_moves non-vide) :
    //   On utilise directement la liste préparée par search().
    //   Avantages vs l'ancien filtre interne :
    //     - Zéro appel generate_legal_moves() à la racine.
    //     - Zéro allocation de String (to_uci() fait partie du passé).
    //     - Le filtre est appliqué UNE SEULE FOIS avant l'itération en profondeur.
    //     - SearchInfo n'a plus de Vec<String> heap-alloué propagé à chaque nœud.
    //
    //   excluded_move est toujours NULL à ply==0 (SE n'est actif qu'à ply>0)
    //   → pas de retain() nécessaire dans ce cas.
    //
    // Appels récursifs (root_moves vide) : génération normale + exclusion SE.
    // Liste de coups allouée sur la PILE (MoveList) — aucune allocation tas par
    // nœud, contrairement à l'ancien Vec<Move>. Indexation, len(), iter(),
    // swap() et slicing fonctionnent via Deref vers [Move].
    let mut moves = MoveList::new();
    if ply == 0 && !root_moves.is_empty() {
        for &mv in root_moves {
            moves.push(mv);
        }
    } else {
        generate_legal_moves_into(board, &mut moves);
        if !excluded_move.is_null() {
            moves.retain(|mv| *mv != excluded_move);
        }
    }

    // --- Fin de partie ---
    if moves.is_empty() {
        if in_check {
            // Échec et mat : préférer les mats rapides (ply petit)
            return -(SCORE_MATE - ply as i32);
        } else {
            // Pat
            return draw_score(info.contempt, ply);
        }
    }

    // --- Tri paresseux par sélection (lazy selection sort) ---
    //
    // Principe : au lieu de trier intégralement N coups en O(N log N),
    //   on calcule tous les scores en une passe O(N) puis on sélectionne
    //   le meilleur coup restant à chaque itération par un balayage linéaire.
    //
    //   Coût total : O(N) scores + O(k × N) sélections pour k coups examinés.
    //
    //   Gain vs tri complet O(N log N + N × traitement) :
    //     Si la coupure bêta arrive au k-ième coup (k ≪ N), on économise
    //     O(N log N − k × N) opérations. Avec un taux de coupure élevé
    //     (coup TT / killer en tête), k vaut typiquement 1–3 sur la plupart
    //     des nœuds internes → économie substantielle sur l'arbre entier.
    //
    //   Cas défavorable : k = N (aucune coupure) → O(N²) au lieu de O(N log N).
    //   En pratique rarissime aux profondeurs élevées grâce au mouvement TT.
    debug_assert!(moves.len() <= MAX_MOVES,
        "alpha_beta: {} coups dépasse MAX_MOVES={}", moves.len(), MAX_MOVES);
    let move_count = moves.len();
    let mut scores = [0i32; MAX_MOVES];
    for (i, &mv) in moves.iter().enumerate() {
        scores[i] = move_score(board, mv, tt_move, killers, history, countermoves, cont_history, prev_key, ply);
    }

    // BUG ÉVITÉ (audit robustesse post-LMP) : un coup élagué par Late Move
    // Pruning n'est JAMAIS réellement recherché — il ne "perd" pas, il n'est
    // simplement pas examiné. Sans ce suivi, history.update_bad() ci-dessous
    // (déclenché par une coupure bêta sur un coup ultérieur) pénaliserait
    // aussi les coups élagués comme s'ils avaient été essayés et avaient
    // échoué, alors qu'ils n'ont jamais été soumis à une recherche. Dégrade
    // silencieusement la qualité de l'history heuristic sans jamais planter
    // — donc jamais détecté par les tests perft/benchmark existants.
    let mut lmp_pruned = [false; MAX_MOVES];

    // --- Exploration des coups ---
    let mut best_score = -SCORE_INF;
    let mut best_move  = Move::NULL; // mis à jour dès le premier coup examiné
    let mut tt_flag    = TTFlag::UpperBound;

    for move_index in 0..move_count {
        // Sélection du meilleur coup restant : balayage O(N − move_index).
        // On amène le coup au score maximum en position `move_index` par échange,
        // de sorte que moves[0..=move_index] contienne toujours les coups examinés
        // par ordre de score décroissant (utile pour history.update_bad ci-dessous).
        let best_idx = {
            let mut b = move_index;
            for j in (move_index + 1)..move_count {
                if scores[j] > scores[b] { b = j; }
            }
            b
        };
        moves.swap(move_index, best_idx);
        scores.swap(move_index, best_idx);

        // mv est une copie (Move implémente Copy) — pas de déréférencement requis.
        let mv = moves[move_index];

        // --- info currmove / currmovenumber (UCI, racine uniquement) ---
        // Retour visuel pur pour la GUI ("en cours d'analyse : tel coup, n-ième
        // sur N") — aucun effet sur la recherche elle-même. Émis uniquement à
        // ply == 0 (jamais dans la boucle chaude des nœuds internes) ET
        // uniquement si info.show_currmove est actif.
        //
        // BUG CORRIGÉ : la première version imprimait sans condition dès que
        // ply == 0, ce qui polluait la sortie de TOUT appelant de alpha_beta()
        // — y compris src/bin/benchmark.rs, qui appelle alpha_beta() directement
        // (hors couche UCI) pour mesurer le NPS brut. Le garde-fou
        // show_currmove (false par défaut, activé uniquement par
        // SearchEngine::search(), la vraie recherche pilotée par l'UCI) isole
        // proprement ce comportement spécifique à l'UCI du reste des usages
        // de alpha_beta() dans le projet.
        if ply == 0 && info.show_currmove {
            println!("info currmove {} currmovenumber {}", mv.to_uci(), move_index + 1);
        }

        board.make_move(mv);

        // Préchargement TT : board.hash est désormais le hash de la position
        // ENFANT, que la descente récursive va sonder en tout premier. On lance
        // le chargement de la ligne de cache MAINTENANT pour qu'elle soit chaude
        // au moment du probe() de l'enfant — la latence mémoire est masquée par
        // le calcul d'extension / LMP qui suit. Pure vitesse, zéro effet de bord.
        tt.prefetch(board.hash);

        // --- Calcul de l'extension pour ce coup ---
        //
        // Check Extension : le coup met l'adversaire en échec → position critique.
        //   Conditions cumulées :
        //     1. gives_check      : le coup met effectivement en échec
        //     2. depth <= 4       : utile uniquement en faible profondeur restante
        //     3. ply + 1 < MAX_PLY : borne de sécurité CRITIQUE contre la récursion infinie.
        //
        //   Sans la condition (3), si depth == 4 et extension == 1 :
        //     depth_enfant = 4 - 1 + 1 = 4  → la profondeur NE DÉCROÎT PAS.
        //   Toute séquence de coups donnant chacun échec crée une récursion sans fin.
        //   Au-delà de MAX_PLY plies, l'extension est désactivée ; depth passe à 3,
        //   puis 2, puis 1, puis 0 → quiescence → terminaison garantie.
        //
        // Singular Extension : ce coup est le seul bon dans la position.
        //   Ne s'applique QU'AU coup TT (celui dont la singularité a été vérifiée).
        //   Les deux extensions sont mutuellement exclusives par construction
        //   (Check Extension prend la priorité si le coup donne aussi échec).
        //
        //   BUG CORRIGÉ (audit post-session) : cette extension souffrait du même
        //   défaut que la Check Extension avant sa correction — elle ajoutait +1
        //   à la profondeur de l'enfant (depth_enfant = depth-1+1 = depth) SANS
        //   la borne `ply + 1 < MAX_PLY`. Comme la Singular Extension se déclenche
        //   à depth >= 6, l'enfant restait à depth >= 6 et pouvait, en théorie,
        //   être à son tour jugé singulier au nœud suivant — répétant le phénomène
        //   sans que la profondeur ne décroisse jamais. Contrairement aux échecs
        //   perpétuels (qui finissent par répéter une position, détectée par la
        //   règle de nulle), une chaîne de positions "singulières" n'a aucune
        //   raison de répéter le plateau : rien d'autre n'aurait arrêté la
        //   récursion, avec un risque de dépassement de pile. Même garde que la
        //   Check Extension, appliquée ici par cohérence et par sécurité.
        let gives_check = is_in_check(board, board.side_to_move);
        let extension   = if gives_check && depth <= 4 && ply + 1 < MAX_PLY {
            1  // Check Extension
        } else if mv == tt_move && singular_extension > 0 && ply + 1 < MAX_PLY {
            singular_extension  // Singular Extension (+1)
        } else {
            0
        };

        // --- Late Move Pruning (LMP) ---
        //
        // Aux profondeurs faibles, un coup silencieux qui arrive très tard
        // dans l'ordre (beaucoup de coups mieux classés l'ont déjà précédé)
        // a une probabilité si faible d'améliorer alpha que rechercher tout
        // son sous-arbre n'est presque jamais rentable. Contrairement au LMR
        // qui réduit seulement la profondeur de la sonde, ici on ne recherche
        // PAS DU TOUT ce coup à cette profondeur — gain maximal, mais aussi
        // le pruning le plus agressif de ce fichier.
        //
        // Sûr UNIQUEMENT parce que le Countermove Heuristic est désormais en
        // place : l'ordre des coups doit être fiable pour qu'un coup "tardif"
        // soit vraiment un mauvais candidat plutôt qu'un bon coup mal classé.
        // Implémenté APRÈS le Countermove Heuristic dans ce projet, pas avant,
        // précisément pour cette raison.
        //
        // Désactivé si :
        //   - Nœud courant en échec (in_check) : peu de coups légaux, souvent
        //     tous tactiques — jamais de LMP dans ces positions
        //   - Le coup donne échec (gives_check) : position critique
        //   - Le coup a reçu une extension (Check ou Singular) : on vient
        //     juste de décider qu'il méritait UNE PROFONDEUR DE PLUS, le
        //     pruner ici serait contradictoire
        //   - Capture ou promotion : déjà bien ordonnées par SEE, jamais prunées
        //   - Killer move ou countermove (CORRIGÉ — audit robustesse) : ce
        //     sont précisément les coups dont on a la PREUVE qu'ils ont été
        //     efficaces ailleurs dans l'arbre (killer) ou contre ce type de
        //     coup adverse (countermove). Sans cette exemption, un coup avec
        //     un historique de réussite pouvait être sauté simplement parce
        //     que plusieurs captures gagnantes le précédaient dans le tri —
        //     contraire à la pratique standard, qui protège toujours ces
        //     deux catégories du Late Move Pruning.
        //   - Profondeur > LMP_MAX_DEPTH : marge de sécurité, devient un no-op
        //     naturel au-delà (voir commentaire de lmp_threshold)
        let is_killer_move = killers.is_killer(mv, ply);
        let is_countermove = prev_key.is_some_and(|(p, t)| countermoves.get(p, t) == mv);

        if !in_check
            && !gives_check
            && extension == 0
            && depth <= LMP_MAX_DEPTH
            && !mv.flags.is_capture()
            && !mv.flags.is_promotion()
            && !is_killer_move
            && !is_countermove
            && move_index >= lmp_threshold(depth, improving)
        {
            lmp_pruned[move_index] = true;
            board.unmake_move(mv);
            continue;
        }

        // --- Futility Pruning (par coup) ---
        //
        // ⚠️ HEURISTIQUE À VALIDER PAR MATCH SPRT avant d'être considérée comme
        // acquise (les marges ci-dessous sont un point de départ CONSERVATEUR, à
        // régler par test A/B — voir la méthode des tests d'Elo). Désactivable
        // à l'exécution via `info.toggles.disable_futility` (utilisé par le binaire
        // selfplay pour les matchs SPRT, clés futility_a / futility_b).
        //
        // Idée : près des feuilles, un coup SILENCIEUX qui ne donne pas échec ne
        // peut quasiment pas remonter alpha si l'évaluation statique du nœud,
        // augmentée d'une marge, reste déjà sous alpha. On le saute sans le
        // rechercher. Complémentaire des deux autres élagages déjà présents :
        //   - Razoring  : coupe au NIVEAU DU NŒUD (avant la boucle de coups) ;
        //   - LMP       : coupe sur le NOMBRE de coups déjà examinés ;
        //   - Futility  : coupe COUP PAR COUP, sur le SCORE statique vs alpha.
        //
        // Conditions (toutes requises, calquées sur la LMP pour la sûreté) :
        //   - static_eval disponible (donc hors échec, hors racine, hors SE) ;
        //   - move_index > 0 : on garde TOUJOURS le coup principal ;
        //   - extension == 0 : ne jamais pruner un coup qu'on vient d'étendre ;
        //   - depth faible : la marge ne couvre le risque qu'à faible profondeur ;
        //   - coup silencieux ne donnant pas échec ;
        //   - ni killer ni countermove (efficacité déjà prouvée ailleurs) ;
        //   - alpha hors zone de mat (par sécurité) ;
        //   - static_eval + marge <= alpha.
        const FUTILITY_MAX_DEPTH:        i32 = 6;
        const FUTILITY_MARGIN_PER_DEPTH: i32 = 100;

        if let Some(static_eval) = static_eval_opt {
            if !info.toggles.disable_futility
                && move_index > 0
                && extension == 0
                && depth <= FUTILITY_MAX_DEPTH
                && !gives_check
                && !mv.flags.is_capture()
                && !mv.flags.is_promotion()
                && !is_killer_move
                && !is_countermove
                && alpha < SCORE_MATE - 200
                && static_eval + FUTILITY_MARGIN_PER_DEPTH * depth <= alpha
            {
                // Non recherché → exclu de history.update_bad (comme la LMP),
                // via le même marquage lmp_pruned[].
                lmp_pruned[move_index] = true;
                board.unmake_move(mv);
                continue;
            }
        }

        // --- Principal Variation Search (PVS) + Late Move Reduction (LMR) ---
        //
        // Coup n°1 (move_index == 0) : c'est le coup le mieux classé par
        // l'ordonnancement (TT move, ou meilleure capture/killer/history à
        // défaut). On le recherche directement à PLEINE FENÊTRE [alpha, beta]
        // pour obtenir un score exact qui servira de référence aux coups
        // suivants.
        //
        // Coups suivants (move_index > 0) : recherche PVS en 3 étapes.
        //   1. Sonde à FENÊTRE NULLE [-alpha-1, -alpha] — éventuellement à
        //      profondeur réduite si les critères LMR sont remplis (coup
        //      tardif, silencieux, profondeur suffisante). Cette sonde ne
        //      répond qu'à la question "ce coup dépasse-t-il alpha ?" — elle
        //      coupe beaucoup plus tôt qu'une fenêtre complète et c'est ce
        //      qui fait tout le gain du PVS (10-20 % de nœuds en moins).
        //   2. Si une réduction LMR a été appliquée ET que la sonde dépasse
        //      alpha : on ne sait pas encore si c'est un vrai bon coup ou un
        //      faux positif dû à la réduction. On reconfirme à PLEINE
        //      PROFONDEUR mais toujours à fenêtre nulle, avant d'envisager
        //      la recherche la plus coûteuse (étape 3).
        //   3. Si le coup dépasse réellement alpha (et reste sous beta) :
        //      il appartient potentiellement à la variante principale. Seule
        //      cette situation justifie une re-recherche à PLEINE FENÊTRE et
        //      pleine profondeur, pour obtenir son score exact.
        //
        // Dans l'immense majorité des nœuds, l'étape 1 suffit (le coup ne
        // dépasse pas alpha) : c'est précisément là que réside le gain PVS.
        let score = if move_index == 0 {
            // Coup principal : toujours pleine fenêtre, jamais de réduction.
            -alpha_beta(
                board,
                depth - 1 + extension,
                -beta,
                -alpha,
                ply + 1,
                tt, killers, history, countermoves, cont_history,
                mv, // mv vient d'être joué : c'est le prev_move de l'enfant
                info,
                Move::NULL,
                &[],
            )
        } else {
            // Critères LMR : coup tardif, silencieux, profondeur suffisante,
            // ni en échec ni donnant échec (ces positions sont trop critiques
            // pour être réduites).
            let do_lmr = depth >= 3
                && move_index >= 3
                && !in_check
                && !gives_check
                && !mv.flags.is_capture()
                && !mv.flags.is_promotion();

            // Profondeur "normale" pour ce coup — STRICTEMENT identique à celle
            // utilisée par le coup n°1 (move_index == 0). C'est la référence de
            // comparaison : tous les coups d'un même nœud doivent être mesurés
            // à la même profondeur pour que leurs scores soient comparables.
            let full_depth = depth - 1 + extension;

            // BUG CORRIGÉ : la version précédente appliquait `.max(1)` à TOUS
            // les coups non-PV, même quand do_lmr == false (reduction == 0).
            // Or quand full_depth == 0 (très fréquent : cela se produit à
            // CHAQUE nœud de l'arbre entier où il reste exactement 1 ply à
            // explorer, à chaque itération d'iterative deepening), le `.max(1)`
            // forçait ces coups à être recherchés à profondeur 1 — c'est-à-dire
            // UN PLY DE PLUS que le coup n°1, qui lui plongeait directement en
            // quiescence à profondeur 0. Cette incohérence systématique rendait
            // les comparaisons de scores invalides à travers tout l'arbre :
            // le coup n°1 (souvent un coup générique en début de liste, par
            // exemple une poussée de pion de bord) semblait artificiellement
            // sûr car évalué moins profondément, tandis que les coups réellement
            // pertinents étaient pénalisés par une analyse plus poussée révélant
            // leurs inconvénients réels. Résultat observé : le moteur jouait
            // systématiquement des coups sans intérêt (pions de bord) en début
            // de partie, signe que la recherche ne comparait plus correctement
            // les coups entre eux.
            //
            // Correction : le `.max(1)` (et la réduction elle-même) ne
            // s'appliquent désormais QUE si do_lmr est vrai. Si do_lmr est
            // faux, probe_depth == full_depth, EXACTEMENT comme le coup n°1.
            let (probe_depth, reduced) = if do_lmr {
                // Réduction de base : table logarithmique (depth × move_index).
                let mut r = lmr_reduction(depth, move_index);

                // --- LMR enrichi (ajustements ⚠️ À VALIDER PAR SPRT) ---
                // Signaux qui ne sont PAS déjà captés par le rang du coup
                // (move_index reflète déjà l'ordonnancement par history) :
                //   - position qui NE s'améliore PAS → réduire un peu PLUS
                //     (un coup tardif a encore moins de chances d'aider) ;
                //   - nœud PV (fenêtre [alpha,beta] large, non nulle) → réduire
                //     un peu MOINS (plus de précision sur la variante principale) ;
                //   - killer ou countermove → réduire un peu MOINS (coup
                //     silencieux dont l'efficacité est déjà prouvée ailleurs).
                // Ajustements volontairement petits (±1) et bornés (r ≥ 1).
                // Désactivables à l'exécution via `info.toggles.disable_lmr_tweaks`
                // (binaire selfplay, clés lmr_a / lmr_b pour les matchs SPRT) :
                // quand actif, on garde la réduction de BASE seule, sans les
                // ajustements d'enrichissement — c'est exactement ce qu'on teste.
                if !info.toggles.disable_lmr_tweaks {
                    let is_pv = beta - alpha > 1;
                    if !improving                       { r += 1; }
                    if is_pv                            { r -= 1; }
                    if is_killer_move || is_countermove { r -= 1; }
                }
                r = r.max(1); // au moins 1 de réduction quand la LMR s'applique

                ((full_depth - r).max(1), true)
            } else {
                (full_depth, false)
            };

            // --- Étape 1 : sonde à fenêtre nulle (profondeur réduite si LMR) ---
            let mut s = -alpha_beta(
                board,
                probe_depth,
                -alpha - 1,
                -alpha,
                ply + 1,
                tt, killers, history, countermoves, cont_history,
                mv,
                info,
                Move::NULL,
                &[],
            );

            // --- Étape 2 : reconfirmation à pleine profondeur (fenêtre nulle) ---
            // Uniquement si une réduction a été appliquée ET que la sonde a
            // dépassé alpha — sinon cette étape est inutile (sans réduction,
            // l'étape 1 était déjà à full_depth, identique au coup n°1).
            if reduced && s > alpha {
                s = -alpha_beta(
                    board,
                    full_depth,
                    -alpha - 1,
                    -alpha,
                    ply + 1,
                    tt, killers, history, countermoves, cont_history,
                    mv,
                    info,
                    Move::NULL,
                    &[],
                );
            }

            // --- Étape 3 : re-recherche à pleine fenêtre (vraie PV) ---
            // Le coup dépasse réellement alpha et reste sous beta : il fait
            // partie de la variante principale, on a besoin de son score exact.
            if s > alpha && s < beta {
                s = -alpha_beta(
                    board,
                    full_depth,
                    -beta,
                    -alpha,
                    ply + 1,
                    tt, killers, history, countermoves, cont_history,
                    mv,
                    info,
                    Move::NULL,
                    &[],
                );
            }

            s
        };

        board.unmake_move(mv);

        if info.should_stop() {
            return 0;
        }

        if score > best_score {
            best_score = score;
            best_move  = mv;

            if score > alpha {
                alpha   = score;
                tt_flag = TTFlag::Exact;
                info.update_best_move(mv, score, ply);
            }
        }

        // --- Coupure bêta ---
        if score >= beta {
            // Mémoriser le killer move et mettre à jour l'historique.
            // moves[..move_index] contient les coups examinés avant mv,
            // dans l'ordre du tri paresseux (du meilleur score au pire) —
            // SAUF ceux marqués lmp_pruned[i] : ces coups n'ont jamais été
            // réellement recherchés (Late Move Pruning les a sautés), ils ne
            // doivent donc pas être traités comme des échecs par
            // history.update_bad() (voir le commentaire au-dessus de la
            // déclaration de lmp_pruned, plus haut dans cette fonction).
            if !mv.flags.is_capture() {
                killers.store(mv, ply);
                history.update_good(board, mv, depth);
                if let Some((prev_piece, prev_to)) = prev_key {
                    countermoves.store(prev_piece, prev_to, mv);
                    cont_history.update_good(prev_piece, prev_to, board, mv, depth);
                }
                for (i, prev_mv) in moves[..move_index].iter().enumerate() {
                    if lmp_pruned[i] { continue; }
                    if !prev_mv.flags.is_capture() {
                        history.update_bad(board, *prev_mv, depth);
                        if let Some((prev_piece, prev_to)) = prev_key {
                            cont_history.update_bad(prev_piece, prev_to, board, *prev_mv, depth);
                        }
                    }
                }
            }

            // Stocker en TT comme borne inférieure
            let tt_score = TranspositionTable::adjust_score_for_tt(beta, ply as i32);
            tt.store(board.hash, tt_score, depth, TTFlag::LowerBound, best_move);

            return beta;
        }
    }

    // --- Mise à jour de la Correction History (⚠️ à valider SPRT) ---
    // On apprend l'écart entre le score de recherche et l'éval statique CORRIGÉE
    // du nœud. `board` est revenu à l'état du nœud (tous les coups annulés) → les
    // clés `corr_keys_opt` calculées en haut de fonction sont encore valides.
    // L'apprentissage est PONDÉRÉ PAR `depth` (recherche profonde = plus fiable).
    // Conditions : correction active (corr_keys_opt est Some, ce qui couvre déjà
    // hors-échec / ply>0 / hors-SE / toggle), meilleur coup silencieux (éval
    // positionnelle pertinente), score hors zone de mat. Version simplifiée : les
    // nœuds à coupure bêta (qui retournent plus haut) ne sont pas mis à jour.
    if let (Some(keys), Some(corrected_eval)) = (&corr_keys_opt, static_eval_opt) {
        let quiet_best = best_move.is_null() || !best_move.flags.is_capture();
        if quiet_best && best_score.abs() < SCORE_MATE - 200 {
            info.correction_history.update(keys, best_score - corrected_eval, depth);
        }
    }

    // --- Stocker le résultat dans la table de transposition ---
    let tt_score = TranspositionTable::adjust_score_for_tt(best_score, ply as i32);
    tt.store(board.hash, tt_score, depth, tt_flag, best_move);

    best_score
}
