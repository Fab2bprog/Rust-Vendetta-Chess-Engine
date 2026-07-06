// =============================================================================
// Vendetta Chess Motor — src/moves/mod.rs
//
// Rôle : Coordinateur de la génération des coups. Ce module est le point
//        d'entrée principal pour obtenir la liste des coups légaux d'une
//        position.
//
// Contenu :
//   - is_square_attacked() : détecte si une case est attaquée par une couleur
//   - is_in_check()        : détecte si un roi est en échec
//   - generate_legal_moves() : génère tous les coups LÉGAUX (zéro coup illégal)
//   - generate_pseudo_moves() : génère les coups pseudo-légaux (usage interne)
//   - perft()              : fonction de test pour valider la génération des coups
//
// Principe de génération légale :
//   1. On génère tous les pseudo-coups (peuvent laisser le roi en échec)
//   2. Pour chaque pseudo-coup, on le joue sur le plateau
//   3. Si le roi n'est pas en échec après le coup, c'est un coup légal
//   4. On annule le coup
//
//   Pour le roque, on vérifie en plus que le roi ne passe pas par une case
//   attaquée et n'est pas en échec au départ.
//
// Philosophie : correction absolue. Zéro coup illégal possible.
// =============================================================================

pub mod pawn;
pub mod knight;
pub mod bishop;
pub mod rook;
pub mod queen;
pub mod king;

use crate::utils::types::{Color, Piece, Move, MoveFlags};
use crate::board::state::Board;
use crate::board::bitboard::{
    Bitboard, pop_lsb,
    knight_attacks, king_attacks,
    rook_attacks, bishop_attacks, queen_attacks,
    white_pawn_attacks, black_pawn_attacks,
    FILE_A, FILE_H, RANK_1, RANK_8,
};

// =============================================================================
// MoveList — liste de coups à capacité fixe, allouée sur la PILE
// =============================================================================

/// Capacité maximale d'une MoveList.
///
/// Le record de coups LÉGAUX dans une position d'échecs est 218 ; les
/// pseudo-légaux générés avant filtrage restent bornés bien en dessous. 256
/// offre une marge confortable tout en restant une puissance de 2.
pub const MAX_MOVE_LIST: usize = 256;

/// Liste de coups à capacité fixe (`[Move; 256]`), allouée sur la PILE — donc
/// AUCUNE allocation tas par nœud, contrairement à `Vec<Move>`. C'est le gain
/// mémoire le plus important sur le chemin chaud : la quiescence représente
/// 80-90 % des nœuds, chacun générant au moins une liste de coups.
///
/// Zéro dépendance externe (pas d'`arrayvec`) — conforme à la philosophie du
/// projet. Se comporte comme une slice `[Move]` via Deref/DerefMut : `len()`,
/// `iter()`, indexation, `swap()`, slicing, `to_vec()`, `is_empty()` fonctionnent
/// directement, sans méthode dédiée.
pub struct MoveList {
    moves: [Move; MAX_MOVE_LIST],
    len:   usize,
}

impl MoveList {
    /// Crée une liste vide. Le tampon est pré-rempli de `Move::NULL` (jamais
    /// lu au-delà de `len` grâce au Deref qui tranche à `..len`).
    #[inline]
    pub fn new() -> MoveList {
        MoveList { moves: [Move::NULL; MAX_MOVE_LIST], len: 0 }
    }

    /// Ajoute un coup en fin de liste.
    ///
    /// Le générateur ne produit jamais plus de coups que `MAX_MOVE_LIST` (le
    /// maximum légal théorique est 218). En développement, un `debug_assert!`
    /// détecte tout dépassement ; en release on ignore silencieusement un
    /// débordement (impossible en pratique) plutôt que de paniquer ou d'écrire
    /// hors bornes — cohérent avec la politique "jamais de panic en production".
    #[inline]
    pub fn push(&mut self, mv: Move) {
        debug_assert!(self.len < MAX_MOVE_LIST,
            "MoveList::push : capacité {} dépassée", MAX_MOVE_LIST);
        if self.len < MAX_MOVE_LIST {
            self.moves[self.len] = mv;
            self.len += 1;
        }
    }

    /// Ne conserve que les coups satisfaisant le prédicat (équivalent de
    /// `Vec::retain`, compactage en place en O(n)).
    #[inline]
    pub fn retain<F: FnMut(&Move) -> bool>(&mut self, mut keep: F) {
        let mut write = 0usize;
        for read in 0..self.len {
            if keep(&self.moves[read]) {
                self.moves[write] = self.moves[read];
                write += 1;
            }
        }
        self.len = write;
    }

