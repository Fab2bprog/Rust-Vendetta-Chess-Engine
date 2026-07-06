// =============================================================================
// Vendetta Chess Motor — src/bin/tuner.rs
//
// Rôle : Texel Tuning — calibre automatiquement un sous-ensemble des
//        constantes d'évaluation sur le fichier de positions produit par
//        extract_positions.rs (FEN;résultat, un par ligne).
//
// Portée de cette première version (v1) :
//   Paramètres ajustés : valeurs des pièces (Cavalier, Fou, Tour, Dame —
//   le Pion reste fixé à 100 comme ancre d'échelle, convention standard du
//   Texel Tuning) + pénalités pions doublés/isolés + bonus pions passés
//   (6 paliers d'avancement).  Soit 12 paramètres.
//
//   Volontairement NON inclus dans cette v1 : les tables de position (PST,
//   384 valeurs) et les autres critères (mobilité, centre, roi, finales).
//   Raison : valider d'abord que tout le pipeline fonctionne correctement
//   sur un jeu de paramètres restreint avant d'étendre la portée — chaque
//   paramètre supplémentaire multiplie le temps de calcul d'une passe.
//
// Pourquoi une évaluation séparée de eval::evaluate() :
//   Le moteur réel maintient le matériel et les PST de façon INCRÉMENTALE
//   (board.eval_mg / board.eval_eg, mis à jour à chaque place_piece /
//   remove_piece) pour la performance en recherche. Cette optimisation est
//   incompatible avec le tuning, qui doit pouvoir recalculer le score d'une
//   position pour des MILLIERS de jeux de paramètres candidats différents.
//   tunable_eval() ci-dessous recalcule donc le matériel et la structure de
//   pions DIRECTEMENT depuis les bitboards à chaque appel — plus lent que
//   le moteur réel, mais sans incidence : le tuning est un calcul hors-ligne,
//   pas une recherche en temps limité. Le moteur de production n'est pas
//   touché par ce fichier.
//
// Algorithme (Texel's Tuning Method — recherche locale par coordonnée) :
//   1. Charger toutes les positions (FEN + résultat) en mémoire.
//   2. Calculer l'erreur totale (MSE entre sigmoïde(eval) et résultat réel)
//      avec les paramètres de départ (valeurs actuelles du moteur).
//   3. Pour chaque paramètre, tour à tour : essayer +1, recalculer l'erreur
//      sur tout le jeu de données ; si meilleure, garder et continuer dans
//      ce sens ; sinon essayer -1 ; sinon laisser ce paramètre inchangé.
//   4. Répéter tous les paramètres jusqu'à ce qu'une passe complète n'améliore
//      plus rien (convergence).
//   5. Afficher les nouvelles valeurs — c'est à reporter manuellement dans
//      le code de production (material.rs, pawns.rs) après vérification.
//
// Utilisation :
//   cargo run --release --bin tuner -- positions.txt
// =============================================================================

use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::time::Instant;

use vendetta_chess_motor::board::bitboard::{
    init_attack_tables,
    knight_attacks, bishop_attacks, rook_attacks, queen_attacks, king_attacks,
    white_pawn_attacks, black_pawn_attacks, file_mask,
};
use vendetta_chess_motor::board::state::Board;
use vendetta_chess_motor::utils::types::{Color, Piece};
use vendetta_chess_motor::eval::tables::{
    mirror_square, PAWN_TABLE, KNIGHT_TABLE, BISHOP_TABLE, ROOK_TABLE, QUEEN_TABLE,
};