    /// Vide la liste (longueur remise à zéro ; le tampon n'est pas effacé).
    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl Default for MoveList {
    #[inline]
    fn default() -> Self { Self::new() }
}

impl std::ops::Deref for MoveList {
    type Target = [Move];
    #[inline]
    fn deref(&self) -> &[Move] {
        &self.moves[..self.len]
    }
}

impl std::ops::DerefMut for MoveList {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Move] {
        &mut self.moves[..self.len]
    }
}

// =============================================================================
// Détection d'attaques
// =============================================================================

/// Retourne true si la case `sq` est attaquée par la couleur `attacker`.
/// Vérifie toutes les pièces de la couleur attaquante.
pub fn is_square_attacked(board: &Board, sq: u8, attacker: Color) -> bool {
    let occupied = board.all_pieces;
    let sq_bb: Bitboard = 1u64 << sq;

    // --- Attaque par cavalier ---
    let knights = board.pieces[attacker.index()][Piece::Knight.index()];
    if knight_attacks(sq) & knights != 0 {
        return true;
    }

    // --- Attaque par roi ---
    let king = board.pieces[attacker.index()][Piece::King.index()];
    if king_attacks(sq) & king != 0 {
        return true;
    }

    // --- Attaque par pion ---
    // Les pions attaquent en diagonale. On vérifie si un pion adverse
    // peut atteindre `sq` depuis ses cases d'attaque.
    let pawns = board.pieces[attacker.index()][Piece::Pawn.index()];
    let pawn_attacks = match attacker {
        Color::White => white_pawn_attacks(pawns),
        Color::Black => black_pawn_attacks(pawns),
    };
    if pawn_attacks & sq_bb != 0 {
        return true;
    }

    // --- Attaque par tour ou dame (lignes horizontales/verticales) ---
    let rooks_queens = board.pieces[attacker.index()][Piece::Rook.index()]
                     | board.pieces[attacker.index()][Piece::Queen.index()];
    if rook_attacks(sq, occupied) & rooks_queens != 0 {
        return true;
    }

    // --- Attaque par fou ou dame (diagonales) ---
    let bishops_queens = board.pieces[attacker.index()][Piece::Bishop.index()]
                       | board.pieces[attacker.index()][Piece::Queen.index()];
    if bishop_attacks(sq, occupied) & bishops_queens != 0 {
        return true;
    }

    false
}

/// Retourne true si le roi de la couleur `color` est en échec.
pub fn is_in_check(board: &Board, color: Color) -> bool {
    let king_sq = board.king_square(color);
    is_square_attacked(board, king_sq, color.opposite())
}

// =============================================================================
// Génération des coups
// =============================================================================

/// Génère tous les pseudo-coups (peuvent laisser le roi en échec).
/// Appelé en interne par generate_legal_moves().
fn generate_pseudo_moves(board: &Board, moves: &mut crate::moves::MoveList) {
    let color = board.side_to_move;
    pawn::generate_pawn_moves(board, color, moves);
    knight::generate_knight_moves(board, color, moves);
    bishop::generate_bishop_moves(board, color, moves);
    rook::generate_rook_moves(board, color, moves);
    queen::generate_queen_moves(board, color, moves);
    king::generate_king_moves(board, color, moves);
}

/// Génère tous les coups LÉGAUX de la position courante.
/// Garantit : aucun coup retourné ne laisse le roi en échec.
/// Garantit : les roques sont validés (roi non en échec, cases traversées sûres).
pub fn generate_legal_moves(board: &mut Board) -> Vec<Move> {
    // Wrapper conservé pour les appelants HORS chemin chaud (perft, tuner,
    // extract_positions, benchmark, tests, parsing UCI) — il alloue un Vec, mais
    // ces usages ne sont pas critiques pour le NPS. Le moteur (alpha_beta /
    // quiescence) utilise generate_legal_moves_into() qui n'alloue rien.
    let mut list = MoveList::new();
    generate_legal_moves_into(board, &mut list);
    list.to_vec()
}

/// Version zéro-allocation de generate_legal_moves() : remplit la `MoveList`
/// fournie par l'appelant (typiquement allouée sur la pile dans la recherche).
/// La liste est vidée au début — son contenu précédent est ignoré.
pub fn generate_legal_moves_into(board: &mut Board, out: &mut MoveList) {
    out.clear();

    // Pseudo-coups générés sur la pile (aucune allocation tas).
    let mut pseudo_moves = MoveList::new();
    generate_pseudo_moves(board, &mut pseudo_moves);

    // Filtrage légal : chemin rapide (clouages) pour le cas courant, make/unmake
    // pour les cas délicats. Voir filter_legal_into().
    filter_legal_into(board, &pseudo_moves, out);
}

/// Vérifie si un coup de roque est légal :
/// - Le roi ne doit pas être en échec au départ
/// - Les cases traversées par le roi ne doivent pas être attaquées
fn is_castling_legal(board: &Board, mv: &Move, color: Color) -> bool {
    let enemy = color.opposite();
    let king_sq = mv.from;

    // Le roi ne doit pas être en échec au départ
    if is_square_attacked(board, king_sq, enemy) {
        return false;
    }

    // Vérifier les cases traversées par le roi
    match mv.flags {
        MoveFlags::CastleKingside => {
            // Le roi passe par f1/f8 (king_sq + 1) et arrive en g1/g8 (king_sq + 2)
            if is_square_attacked(board, king_sq + 1, enemy) { return false; }
            if is_square_attacked(board, king_sq + 2, enemy) { return false; }
        }
        MoveFlags::CastleQueenside => {
            // Le roi passe par d1/d8 (king_sq - 1) et arrive en c1/c8 (king_sq - 2)
            if is_square_attacked(board, king_sq - 1, enemy) { return false; }
            if is_square_attacked(board, king_sq - 2, enemy) { return false; }
        }
        _ => {}
    }

    true
}

// =============================================================================
// Filtrage légal accéléré par détection des clouages
// =============================================================================

/// Bitboard des pièces de `us` ABSOLUMENT clouées sur leur roi (ne peuvent
/// bouger que le long de la ligne du clouage sans exposer le roi).
///
/// PRÉCONDITION : le roi de `us` n'est PAS en échec (le chemin rapide de
/// filter_legal_into() n'appelle cette fonction que dans ce cas). Sous cette
/// précondition, le résultat est EXACT — ni faux positif ni faux négatif :
///   - On part des pièces de `us` qui bloquent EN PREMIER une ligne du roi
///     (`rook_attacks`/`bishop_attacks` depuis le roi s'arrêtent au 1er bloqueur).
///   - On retire chaque bloqueur et on regarde si une pièce glissante adverse du
///     bon type (tour/dame en ligne, fou/dame en diagonale) attaque alors le roi.
///     Comme le roi n'était PAS en échec, aucune glissante n'attaquait avant :
///     un attaquant révélé par le retrait ne peut venir que de la ligne de ce
///     bloqueur → le bloqueur est réellement cloué.
///
/// Le coût est de quelques lookups magiques par nœud (un par pièce bloquant une
/// ligne du roi, typiquement 0 à 4), bien moins cher que ~35 make/unmake.
fn pinned_pieces(board: &Board, us: Color) -> Bitboard {
    let king_sq = board.king_square(us);
    let them    = us.opposite();
    let occ     = board.all_pieces;
    let own     = board.occupancy[us.index()];

    let rook_sliders   = board.pieces[them.index()][Piece::Rook.index()]
                       | board.pieces[them.index()][Piece::Queen.index()];
    let bishop_sliders = board.pieces[them.index()][Piece::Bishop.index()]
                       | board.pieces[them.index()][Piece::Queen.index()];

    let mut pinned: Bitboard = 0;

    // Clouages le long des lignes droites (tours / dames).
    let mut blockers = rook_attacks(king_sq, occ) & own;
    while blockers != 0 {
        let sq = pop_lsb(&mut blockers);
        if rook_attacks(king_sq, occ ^ (1u64 << sq)) & rook_sliders != 0 {
            pinned |= 1u64 << sq;
        }
    }

    // Clouages le long des diagonales (fous / dames).
    let mut blockers = bishop_attacks(king_sq, occ) & own;
    while blockers != 0 {
        let sq = pop_lsb(&mut blockers);
        if bishop_attacks(king_sq, occ ^ (1u64 << sq)) & bishop_sliders != 0 {
            pinned |= 1u64 << sq;
        }
    }

    pinned
}