// =============================================================================
// Paramètres ajustables
// =============================================================================
//
// v2 — Extension du modèle v1 (matériel + structure de pions, 12 paramètres).
//
// Pourquoi cette extension était nécessaire :
//   Le tuning v1 a convergé vers une solution dégénérée — toutes les valeurs
//   de pièces divisées par ~2, et des bonus de pions passés NÉGATIFS aux
//   premiers rangs (chose impossible aux échecs : un pion passé n'est jamais
//   une faiblesse). Cause : le modèle v1 était trop pauvre pour expliquer la
//   variance des résultats réels (gaffes, pression du temps, tactiques que
//   le modèle ne voit pas) — l'optimiseur a "triché" en aplatissant tous les
//   poids vers zéro, ce qui réduit l'erreur quadratique sur un signal bruité
//   sans avoir aucun rapport avec la qualité réelle des positions.
//
//   v2 ajoute 10 paramètres (mobilité ×4, sécurité du roi ×3, centre ×2,
//   paire de fous ×1) pour donner au modèle assez d'expressivité — il peut
//   désormais expliquer une partie de la variance par autre chose que le
//   matériel brut, ce qui devrait éliminer l'incitation à aplatir l'échelle.
//
//   Simplification volontaire conservée : contrairement à evaluate(), ces
//   critères ne sont PAS désactivés en finale (pas de détection de phase
//   portée dans le tuner pour l'instant) — ils s'appliquent à toutes les
//   positions échantillonnées, milieu de partie et finale confondus.
//
// v4 — Ajout des tables de position (PST), Pion/Cavalier/Fou/Tour/Dame
//   uniquement (5 × 64 = 320 nouveaux paramètres, total 342).
//
//   Choix délibéré d'EXCLURE le Roi de cette extension (décidé explicitement
//   avec l'utilisateur avant d'écrire ce code) : en production, le Roi a
//   DEUX tables distinctes (KING_MIDDLEGAME_TABLE : rester protégé,
//   KING_ENDGAME_TABLE : centraliser) sélectionnées par la phase de la
//   partie. Ce tuner n'a toujours pas de détection de phase (voir note v2
//   ci-dessus) — il ne calcule qu'UN SEUL score par position, pas un mélange
//   MG/EG pondéré. Tuner une seule table de Roi reviendrait soit à l'appliquer
//   aux deux tables de production (perdant la distinction abri/centralisation
//   qui fait justement l'intérêt de ce découpage), soit à devoir ajouter la
//   détection de phase au tuner — un chantier à part, plus risqué pour la
//   convergence, réservé à une v5 éventuelle.
//
//   Pour les 5 pièces tunées ici, ce choix ne pose AUCUN problème de fidélité :
//   en production, ces 5 pièces utilisent déjà UNE SEULE table pour MG et EG
//   (voir eval/tables.rs::piece_square_values — seul le Roi a deux tables).
//   Le modèle simplifié du tuner est donc une représentation EXACTE de la
//   structure PST de production pour ces 5 pièces, pas une approximation.
//
//   Valeurs de départ importées DIRECTEMENT depuis eval::tables (pas de
//   copie-collé manuel des constantes — élimine tout risque de désynchro-
//   nisation entre le tuner et la production).
//
//   Coût : 342 paramètres contre 22 en v3, soit ~15,5× plus d'essais par
//   passe de coordinate descent. Le temps total de convergence sera
//   nettement plus long qu'en v3 (probablement des dizaines de minutes à
//   quelques heures selon le nombre de passes nécessaires) — à constater
//   empiriquement plutôt qu'à promettre un chiffre précis ici.

/// Nombre de paramètres scalaires (matériel, structure de pions, mobilité,
/// sécurité du roi, centre — hérités de v1/v2/v3). Les tables PST (v4)
/// commencent à cet index.
const NUM_SCALAR_PARAMS: usize = 22;

/// Index de base de chacune des 5 tables PST tunées, 64 valeurs chacune.
/// Convention IDENTIQUE à eval/tables.rs : case a1 = index 0 du point de
/// vue Blanc, mirror_square() pour les Noirs.
const IDX_PST_PAWN_BASE:   usize = NUM_SCALAR_PARAMS;             // 22
const IDX_PST_KNIGHT_BASE: usize = IDX_PST_PAWN_BASE   + 64;      // 86
const IDX_PST_BISHOP_BASE: usize = IDX_PST_KNIGHT_BASE + 64;      // 150
const IDX_PST_ROOK_BASE:   usize = IDX_PST_BISHOP_BASE + 64;      // 214
const IDX_PST_QUEEN_BASE:  usize = IDX_PST_ROOK_BASE   + 64;      // 278

/// Nombre total de paramètres ajustables (22 scalaires + 320 PST = 342).
const NUM_PARAMS: usize = IDX_PST_QUEEN_BASE + 64;

/// Noms des paramètres SCALAIRES uniquement (indices 0..NUM_SCALAR_PARAMS),
/// dans le même ordre que les index ci-dessous — pour l'affichage périodique
/// et final. Les 320 paramètres PST (v4) ont leur propre fonction d'affichage
/// dédiée (print_pst_table()) — un nom par case serait illisible ici.
const PARAM_NAMES: [&str; NUM_SCALAR_PARAMS] = [
    "knight", "bishop", "rook", "queen",
    "doubled_pawn_penalty", "isolated_pawn_penalty",
    "passed_pawn_bonus[rang2]", "passed_pawn_bonus[rang3]",
    "passed_pawn_bonus[rang4]", "passed_pawn_bonus[rang5]",
    "passed_pawn_bonus[rang6]", "passed_pawn_bonus[rang7]",
    "bishop_pair_bonus",
    "knight_mobility", "bishop_mobility", "rook_mobility", "queen_mobility",
    "shield_pawn_bonus", "king_center_penalty", "open_file_near_king_penalty",
    "center_pawn_bonus", "center_attack_bonus",
];

const IDX_KNIGHT: usize = 0;
const IDX_BISHOP: usize = 1;
const IDX_ROOK:   usize = 2;
const IDX_QUEEN:  usize = 3;
const IDX_DOUBLED:  usize = 4;
const IDX_ISOLATED: usize = 5;
const IDX_PASSED_BASE: usize = 6; // occupe les index 6 à 11 (rangs 2 à 7)
const IDX_BISHOP_PAIR: usize = 12;
const IDX_KNIGHT_MOB: usize = 13;
const IDX_BISHOP_MOB: usize = 14;
const IDX_ROOK_MOB:   usize = 15;
const IDX_QUEEN_MOB:  usize = 16;
const IDX_SHIELD_PAWN:        usize = 17;
const IDX_KING_CENTER_PEN:    usize = 18;
const IDX_OPEN_FILE_KING_PEN: usize = 19;
const IDX_CENTER_PAWN:  usize = 20;
const IDX_CENTER_ATTACK: usize = 21;

/// Paramètres ajustables, représentés comme un simple tableau de i32.
/// Choix volontaire (plutôt qu'une struct à champs nommés avec des
/// références mutables) : un tableau plat évite toute construction
/// d'emprunts imbriqués dans la boucle de coordinate descent — plus simple
/// à relire et à garantir correct sans pouvoir compiler pour vérifier ici.
#[derive(Clone, Debug)]
struct EvalParams {
    values: [i32; NUM_PARAMS],
}

impl EvalParams {
    /// Valeurs de départ = constantes actuelles du moteur de production.
    /// Scalaires : material.rs, pawns.rs, mobility.rs, king_safety.rs, center.rs.
    /// PST (v4) : importées DIRECTEMENT depuis eval::tables (voir note d'en-tête
    /// v4) — pas de copie-collé manuel, donc pas de risque de désynchronisation
    /// entre les valeurs de départ du tuner et la production.
    fn default_from_engine() -> Self {
        // [0i32; NUM_PARAMS] : tableau plat, rempli par tranches ci-dessous.
        let mut values = [0i32; NUM_PARAMS];

        let scalars: [i32; NUM_SCALAR_PARAMS] = [
            320, 330, 500, 900,     // knight, bishop, rook, queen
            -20, -20,                // doubled, isolated
            5, 10, 20, 35, 60, 100,  // passed[rang2..rang7]
            30,                      // bishop_pair_bonus
            4, 3, 2, 1,              // knight/bishop/rook/queen mobility
            10, -30, -15,            // shield_pawn, king_center_pen, open_file_pen
            15, 5,                   // center_pawn, center_attack
        ];
        values[0..NUM_SCALAR_PARAMS].copy_from_slice(&scalars);

        values[IDX_PST_PAWN_BASE   .. IDX_PST_PAWN_BASE   + 64].copy_from_slice(&PAWN_TABLE);
        values[IDX_PST_KNIGHT_BASE .. IDX_PST_KNIGHT_BASE + 64].copy_from_slice(&KNIGHT_TABLE);
        values[IDX_PST_BISHOP_BASE .. IDX_PST_BISHOP_BASE + 64].copy_from_slice(&BISHOP_TABLE);
        values[IDX_PST_ROOK_BASE   .. IDX_PST_ROOK_BASE   + 64].copy_from_slice(&ROOK_TABLE);
        values[IDX_PST_QUEEN_BASE  .. IDX_PST_QUEEN_BASE  + 64].copy_from_slice(&QUEEN_TABLE);

        EvalParams { values }
    }

    /// Valeur PST de `params` pour la table de base `base`, case `sq`
    /// (déjà retournée du point de vue de la couleur par l'appelant — voir
    /// pst_score() qui applique mirror_square() pour les Noirs avant d'appeler
    /// cette fonction).
    #[inline]
    fn pst(&self, base: usize, sq: u8) -> i32 {
        self.values[base + sq as usize]
    }

    #[inline]
    fn knight(&self) -> i32 { self.values[IDX_KNIGHT] }
    #[inline]
    fn bishop(&self) -> i32 { self.values[IDX_BISHOP] }
    #[inline]
    fn rook(&self)   -> i32 { self.values[IDX_ROOK] }
    #[inline]
    fn queen(&self)  -> i32 { self.values[IDX_QUEEN] }
    #[inline]
    fn doubled_pawn_penalty(&self)  -> i32 { self.values[IDX_DOUBLED] }
    #[inline]
    fn isolated_pawn_penalty(&self) -> i32 { self.values[IDX_ISOLATED] }
    /// `advancement` : 1 (rang 2) à 6 (rang 7).
    #[inline]
    fn passed_pawn_bonus(&self, advancement: i32) -> i32 {
        self.values[IDX_PASSED_BASE + (advancement - 1) as usize]
    }
    #[inline]
    fn bishop_pair_bonus(&self) -> i32 { self.values[IDX_BISHOP_PAIR] }
    #[inline]
    fn knight_mobility(&self) -> i32 { self.values[IDX_KNIGHT_MOB] }
    #[inline]
    fn bishop_mobility(&self) -> i32 { self.values[IDX_BISHOP_MOB] }
    #[inline]
    fn rook_mobility(&self)   -> i32 { self.values[IDX_ROOK_MOB] }
    #[inline]
    fn queen_mobility(&self)  -> i32 { self.values[IDX_QUEEN_MOB] }
    #[inline]
    fn shield_pawn_bonus(&self) -> i32 { self.values[IDX_SHIELD_PAWN] }
    #[inline]
    fn king_center_penalty(&self) -> i32 { self.values[IDX_KING_CENTER_PEN] }
    #[inline]
    fn open_file_near_king_penalty(&self) -> i32 { self.values[IDX_OPEN_FILE_KING_PEN] }
    #[inline]
    fn center_pawn_bonus(&self) -> i32 { self.values[IDX_CENTER_PAWN] }
    #[inline]
    fn center_attack_bonus(&self) -> i32 { self.values[IDX_CENTER_ATTACK] }
}

// =============================================================================
// Évaluation tunable (matériel + structure de pions uniquement, voir en-tête)
// =============================================================================

const PAWN_VALUE: i32 = 100; // ancre fixe, convention Texel Tuning standard