/// Filtre les pseudo-coups `pseudo` en coups LÉGAUX, ajoutés dans `out`.
///
/// CHEMIN RAPIDE (zéro make/unmake) pour le cas courant — pas en échec, pièce
/// ni clouée ni roi, ni roque, ni prise en passant : le coup est légal par
/// construction. Justification : si le roi n'est pas en échec, déplacer une
/// pièce NON clouée (autre que le roi) ne peut pas exposer son propre roi (seul
/// le retrait d'un cloueur le pourrait). La case d'arrivée est sans incidence
/// pour une pièce autre que le roi.
///
/// CHEMIN SÛR (make / is_in_check / unmake) pour tous les autres cas : en échec
/// (le roi doit répondre), coup du roi (peut entrer en échec), roque (validé en
/// plus par is_castling_legal — cases traversées), prise en passant (échec à la
/// découverte horizontal après retrait du pion capturé, qu'un simple test de
/// clouage du pion qui joue ne détecte pas), ou pièce clouée (légale seulement
/// le long de la ligne — vérifié par make/unmake).
///
/// FILET DE SÉCURITÉ : en build de DEBUG uniquement, chaque décision du chemin
/// rapide est revérifiée contre make/unmake. Toute divergence (un clouage qui
/// aurait été manqué) fait échouer immédiatement perft / `cargo test`, AVANT
/// toute partie réelle. En release, le chemin rapide ne fait aucun make/unmake.
fn filter_legal_into(board: &mut Board, pseudo: &MoveList, out: &mut MoveList) {
    let us       = board.side_to_move;
    let in_check = is_in_check(board, us);
    let king_sq  = board.king_square(us);
    let pinned   = if in_check { 0 } else { pinned_pieces(board, us) };

    for &mv in pseudo.iter() {
        let needs_full_check = in_check
            || mv.from == king_sq
            || mv.flags == MoveFlags::EnPassant
            || (pinned & (1u64 << mv.from)) != 0;

        if needs_full_check {
            // Roque : validation supplémentaire (roi non en échec, cases
            // traversées non attaquées). Le `from` d'un roque EST la case du roi,
            // donc ce cas est bien capté par `mv.from == king_sq` ci-dessus.
            if (mv.flags == MoveFlags::CastleKingside
                || mv.flags == MoveFlags::CastleQueenside)
                && !is_castling_legal(board, &mv, us)
            {
                continue;
            }
            board.make_move(mv);
            let legal = !is_in_check(board, us);
            board.unmake_move(mv);
            if legal {
                out.push(mv);
            }
        } else {
            // Chemin rapide : coup légal garanti par construction.
            #[cfg(debug_assertions)]
            {
                // Filet de sécurité (debug uniquement) : revérifier que le coup
                // est bien légal. Si ce debug_assert se déclenche, pinned_pieces
                // a manqué un clouage — bug à corriger avant toute partie.
                board.make_move(mv);
                let really_legal = !is_in_check(board, us);
                board.unmake_move(mv);
                debug_assert!(
                    really_legal,
                    "FAST-PATH clouage manqué : coup {:?} jugé légal sans \
                     vérification mais laisse le roi en échec",
                    mv
                );
            }
            out.push(mv);
        }
    }
}

// =============================================================================
// Génération rapide des captures (pour la recherche de quiescence)
//
// Principe : générer UNIQUEMENT les pseudo-captures (captures, en passant,
// promotion-captures), puis filtrer la légalité via make/unmake.
//
// Pourquoi c'est critique :
//   L'ancienne implémentation appelait generate_legal_moves() (make/unmake pour
//   ~35 pseudo-coups en moyenne) puis jetait ~30 coups silencieux.
//   La nouvelle ne fait make/unmake que sur ~5 pseudo-captures en moyenne.
//   La quiescence représentant 80-90% des nœuds totaux, c'est le gain le plus
//   important possible sur les NPS.
//
// Captures générées (identiques à l'ancienne implémentation) :
//   ✓ Captures normales de toutes les pièces
//   ✓ Prises en passant
//   ✓ Promotion-captures (×4 promotions)
//   ✗ Promotions silencieuses (non incluses, comme avant)
//   ✗ Roques (jamais des captures)
// =============================================================================