fn piece_value(params: &EvalParams, piece: Piece) -> i32 {
    match piece {
        Piece::Pawn   => PAWN_VALUE,
        Piece::Knight => params.knight(),
        Piece::Bishop => params.bishop(),
        Piece::Rook   => params.rook(),
        Piece::Queen  => params.queen(),
        Piece::King   => 0,
    }
}

fn material_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let mut score = 0i32;
    for piece in [Piece::Pawn, Piece::Knight, Piece::Bishop, Piece::Rook, Piece::Queen] {
        let count = board.pieces[color.index()][piece.index()].count_ones() as i32;
        score += count * piece_value(params, piece);
    }
    score
}

/// Structure de pions : doublés, isolés, passés — logique identique à
/// pawns.rs, mais avec les pénalités/bonus tirés de `params` au lieu des
/// constantes figées (et table de pion passé précalculée à chaque appel,
/// ici sans la table OnceLock de pawns.rs — acceptable car non chaud).
fn pawn_structure_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let pawns       = board.pieces[color.index()][Piece::Pawn.index()];
    let enemy_pawns = board.pieces[color.opposite().index()][Piece::Pawn.index()];
    let mut score   = 0i32;

    for file in 0u8..8 {
        let col_mask = 0x0101_0101_0101_0101u64 << file;
        let pawns_on_file = pawns & col_mask;
        let count = pawns_on_file.count_ones() as i32;
        if count == 0 { continue; }

        if count > 1 {
            score += params.doubled_pawn_penalty() * (count - 1);
        }

        let left  = if file > 0 { 0x0101_0101_0101_0101u64 << (file - 1) } else { 0 };
        let right = if file < 7 { 0x0101_0101_0101_0101u64 << (file + 1) } else { 0 };
        let adjacent = left | right;

        if pawns & adjacent == 0 {
            score += params.isolated_pawn_penalty() * count;
        }

        let mut bb = pawns_on_file;
        while bb != 0 {
            let sq = bb.trailing_zeros() as u8;
            bb &= bb - 1;
            let rank = sq / 8;

            let zone = col_mask | left | right;
            let front: u64 = match color {
                Color::White => {
                    let mut m = 0u64;
                    for r in (rank + 1)..8 { m |= 0x0000_0000_0000_00FFu64 << (r * 8) & zone; }
                    m
                }
                Color::Black => {
                    let mut m = 0u64;
                    for r in 0..rank { m |= 0x0000_0000_0000_00FFu64 << (r * 8) & zone; }
                    m
                }
            };

            if enemy_pawns & front == 0 {
                let advancement = match color {
                    Color::White => rank as i32,
                    Color::Black => 7 - rank as i32,
                };
                // advancement: 0=rang1(promu, n'arrive jamais), 1=rang2, ... 6=rang7, 7=rang8
                if (1..=6).contains(&advancement) {
                    score += params.passed_pawn_bonus(advancement);
                }
            }
        }
    }

    score
}

/// Tables de position (PST) — Pion/Cavalier/Fou/Tour/Dame uniquement (voir
/// note v4 en en-tête : le Roi est exclu, ses deux tables MG/EG de production
/// ne peuvent pas être représentées fidèlement par ce tuner sans détection
/// de phase).
///
/// Convention IDENTIQUE à eval/tables.rs::piece_square_values() : la case
/// est lue directement pour les Blancs, via mirror_square() pour les Noirs —
/// les tables tunées sont donc, comme en production, "du point de vue Blanc".
fn pst_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let mut score = 0i32;

    let mirrored = |sq: u8| -> u8 {
        if color == Color::White { sq } else { mirror_square(sq) }
    };

    let mut bb = board.pieces[color.index()][Piece::Pawn.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        score += params.pst(IDX_PST_PAWN_BASE, mirrored(sq));
    }

    let mut bb = board.pieces[color.index()][Piece::Knight.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        score += params.pst(IDX_PST_KNIGHT_BASE, mirrored(sq));
    }

    let mut bb = board.pieces[color.index()][Piece::Bishop.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        score += params.pst(IDX_PST_BISHOP_BASE, mirrored(sq));
    }

    let mut bb = board.pieces[color.index()][Piece::Rook.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        score += params.pst(IDX_PST_ROOK_BASE, mirrored(sq));
    }

    let mut bb = board.pieces[color.index()][Piece::Queen.index()];
    while bb != 0 {
        let sq = bb.trailing_zeros() as u8;
        bb &= bb - 1;
        score += params.pst(IDX_PST_QUEEN_BASE, mirrored(sq));
    }

    score
}

/// Bonus de paire de fous — logique identique à material::bishop_pair_score().
fn bishop_pair_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let count = board.pieces[color.index()][Piece::Bishop.index()].count_ones();
    if count >= 2 { params.bishop_pair_bonus() } else { 0 }
}

/// Mobilité des cavaliers/fous/tours/dames — logique identique à
/// mobility::mobility_score(), bonus tirés de `params` au lieu des constantes.
fn mobility_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let own_pieces = board.occupancy[color.index()];
    let occupied   = board.all_pieces;
    let mut score  = 0i32;

    let mut knights = board.pieces[color.index()][Piece::Knight.index()];
    while knights != 0 {
        let sq = knights.trailing_zeros() as u8;
        knights &= knights - 1;
        let moves = knight_attacks(sq) & !own_pieces;
        score += moves.count_ones() as i32 * params.knight_mobility();
    }

    let mut bishops = board.pieces[color.index()][Piece::Bishop.index()];
    while bishops != 0 {
        let sq = bishops.trailing_zeros() as u8;
        bishops &= bishops - 1;
        let moves = bishop_attacks(sq, occupied) & !own_pieces;
        score += moves.count_ones() as i32 * params.bishop_mobility();
    }

    let mut rooks = board.pieces[color.index()][Piece::Rook.index()];
    while rooks != 0 {
        let sq = rooks.trailing_zeros() as u8;
        rooks &= rooks - 1;
        let moves = rook_attacks(sq, occupied) & !own_pieces;
        score += moves.count_ones() as i32 * params.rook_mobility();
    }

    let mut queens = board.pieces[color.index()][Piece::Queen.index()];
    while queens != 0 {
        let sq = queens.trailing_zeros() as u8;
        queens &= queens - 1;
        let moves = queen_attacks(sq, occupied) & !own_pieces;
        score += moves.count_ones() as i32 * params.queen_mobility();
    }

    score
}

/// Sécurité du roi — logique identique à king_safety::king_safety_score(),
/// SANS désactivation en finale (le tuner n'a pas de détection de phase —
/// voir note en en-tête du fichier).
fn king_safety_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    let mut score  = 0i32;
    let king_sq    = board.king_square(color);
    let king_file  = king_sq % 8;
    let pawns      = board.pieces[color.index()][Piece::Pawn.index()];

    let shield_area  = king_attacks(king_sq) | (1u64 << king_sq);
    let shield_count = (shield_area & pawns).count_ones() as i32;
    score += shield_count * params.shield_pawn_bonus();

    if (2..=5).contains(&king_file) {
        score += params.king_center_penalty();
    }

    let enemy_rooks_queens = board.pieces[color.opposite().index()][Piece::Rook.index()]
                           | board.pieces[color.opposite().index()][Piece::Queen.index()];
    if enemy_rooks_queens != 0 {
        for f in king_file.saturating_sub(1)..=(king_file + 1).min(7) {
            let col = file_mask(f);
            if pawns & col == 0 {
                score += params.open_file_near_king_penalty();
            }
        }
    }

    score
}

/// Contrôle du centre (pions + pièces) — logique identique à center.rs,
/// fusionné en une seule fonction ici (pas besoin de séparer pour la mobilité
/// comme dans le moteur de production, le tuner n'a pas ce souci de partage
/// de calcul d'attaque).
fn center_score(params: &EvalParams, board: &Board, color: Color) -> i32 {
    const CENTER_SQUARES: u64 = (1u64 << 27) | (1u64 << 28) | (1u64 << 35) | (1u64 << 36);
    let occupied = board.all_pieces;
    let mut score = 0i32;

    let pawns = board.pieces[color.index()][Piece::Pawn.index()];
    score += (pawns & CENTER_SQUARES).count_ones() as i32 * params.center_pawn_bonus();

    let pawn_attacks = if color == Color::White {
        white_pawn_attacks(pawns)
    } else {
        black_pawn_attacks(pawns)
    };
    score += (pawn_attacks & CENTER_SQUARES).count_ones() as i32 * params.center_attack_bonus();

    let mut knights = board.pieces[color.index()][Piece::Knight.index()];
    while knights != 0 {
        let sq = knights.trailing_zeros() as u8;
        knights &= knights - 1;
        score += (knight_attacks(sq) & CENTER_SQUARES).count_ones() as i32 * params.center_attack_bonus();
    }

    let mut bishops = board.pieces[color.index()][Piece::Bishop.index()];
    while bishops != 0 {
        let sq = bishops.trailing_zeros() as u8;
        bishops &= bishops - 1;
        score += (bishop_attacks(sq, occupied) & CENTER_SQUARES).count_ones() as i32 * params.center_attack_bonus();
    }

    let mut rooks = board.pieces[color.index()][Piece::Rook.index()];
    while rooks != 0 {
        let sq = rooks.trailing_zeros() as u8;
        rooks &= rooks - 1;
        score += (rook_attacks(sq, occupied) & CENTER_SQUARES).count_ones() as i32 * params.center_attack_bonus();
    }

    let mut queens = board.pieces[color.index()][Piece::Queen.index()];
    while queens != 0 {
        let sq = queens.trailing_zeros() as u8;
        queens &= queens - 1;
        score += (queen_attacks(sq, occupied) & CENTER_SQUARES).count_ones() as i32 * params.center_attack_bonus();
    }

    score
}

/// Score total (matériel + structure de pions + mobilité + sécurité du roi +
/// centre + paire de fous), point de vue Blancs.
/// Toujours plus simple que evaluate() (pas de PST, pas de finale dédiée,
/// pas de phase) — voir portée v2 en en-tête du fichier.
fn tunable_eval_white_pov(params: &EvalParams, board: &Board) -> i32 {
    let mut white = 0i32;
    let mut black = 0i32;

    for color in [Color::White, Color::Black] {
        let total = material_score(params, board, color)
            + pawn_structure_score(params, board, color)
            + pst_score(params, board, color)
            + bishop_pair_score(params, board, color)
            + mobility_score(params, board, color)
            + king_safety_score(params, board, color)
            + center_score(params, board, color);

        if color == Color::White { white = total; } else { black = total; }
    }

    white - black
}