/// Génère les pseudo-captures des pions de la couleur `color`.
///
/// Inclut : captures diagonales normales, promotion-captures, prises en passant.
/// N'inclut PAS les poussées simples, poussées doubles, ni promotions silencieuses.
///
/// La logique bitboard est une extraction directe de pawn.rs — seules les
/// branches "captures" sont conservées, les branches "push" sont supprimées.
fn generate_pawn_captures(board: &Board, color: Color, moves: &mut crate::moves::MoveList) {
    let pawns = board.pieces[color.index()][Piece::Pawn.index()];
    let enemy = board.occupancy[color.opposite().index()];

    match color {
        // -----------------------------------------------------------------------
        // Pions BLANCS — avancent vers les rangs croissants (+8 par rang)
        // -----------------------------------------------------------------------
        Color::White => {
            // --- Captures vers le Nord-Est (diagonale droite) ---
            // Un pion blanc sur sq capture sur sq+9 si sq n'est pas sur la colonne H.
            let cap_ne       = ((pawns & !FILE_H) << 9) & enemy;
            let cap_ne_promo = cap_ne & RANK_8;   // Capture sur la dernière rangée → promo
            let cap_ne_norm  = cap_ne & !RANK_8;

            let mut bb = cap_ne_norm;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to - 9;
                moves.push(Move::capture(from, to));
            }
            let mut bb = cap_ne_promo;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to - 9;
                // Toujours émettre les 4 pièces — la GUI choisira
                moves.push(Move::promotion_capture(from, to, 4)); // Dame
                moves.push(Move::promotion_capture(from, to, 3)); // Tour
                moves.push(Move::promotion_capture(from, to, 2)); // Fou
                moves.push(Move::promotion_capture(from, to, 1)); // Cavalier
            }

            // --- Captures vers le Nord-Ouest (diagonale gauche) ---
            // Un pion blanc sur sq capture sur sq+7 si sq n'est pas sur la colonne A.
            let cap_nw       = ((pawns & !FILE_A) << 7) & enemy;
            let cap_nw_promo = cap_nw & RANK_8;
            let cap_nw_norm  = cap_nw & !RANK_8;

            let mut bb = cap_nw_norm;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to - 7;
                moves.push(Move::capture(from, to));
            }
            let mut bb = cap_nw_promo;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to - 7;
                moves.push(Move::promotion_capture(from, to, 4));
                moves.push(Move::promotion_capture(from, to, 3));
                moves.push(Move::promotion_capture(from, to, 2));
                moves.push(Move::promotion_capture(from, to, 1));
            }

            // --- Prise en passant blanche ---
            // Copie exacte de generate_white_pawn_moves() — même logique bitboard.
            if let Some(ep_sq) = board.en_passant {
                let ep_bb: Bitboard = 1u64 << ep_sq;
                // Chercher les pions blancs pouvant atteindre ep_sq en diagonale
                let ep_attackers =
                    (((ep_bb >> 9) & !FILE_H) | ((ep_bb >> 7) & !FILE_A)) & pawns;
                let mut bb = ep_attackers;
                while bb != 0 {
                    let from = pop_lsb(&mut bb);
                    moves.push(Move::en_passant(from, ep_sq));
                }
            }
        }

        // -----------------------------------------------------------------------
        // Pions NOIRS — avancent vers les rangs décroissants (-8 par rang)
        // -----------------------------------------------------------------------
        Color::Black => {
            // --- Captures vers le Sud-Est (diagonale droite pour les noirs) ---
            // Un pion noir sur sq capture sur sq-7 si sq n'est pas sur la colonne H.
            let cap_se       = ((pawns & !FILE_H) >> 7) & enemy;
            let cap_se_promo = cap_se & RANK_1;   // Capture sur le rang 1 → promo
            let cap_se_norm  = cap_se & !RANK_1;

            let mut bb = cap_se_norm;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to + 7;
                moves.push(Move::capture(from, to));
            }
            let mut bb = cap_se_promo;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to + 7;
                moves.push(Move::promotion_capture(from, to, 4));
                moves.push(Move::promotion_capture(from, to, 3));
                moves.push(Move::promotion_capture(from, to, 2));
                moves.push(Move::promotion_capture(from, to, 1));
            }

            // --- Captures vers le Sud-Ouest (diagonale gauche pour les noirs) ---
            // Un pion noir sur sq capture sur sq-9 si sq n'est pas sur la colonne A.
            let cap_sw       = ((pawns & !FILE_A) >> 9) & enemy;
            let cap_sw_promo = cap_sw & RANK_1;
            let cap_sw_norm  = cap_sw & !RANK_1;

            let mut bb = cap_sw_norm;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to + 9;
                moves.push(Move::capture(from, to));
            }
            let mut bb = cap_sw_promo;
            while bb != 0 {
                let to   = pop_lsb(&mut bb);
                let from = to + 9;
                moves.push(Move::promotion_capture(from, to, 4));
                moves.push(Move::promotion_capture(from, to, 3));
                moves.push(Move::promotion_capture(from, to, 2));
                moves.push(Move::promotion_capture(from, to, 1));
            }

            // --- Prise en passant noire ---
            // Copie exacte de generate_black_pawn_moves() — même logique bitboard.
            if let Some(ep_sq) = board.en_passant {
                let ep_bb: Bitboard = 1u64 << ep_sq;
                let ep_attackers =
                    (((ep_bb << 9) & !FILE_A) | ((ep_bb << 7) & !FILE_H)) & pawns;
                let mut bb = ep_attackers;
                while bb != 0 {
                    let from = pop_lsb(&mut bb);
                    moves.push(Move::en_passant(from, ep_sq));
                }
            }
        }
    }
}