// =============================================================================
// Fonction d'erreur (Texel Tuning)
// =============================================================================

/// Échelle de la sigmoïde — calibrée sur LES DONNÉES (voir calibrate_k() plus
/// bas) plutôt que figée à la valeur "historique" de 400.
///
/// Pourquoi c'est indispensable et pas un simple détail :
///   Les tunings v1 et v2 ont tous les deux convergé vers un effondrement
///   d'échelle (matériel divisé par ~2) ET un signe incohérent sur les bonus
///   de pions passés aux premiers rangs. La cause de l'effondrement : K=400
///   n'avait jamais été calibré pour CE modèle et CES données précises — la
///   méthode Texel originale calibre K en premier (recherche 1D sur les
///   valeurs de départ), AVANT de toucher aux poids d'évaluation, précisément
///   pour éviter que l'optimiseur ne compense un mauvais calibrage d'échelle
///   en rétrécissant tous les autres paramètres. C'est l'étape qui manquait.
#[inline]
fn sigmoid(score: i32, k: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf(-(score as f64) / k))
}

/// Position pré-chargée : plateau déjà parsé + résultat réel de la partie
/// (point de vue Blancs : 1.0, 0.5 ou 0.0).
struct Sample {
    board:  Board,
    result: f64,
}

/// Calcule l'erreur quadratique moyenne sur tout le jeu de données pour un
/// jeu de paramètres donné. C'est la fonction appelée à CHAQUE essai de
/// valeur candidate dans la boucle de coordinate descent — son coût domine
/// le temps total du tuning.
///
/// Version séquentielle — conservée pour les petits jeux de données ou le
/// cas num_threads == 1. La version utilisée par défaut est total_error()
/// ci-dessous (parallèle).
fn total_error_sequential(params: &EvalParams, samples: &[Sample], k: f64) -> f64 {
    let mut sum = 0.0f64;
    for s in samples {
        let eval = tunable_eval_white_pov(params, &s.board);
        let pred = sigmoid(eval, k);
        let diff = s.result - pred;
        sum += diff * diff;
    }
    sum / samples.len() as f64
}

/// Calcule l'erreur quadratique moyenne en répartissant le jeu de données
/// sur `num_threads` threads — chaque thread traite une tranche contiguë et
/// indépendante du Vec<Sample>, sans aucune écriture partagée (somme locale
/// par thread, combinée à la fin). C'est une parallélisation "embarrassingly
/// parallel" : aucun verrou, aucune coordination pendant le calcul.
///
/// Gain attendu : proche du nombre de cœurs disponibles, puisque chaque
/// position est indépendante des autres (contrairement à la recherche
/// alpha-bêta du moteur, où le Lazy SMP doit composer avec du travail
/// redondant entre threads — ici, zéro redondance).
fn total_error(params: &EvalParams, samples: &[Sample], num_threads: usize, k: f64) -> f64 {
    if num_threads <= 1 || samples.len() < num_threads * 1000 {
        return total_error_sequential(params, samples, k);
    }

    // Division entière arrondie au supérieur (chaque thread traite un bloc, le
    // dernier éventuellement plus petit). div_ceil est stable depuis Rust 1.73.
    let chunk_size = samples.len().div_ceil(num_threads);

    std::thread::scope(|scope| {
        let handles: Vec<_> = samples
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut sum = 0.0f64;
                    for s in chunk {
                        let eval = tunable_eval_white_pov(params, &s.board);
                        let pred = sigmoid(eval, k);
                        let diff = s.result - pred;
                        sum += diff * diff;
                    }
                    sum
                })
            })
            .collect();

        let total: f64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        total / samples.len() as f64
    })
}

/// Calibre l'échelle de la sigmoïde K en minimisant l'erreur sur le jeu de
/// données, à PARAMÈTRES D'ÉVALUATION FIXES (les valeurs de départ, voir
/// note sur sigmoid() plus haut). Recherche ternaire : fonctionne car
/// l'erreur en fonction de K, pour un eval donné, est unimodale (un seul
/// minimum) — typique de ce genre de calibrage d'échelle.
///
/// Cette étape doit être effectuée UNE FOIS, avant la boucle de coordinate
/// descent sur les poids — jamais pendant, sous peine de réintroduire la
/// confusion entre "le bon K" et "les bons poids" qui causait l'effondrement
/// d'échelle observé dans les versions précédentes du tuner.
fn calibrate_k(params: &EvalParams, samples: &[Sample], num_threads: usize) -> f64 {
    let mut lo = 50.0f64;
    let mut hi = 1000.0f64;

    // ~30 itérations suffisent largement à atteindre une précision de 0.01
    // sur cette plage (chaque itération réduit l'intervalle d'un facteur 2/3) ;
    // la borne à 60 est une marge de sécurité, la sortie anticipée fait le travail.
    for _ in 0..60 {
        if (hi - lo) < 0.01 { break; }
        let m1 = lo + (hi - lo) / 3.0;
        let m2 = hi - (hi - lo) / 3.0;
        let e1 = total_error(params, samples, num_threads, m1);
        let e2 = total_error(params, samples, num_threads, m2);
        if e1 < e2 {
            hi = m2;
        } else {
            lo = m1;
        }
    }

    (lo + hi) / 2.0
}

// =============================================================================
// Chargement du jeu de données
// =============================================================================

fn load_samples(path: &str) -> Vec<Sample> {
    let file   = File::open(path).expect("Impossible d'ouvrir le fichier de positions");
    let reader = BufReader::with_capacity(4 << 20, file);

    let mut samples = Vec::with_capacity(2_000_000);
    let mut skipped = 0u64;

    for line in reader.lines() {
        let line = line.expect("Erreur de lecture");
        let Some(sep) = line.rfind(';') else { skipped += 1; continue; };
        let fen_str    = &line[..sep];
        let result_str = &line[sep + 1..];

        let Ok(result) = result_str.parse::<f64>() else { skipped += 1; continue; };
        let Ok(board)  = Board::from_fen(fen_str) else { skipped += 1; continue; };

        samples.push(Sample { board, result });
    }

    eprintln!("Positions chargées : {}", samples.len());
    if skipped > 0 {
        eprintln!("Lignes ignorées    : {} (FEN ou résultat invalide)", skipped);
    }
    samples
}

// =============================================================================
// Point d'entrée
// =============================================================================

/// Affiche l'état courant : erreur, temps écoulé, et les paramètres
/// SCALAIRES uniquement (22, voir PARAM_NAMES) — les 320 paramètres PST
/// (v4) sont délibérément omis ici : un dump de 320 valeurs toutes les
/// PRINT_EVERY passes serait illisible. Voir print_pst_table() pour
/// l'affichage final, formaté et bien plus utile (prêt à copier-coller).
///
/// BUG ÉVITÉ : la version précédente (v1-v3) bouclait sur `0..NUM_PARAMS`
/// en indexant PARAM_NAMES[i] — avec NUM_PARAMS désormais à 342 (v4) contre
/// PARAM_NAMES.len() == 22, ça aurait paniqué (index hors limites) dès le
/// premier appel. Corrigé en bouclant explicitement sur NUM_SCALAR_PARAMS.
fn print_status(pass: u32, error: f64, elapsed_s: f64, params: &EvalParams) {
    eprintln!(
        "── Passe {:>4} — erreur = {:.6} — {:.1}s écoulées ──",
        pass, error, elapsed_s
    );
    for (name, value) in PARAM_NAMES.iter().zip(params.values.iter()).take(NUM_SCALAR_PARAMS) {
        eprintln!("    {:<28} = {}", name, value);
    }
    eprintln!("    (320 paramètres PST omis ici — voir le rapport final)");
}

/// Affiche une table PST tunée, formatée comme un tableau Rust prêt à copier
/// directement dans eval/tables.rs (8 valeurs par ligne, alignées sur 4
/// caractères — même présentation que les tables actuelles du fichier).
fn print_pst_table(name: &str, params: &EvalParams, base: usize) {
    eprintln!("pub const {}: [i32; 64] = [", name);
    for rank in 0..8 {
        let row: Vec<String> = (0..8)
            .map(|file| format!("{:4}", params.values[base + rank * 8 + file]))
            .collect();
        eprintln!("    {},", row.join(","));
    }
    eprintln!("];");
}