/// Génère toutes les pseudo-captures pour la couleur ayant le trait.
///
/// Pour chaque type de pièce, on intersecte directement les cases attaquées
/// avec le bitboard ennemi (`& enemy`). Ceci évite de générer les coups
/// silencieux et le test `board.all_pieces & (1u64 << to)` de la version complète.
///
/// Le roque est exclu : il ne peut jamais être une capture.
/// Les promotions silencieuses sont exclues : gérées hors de la quiescence.
fn generate_pseudo_captures(board: &Board, moves: &mut crate::moves::MoveList) {
    let color    = board.side_to_move;
    let enemy    = board.occupancy[color.opposite().index()];
    let occupied = board.all_pieces;

    // --- Pions ---
    generate_pawn_captures(board, color, moves);

    // --- Cavaliers ---
    let mut knights = board.pieces[color.index()][Piece::Knight.index()];
    while knights != 0 {
        let from     = pop_lsb(&mut knights);
        // Attaques du cavalier intersectées avec les pièces ennemies uniquement
        let mut caps = knight_attacks(from) & enemy;
        while caps != 0 {
            let to = pop_lsb(&mut caps);
            moves.push(Move::capture(from, to));
        }
    }

    // --- Fous ---
    let mut bishops = board.pieces[color.index()][Piece::Bishop.index()];
    while bishops != 0 {
        let from     = pop_lsb(&mut bishops);
        let mut caps = bishop_attacks(from, occupied) & enemy;
        while caps != 0 {
            let to = pop_lsb(&mut caps);
            moves.push(Move::capture(from, to));
        }
    }

    // --- Tours ---
    let mut rooks = board.pieces[color.index()][Piece::Rook.index()];
    while rooks != 0 {
        let from     = pop_lsb(&mut rooks);
        let mut caps = rook_attacks(from, occupied) & enemy;
        while caps != 0 {
            let to = pop_lsb(&mut caps);
            moves.push(Move::capture(from, to));
        }
    }

    // --- Dames ---
    let mut queens = board.pieces[color.index()][Piece::Queen.index()];
    while queens != 0 {
        let from     = pop_lsb(&mut queens);
        // La dame combine les attaques de la tour et du fou
        let mut caps = queen_attacks(from, occupied) & enemy;
        while caps != 0 {
            let to = pop_lsb(&mut caps);
            moves.push(Move::capture(from, to));
        }
    }

    // --- Roi ---
    // Le roi ne peut pas roquer : le roque n'est jamais une capture.
    let king_sq  = board.king_square(color);
    let mut caps = king_attacks(king_sq) & enemy;
    while caps != 0 {
        let to = pop_lsb(&mut caps);
        moves.push(Move::capture(king_sq, to));
    }
}

/// Génère tous les coups de capture LÉGAUX (captures, en passant, promotion-captures).
///
/// Garantit : aucun coup retourné ne laisse le roi en échec.
///
/// Complexité vs ancienne implémentation :
///   Avant : generate_legal_moves() → make/unmake pour ~35 pseudo-coups → filtrer
///   Après : generate_pseudo_captures() → make/unmake pour ~5 pseudo-captures → résultat
///
/// La quiescence représentant 80-90% des nœuds totaux d'une recherche, ce changement
/// est le plus impactant possible sur les performances du moteur.
pub fn generate_legal_captures(board: &mut Board) -> Vec<Move> {
    // Wrapper allouant un Vec — conservé pour d'éventuels appelants hors chemin
    // chaud. Le moteur utilise generate_legal_captures_into() (zéro allocation).
    let mut list = MoveList::new();
    generate_legal_captures_into(board, &mut list);
    list.to_vec()
}

/// Version zéro-allocation de generate_legal_captures() : remplit la `MoveList`
/// fournie (typiquement sur la pile). La liste est vidée au début.
pub fn generate_legal_captures_into(board: &mut Board, out: &mut MoveList) {
    out.clear();

    // Pseudo-captures générées sur la pile.
    let mut pseudo = MoveList::new();
    generate_pseudo_captures(board, &mut pseudo);

    // Même filtrage légal accéléré que pour les coups complets (chemin rapide
    // par clouages + make/unmake pour les cas délicats — en passant inclus).
    filter_legal_into(board, &pseudo, out);
}

/// Retourne true si la position est un pat (aucun coup légal, roi non en échec).
pub fn is_stalemate(board: &mut Board) -> bool {
    let moves = generate_legal_moves(board);
    moves.is_empty() && !is_in_check(board, board.side_to_move)
}

/// Retourne true si la position est un échec et mat.
pub fn is_checkmate(board: &mut Board) -> bool {
    let moves = generate_legal_moves(board);
    moves.is_empty() && is_in_check(board, board.side_to_move)
}

// =============================================================================
// Perft — Test de génération des coups
//
// Perft (PERFormance Test) compte le nombre de noeuds feuilles à une profondeur
// donnée. Les résultats sont connus et peuvent être vérifiés contre des tables
// de référence pour valider la correction de la génération des coups.
// =============================================================================

/// Compte le nombre de positions atteignables à la profondeur `depth`.
/// Résultats de référence pour la position initiale :
///   depth 1 → 20
///   depth 2 → 400
///   depth 3 → 8902
///   depth 4 → 197281
///   depth 5 → 4865609
pub fn perft(board: &mut Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }

    let moves = generate_legal_moves(board);

    if depth == 1 {
        return moves.len() as u64;
    }

    let mut count = 0u64;
    for mv in moves {
        board.make_move(mv);
        count += perft(board, depth - 1);
        board.unmake_move(mv);
    }

    count
}

/// Version de perft qui affiche le détail par coup (utile pour le débogage).
pub fn perft_divide(board: &mut Board, depth: u32) -> u64 {
    let moves = generate_legal_moves(board);
    let mut total = 0u64;

    for mv in moves {
        board.make_move(mv);
        let count = perft(board, depth - 1);
        board.unmake_move(mv);

        println!("{}: {}", mv.to_uci(), count);
        total += count;
    }

    println!("Total : {}", total);
    total
}

// =============================================================================
// Tests Perft — Validation de la génération de coups
//
// Ces tests comparent les résultats de perft() aux valeurs de référence de la
// Chess Programming Wiki : https://www.chessprogramming.org/Perft_Results
//
// Un écart de 1 nœud, même à profondeur 3, révèle un bug précis dans la
// génération : roque illégal accepté, prise en passant manquée, clouage
// ignoré, promotion incorrecte, etc.
//
// Organisation :
//   Tests rapides  — profondeur ≤ 3, < 100 000 nœuds → lancés par `cargo test`
//   Tests lents    — profondeur 4-5, millions de nœuds → #[ignore], lancés avec
//                    `cargo test -- --include-ignored` ou via `cargo run --bin perft`
//
// Utilisation recommandée :
//   1. `cargo test`                               : tests rapides (quelques secondes)
//   2. `cargo test -- --include-ignored`          : suite complète (quelques minutes debug)
//   3. `cargo run --release --bin perft`          : suite optimisée (< 30 secondes release)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::state::Board;

    // =========================================================================
    // Position 1 — Position initiale
    // rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1
    // Source : https://www.chessprogramming.org/Perft_Results
    // Couvre : cas de base, toutes les pièces normales
    // =========================================================================

    #[test]
    fn pos1_initiale_d1() {
        let mut b = Board::start_position();
        assert_eq!(perft(&mut b, 1), 20,
            "Position initiale d1 : attendu 20 coups (8 pions × 2 + 2 cavaliers × 2)");
    }

    #[test]
    fn pos1_initiale_d2() {
        let mut b = Board::start_position();
        assert_eq!(perft(&mut b, 2), 400, "Position initiale d2 : attendu 400");
    }

    #[test]
    fn pos1_initiale_d3() {
        let mut b = Board::start_position();
        assert_eq!(perft(&mut b, 3), 8_902, "Position initiale d3 : attendu 8 902");
    }

    #[test]
    #[ignore = "lent en debug (~200K noeuds) — utiliser --release ou --bin perft"]
    fn pos1_initiale_d4() {
        let mut b = Board::start_position();
        assert_eq!(perft(&mut b, 4), 197_281, "Position initiale d4 : attendu 197 281");
    }

    #[test]
    #[ignore = "lent en debug (~5M noeuds) — utiliser --release ou --bin perft"]
    fn pos1_initiale_d5() {
        let mut b = Board::start_position();
        assert_eq!(perft(&mut b, 5), 4_865_609, "Position initiale d5 : attendu 4 865 609");
    }

    // =========================================================================
    // Position 2 — Kiwipete
    // r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1
    // Couvre : roques des deux côtés, prises en passant, promotions
    //          découvertes d'échec, positions complexes
    // =========================================================================

    #[test]
    fn pos2_kiwipete_d1() {
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 1), 48, "Kiwipete d1 : attendu 48");
    }

    #[test]
    fn pos2_kiwipete_d2() {
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 2), 2_039, "Kiwipete d2 : attendu 2 039");
    }

    #[test]
    #[ignore = "lent en debug (~100K noeuds) — utiliser --release ou --bin perft"]
    fn pos2_kiwipete_d3() {
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 3), 97_862, "Kiwipete d3 : attendu 97 862");
    }

    #[test]
    #[ignore = "lent en debug (~4M noeuds) — utiliser --release ou --bin perft"]
    fn pos2_kiwipete_d4() {
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 4), 4_085_603, "Kiwipete d4 : attendu 4 085 603");
    }

    // =========================================================================
    // Position 3 — Finale avec pions passés
    // 8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1
    // Couvre : prises en passant edge-case, promotions multiples, peu de pièces
    // =========================================================================

    #[test]
    fn pos3_finale_d1() {
        let fen = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 1), 14, "Pos3 d1 : attendu 14");
    }

    #[test]
    fn pos3_finale_d2() {
        let fen = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 2), 191, "Pos3 d2 : attendu 191");
    }

    #[test]
    fn pos3_finale_d3() {
        let fen = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 3), 2_812, "Pos3 d3 : attendu 2 812");
    }

    #[test]
    #[ignore = "lent en debug (~43K noeuds)"]
    fn pos3_finale_d4() {
        let fen = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 4), 43_238, "Pos3 d4 : attendu 43 238");
    }

    #[test]
    #[ignore = "lent en debug (~675K noeuds)"]
    fn pos3_finale_d5() {
        let fen = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 5), 674_624, "Pos3 d5 : attendu 674 624");
    }

    // =========================================================================
    // Position 4 — Promotions et roques minoritaires
    // r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1
    // Couvre : promotions de 7 pions blancs en position, roques limités (kq seulement)
    // =========================================================================

    #[test]
    fn pos4_promotions_d1() {
        let fen = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 1), 6, "Pos4 d1 : attendu 6");
    }

    #[test]
    fn pos4_promotions_d2() {
        let fen = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 2), 264, "Pos4 d2 : attendu 264");
    }

    #[test]
    fn pos4_promotions_d3() {
        let fen = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 3), 9_467, "Pos4 d3 : attendu 9 467");
    }

    #[test]
    #[ignore = "lent en debug (~422K noeuds)"]
    fn pos4_promotions_d4() {
        let fen = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 4), 422_333, "Pos4 d4 : attendu 422 333");
    }

    // =========================================================================
    // Position 5 — En passant et promotions edge-cases
    // rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8
    // Couvre : prise en passant double, promotions avec capture, roi sans roque
    // =========================================================================

    #[test]
    fn pos5_ep_promos_d1() {
        let fen = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 1), 44, "Pos5 d1 : attendu 44");
    }

    #[test]
    fn pos5_ep_promos_d2() {
        let fen = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 2), 1_486, "Pos5 d2 : attendu 1 486");
    }

    #[test]
    #[ignore = "lent en debug (~62K noeuds)"]
    fn pos5_ep_promos_d3() {
        let fen = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 3), 62_379, "Pos5 d3 : attendu 62 379");
    }

    #[test]
    #[ignore = "lent en debug (~2M noeuds)"]
    fn pos5_ep_promos_d4() {
        let fen = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 4), 2_103_487, "Pos5 d4 : attendu 2 103 487");
    }

    // =========================================================================
    // Position 6 — Milieu de partie équilibré
    // r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10
    // Couvre : positions ouvertes, fous en fianchetto, pas de roque disponible
    // =========================================================================

    #[test]
    fn pos6_milieu_d1() {
        let fen = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 1), 46, "Pos6 d1 : attendu 46");
    }

    #[test]
    fn pos6_milieu_d2() {
        let fen = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 2), 2_079, "Pos6 d2 : attendu 2 079");
    }

    #[test]
    #[ignore = "lent en debug (~90K noeuds)"]
    fn pos6_milieu_d3() {
        let fen = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 3), 89_890, "Pos6 d3 : attendu 89 890");
    }

    #[test]
    #[ignore = "lent en debug (~4M noeuds)"]
    fn pos6_milieu_d4() {
        let fen = "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";
        let mut b = Board::from_fen(fen).unwrap();
        assert_eq!(perft(&mut b, 4), 3_894_594, "Pos6 d4 : attendu 3 894 594");
    }

    // =========================================================================
    // Tests de légalité générale
    // =========================================================================

    #[test]
    fn aucun_coup_legal_ne_laisse_le_roi_en_echec() {
        // Vérifier qu'aucun coup légal ne laisse le roi en échec — position initiale.
        let mut board = Board::start_position();
        let legal_moves = generate_legal_moves(&mut board);
        for mv in legal_moves {
            board.make_move(mv);
            assert!(!is_in_check(&board, Color::White),
                "Le coup {} laisse le roi blanc en échec !", mv.to_uci());
            board.unmake_move(mv);
        }
    }

    #[test]
    fn aucun_coup_legal_ne_laisse_le_roi_en_echec_kiwipete() {
        // Même vérification sur Kiwipete (plus de cas spéciaux).
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut board = Board::from_fen(fen).unwrap();
        let legal_moves = generate_legal_moves(&mut board);
        assert_eq!(legal_moves.len(), 48,
            "Kiwipete : attendu 48 coups légaux, obtenus {}", legal_moves.len());
        let color = board.side_to_move;
        for mv in legal_moves {
            board.make_move(mv);
            assert!(!is_in_check(&board, color),
                "Le coup {} laisse le roi en échec !", mv.to_uci());
            board.unmake_move(mv);
        }
    }
}