/// Fréquence d'affichage du détail des paramètres (en nombre de passes).
///
/// Valeur 100 (héritée de v1/v2/v3) abaissée à 5 pour la v4 : avec 342
/// paramètres (~15,5× plus coûteux par passe qu'en v3, voir note d'en-tête
/// v4), 100 passes peuvent représenter 20-30 minutes de silence total dans
/// le terminal — au point de donner l'impression (à tort) que le programme
/// est bloqué. À 5 passes, un retour visuel apparaît en quelques dizaines de
/// secondes à quelques minutes selon le matériel, suffisant pour confirmer
/// que ça avance sans noyer la sortie comme le ferait un affichage à chaque
/// passe.
const PRINT_EVERY: u32 = 5;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage : {} <positions.txt>", args[0]);
        std::process::exit(1);
    }

    init_attack_tables();

    eprintln!("Chargement du jeu de données...");
    let load_start = Instant::now();
    let samples = load_samples(&args[1]);
    eprintln!("Chargé en {:.1}s", load_start.elapsed().as_secs_f64());

    // Toutes les positions sont déjà en RAM dans `samples` (Vec<Sample>) —
    // aucune passe ne retouche le disque. Le seul levier de vitesse restant
    // est la parallélisation du calcul d'erreur lui-même sur les cœurs
    // disponibles (somme indépendante par position, sans coordination).
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    eprintln!("Threads utilisés  : {}", num_threads);
    eprintln!();

    let params0 = EvalParams::default_from_engine();

    // --- Calibrage de K (voir calibrate_k() pour le pourquoi détaillé) ---
    // Étape OBLIGATOIRE avant le tuning des poids : sans elle, l'optimiseur
    // compense un mauvais calibrage d'échelle en rétrécissant les poids au
    // lieu de vraiment les calibrer — c'est ce qui causait l'effondrement
    // observé dans les versions précédentes du tuner.
    eprintln!("Calibrage de l'échelle K (recherche ternaire sur les valeurs de départ)...");
    let calib_start = Instant::now();
    let k = calibrate_k(&params0, &samples, num_threads);
    eprintln!("K calibré = {:.2} (référence historique : 400) — {:.1}s", k, calib_start.elapsed().as_secs_f64());
    eprintln!();

    let mut params = params0;

    let mut best_error = total_error(&params, &samples, num_threads, k);
    eprintln!("Erreur initiale (valeurs actuelles du moteur, K calibré) : {:.6}", best_error);
    eprintln!();

    // Pas d'ajustement : ±1 sur l'échelle centipions — suffisamment fin
    // pour ces paramètres, convention Texel Tuning standard.
    const STEP: i32 = 1;
    let mut improved_any = true;
    let mut pass = 0u32;
    let tune_start = Instant::now();

    while improved_any {
        improved_any = false;
        pass += 1;

        for idx in 0..NUM_PARAMS {
            // --- Essayer +STEP ---
            params.values[idx] += STEP;
            let err_plus = total_error(&params, &samples, num_threads, k);

            if err_plus < best_error {
                best_error = err_plus;
                improved_any = true;
                continue; // garder +STEP, passer au paramètre suivant
            }

            // --- Annuler le +STEP, puis essayer -STEP ---
            params.values[idx] -= 2 * STEP; // revient à -STEP par rapport à l'original
            let err_minus = total_error(&params, &samples, num_threads, k);

            if err_minus < best_error {
                best_error = err_minus;
                improved_any = true;
            } else {
                // Ni +STEP ni -STEP n'aident : revenir à la valeur d'origine.
                params.values[idx] += STEP;
            }
        }

        if pass.is_multiple_of(PRINT_EVERY) {
            print_status(pass, best_error, tune_start.elapsed().as_secs_f64(), &params);
        }
    }

    eprintln!();
    eprintln!("Convergence atteinte après {} passe(s).", pass);
    eprintln!();
    print_status(pass, best_error, tune_start.elapsed().as_secs_f64(), &params);
    eprintln!();
    eprintln!("=== Nouvelles valeurs (à reporter manuellement dans le code) ===");
    eprintln!("material.rs PIECE_VALUE :");
    eprintln!("  Pion     = {} (fixe, ancre)", PAWN_VALUE);
    eprintln!("  Cavalier = {}", params.knight());
    eprintln!("  Fou      = {}", params.bishop());
    eprintln!("  Tour     = {}", params.rook());
    eprintln!("  Dame     = {}", params.queen());
    eprintln!();
    eprintln!("pawns.rs :");
    eprintln!("  DOUBLED_PAWN_PENALTY  = {}", params.doubled_pawn_penalty());
    eprintln!("  ISOLATED_PAWN_PENALTY = {}", params.isolated_pawn_penalty());
    eprintln!(
        "  PASSED_PAWN_BONUS = [0, {}, {}, {}, {}, {}, {}, 0]",
        params.passed_pawn_bonus(1), params.passed_pawn_bonus(2), params.passed_pawn_bonus(3),
        params.passed_pawn_bonus(4), params.passed_pawn_bonus(5), params.passed_pawn_bonus(6),
    );
    eprintln!();
    eprintln!("material.rs :");
    eprintln!("  BISHOP_PAIR_BONUS = {}", params.bishop_pair_bonus());
    eprintln!();
    eprintln!("mobility.rs :");
    eprintln!("  KNIGHT_MOBILITY_BONUS = {}", params.knight_mobility());
    eprintln!("  BISHOP_MOBILITY_BONUS = {}", params.bishop_mobility());
    eprintln!("  ROOK_MOBILITY_BONUS   = {}", params.rook_mobility());
    eprintln!("  QUEEN_MOBILITY_BONUS  = {}", params.queen_mobility());
    eprintln!();
    eprintln!("king_safety.rs :");
    eprintln!("  SHIELD_PAWN_BONUS           = {}", params.shield_pawn_bonus());
    eprintln!("  KING_CENTER_PENALTY         = {}", params.king_center_penalty());
    eprintln!("  OPEN_FILE_NEAR_KING_PENALTY = {}", params.open_file_near_king_penalty());
    eprintln!();
    eprintln!("center.rs :");
    eprintln!("  CENTER_PAWN_BONUS  = {}", params.center_pawn_bonus());
    eprintln!("  CENTER_ATTACK_BONUS = {}", params.center_attack_bonus());
    eprintln!();
    eprintln!("eval/tables.rs (v4 — PST, Roi exclu, voir note d'en-tête) :");
    eprintln!();
    print_pst_table("PAWN_TABLE",   &params, IDX_PST_PAWN_BASE);
    eprintln!();
    print_pst_table("KNIGHT_TABLE", &params, IDX_PST_KNIGHT_BASE);
    eprintln!();
    print_pst_table("BISHOP_TABLE", &params, IDX_PST_BISHOP_BASE);
    eprintln!();
    print_pst_table("ROOK_TABLE",   &params, IDX_PST_ROOK_BASE);
    eprintln!();
    print_pst_table("QUEEN_TABLE",  &params, IDX_PST_QUEEN_BASE);
}
