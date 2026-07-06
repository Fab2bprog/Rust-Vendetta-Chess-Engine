// =============================================================================
// Vendetta Chess Motor — src/board/state.rs
//
// Rôle : Représentation complète de l'état d'une position d'échecs.
//        C'est la structure centrale du moteur autour de laquelle tout s'articule.
//
// Contenu :
//   - CastlingRights : droits de roque encodés en 4 bits
//   - BoardState : état irréversible sauvegardé avant chaque coup
//     (pour pouvoir annuler un coup — "unmake move")
//   - Board : structure principale avec 12 bitboards, état complet, hash Zobrist
//   - Lecture/écriture FEN
//   - make_move / unmake_move
//   - Hachage Zobrist (identification unique d'une position)
//
// Choix technique : on utilise 12 bitboards (6 types × 2 couleurs) pour
// représenter toutes les pièces, plus 2 bitboards d'occupation (une par couleur)
// et 1 bitboard global pour la rapidité des opérations.
// =============================================================================

use crate::utils::types::{Color, Piece, Move, MoveFlags, file_of, rank_of, make_square, square_from_str};
use crate::board::bitboard::{
    Bitboard, set_bit, clear_bit, get_bit, lsb,
    init_attack_tables,
};
use crate::eval::tables::piece_square_values;

// =============================================================================
// Droits de roque
// =============================================================================

/// Droits de roque encodés en 4 bits :
/// - bit 0 : petit roque blanc (côté roi)
/// - bit 1 : grand roque blanc (côté dame)
/// - bit 2 : petit roque noir (côté roi)
/// - bit 3 : grand roque noir (côté dame)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CastlingRights(pub u8);

impl CastlingRights {
    pub const NONE: CastlingRights = CastlingRights(0);
    pub const ALL:  CastlingRights = CastlingRights(0b1111);

    pub const WHITE_KINGSIDE:  u8 = 0b0001;
    pub const WHITE_QUEENSIDE: u8 = 0b0010;
    pub const BLACK_KINGSIDE:  u8 = 0b0100;
    pub const BLACK_QUEENSIDE: u8 = 0b1000;

    /// Retourne true si le petit roque est disponible pour la couleur donnée.
    #[inline]
    pub fn can_castle_kingside(self, color: Color) -> bool {
        let flag = if color == Color::White {
            Self::WHITE_KINGSIDE
        } else {
            Self::BLACK_KINGSIDE
        };
        self.0 & flag != 0
    }

    /// Retourne true si le grand roque est disponible pour la couleur donnée.
    #[inline]
    pub fn can_castle_queenside(self, color: Color) -> bool {
        let flag = if color == Color::White {
            Self::WHITE_QUEENSIDE
        } else {
            Self::BLACK_QUEENSIDE
        };
        self.0 & flag != 0
    }

    /// Retire les droits de roque associés à une case (appelé quand une pièce bouge).
    #[inline]
    pub fn remove_rights_for_square(&mut self, sq: u8) {
        // Si le roi ou la tour bouge, on retire les droits correspondants.
        let mask = CASTLING_RIGHTS_MASK[sq as usize];
        self.0 &= !mask;
    }
}

/// Masque de mise à jour des droits de roque pour chaque case.
/// Si une pièce bouge depuis (ou vers) cette case, on retire ces droits.
const CASTLING_RIGHTS_MASK: [u8; 64] = {
    let mut mask = [0u8; 64];
    // Tour blanche côté roi : h1 = case 7
    mask[7]  = CastlingRights::WHITE_KINGSIDE;
    // Tour blanche côté dame : a1 = case 0
    mask[0]  = CastlingRights::WHITE_QUEENSIDE;
    // Roi blanc : e1 = case 4
    mask[4]  = CastlingRights::WHITE_KINGSIDE | CastlingRights::WHITE_QUEENSIDE;
    // Tour noire côté roi : h8 = case 63
    mask[63] = CastlingRights::BLACK_KINGSIDE;
    // Tour noire côté dame : a8 = case 56
    mask[56] = CastlingRights::BLACK_QUEENSIDE;
    // Roi noir : e8 = case 60
    mask[60] = CastlingRights::BLACK_KINGSIDE | CastlingRights::BLACK_QUEENSIDE;
    mask
};

// =============================================================================
// État irréversible (sauvegardé pour annuler un coup)
// =============================================================================

/// Informations irréversibles de la position, sauvegardées avant chaque coup.
/// Permettent de restaurer exactement l'état précédent lors d'un unmake_move.
#[derive(Clone, Copy, Debug)]
pub struct BoardState {
    /// Droits de roque avant le coup.
    pub castling: CastlingRights,
    /// Case cible de la prise en passant avant le coup (None si aucune).
    pub en_passant: Option<u8>,
    /// Compteur des demi-coups pour la règle des 50 coups avant ce coup.
    pub halfmove_clock: u32,
    /// Pièce capturée (None si coup silencieux).
    pub captured_piece: Option<Piece>,
    /// Hash Zobrist avant le coup.
    pub hash: u64,
}

// =============================================================================
// Structure principale : Board
// =============================================================================

/// Représentation complète d'une position d'échecs.
///
/// Utilise 12 bitboards (6 types de pièces × 2 couleurs) pour représenter
/// toutes les pièces. Des bitboards d'occupation dérivés permettent d'accélérer
/// les opérations fréquentes.
///
/// Clone est dérivé pour permettre au Lazy SMP de donner à chaque thread
/// sa propre copie indépendante du plateau.
#[derive(Clone)]
pub struct Board {
    /// Bitboards des pièces : pieces[couleur][type_pièce].
    /// couleur : 0=Blanc, 1=Noir
    /// type_pièce : 0=Pion, 1=Cavalier, 2=Fou, 3=Tour, 4=Dame, 5=Roi
    pub pieces: [[Bitboard; 6]; 2],

    /// Bitboard d'occupation par couleur : toutes les pièces d'une couleur.
    pub occupancy: [Bitboard; 2],

    /// Bitboard de toutes les pièces (toutes couleurs confondues).
    pub all_pieces: Bitboard,

    /// Couleur du joueur qui doit jouer.
    pub side_to_move: Color,

    /// Droits de roque actuels.
    pub castling: CastlingRights,

    /// Case cible de la prise en passant (None si aucune prise en passant possible).
    pub en_passant: Option<u8>,

    /// Compteur de demi-coups pour la règle des 50 coups.
    /// Remis à zéro après une capture ou un mouvement de pion.
    pub halfmove_clock: u32,

    /// Numéro du coup complet (commence à 1, incrémenté après le coup des Noirs).
    pub fullmove_number: u32,

    /// Hash Zobrist de la position courante (identifiant unique).
    pub hash: u64,

    /// Score incrémental matériel + PST en milieu de partie, perspective Blanc.
    /// Blanc − Noir : positif = avantage Blanc.
    /// Mis à jour dans place_piece() et remove_piece() à chaque coup.
    pub eval_mg: i32,

    /// Score incrémental matériel + PST en finale, perspective Blanc.
    /// Même convention que eval_mg.
    pub eval_eg: i32,

    /// Compteur de pièces par couleur et type : piece_count[couleur][type_pièce].
    /// Indices : couleur 0=Blanc 1=Noir ; type 0=Pion 1=Cavalier 2=Fou 3=Tour 4=Dame 5=Roi.
    /// Mis à jour dans place_piece() (+1) et remove_piece() (-1).
    ///
    /// Permet à is_insufficient_material() de remplacer 10 appels count_ones()
    /// par 10 lectures de u8 — ~10× moins cher, appelé à chaque nœud alpha-bêta.
    /// u8 suffit : maximum théorique = 9 pions après promotion, jamais > 255.
    pub piece_count: [[u8; 6]; 2],

    /// Mailbox : pièce présente sur chaque case (None = case vide), indexé par
    /// case (0..63). Maintenu incrémentalement dans place_piece()/remove_piece()
    /// — les SEULS points de mutation des bitboards de pièces (vérifié : aucune
    /// écriture directe des bitboards ailleurs dans le code).
    ///
    /// But : rendre piece_at() O(1) (une lecture indexée) au lieu d'un scan
    /// linéaire de 12 bitboards. piece_at() est sur le chemin le plus chaud du
    /// moteur (make_move, SEE, ordonnancement des coups, détection de captures)
    /// — d'où un gain de NPS, sans aucun changement de résultat (zéro Elo perdu).
    pub piece_on: [Option<(Piece, Color)>; 64],

    /// Historique des états irréversibles (une entrée par coup joué).
    pub history: Vec<BoardState>,
}

impl Board {
    /// Crée un plateau vide (aucune pièce).
    pub fn empty() -> Board {
        init_attack_tables();
        Board {
            pieces:          [[0; 6]; 2],
            occupancy:       [0; 2],
            all_pieces:      0,
            side_to_move:    Color::White,
            castling:        CastlingRights::NONE,
            en_passant:      None,
            halfmove_clock:  0,
            fullmove_number: 1,
            hash:            0,
            eval_mg:         0,
            eval_eg:         0,
            piece_count:     [[0u8; 6]; 2],
            piece_on:        [None; 64],
            history:         Vec::with_capacity(256),
        }
    }

    /// Crée un plateau avec la position initiale des échecs.
    pub fn start_position() -> Board {
        Board::from_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1")
            .expect("La position initiale FEN est valide")
    }

    // =========================================================================
    // Accès aux bitboards
    // =========================================================================

    /// Retourne la pièce présente sur la case `sq`, ou None si vide.
    ///
    /// O(1) : simple lecture du mailbox `piece_on` (maintenu incrémentalement
    /// dans place_piece()/remove_piece()). En build de développement, un
    /// debug_assert revérifie la cohérence du mailbox avec un scan des bitboards
    /// — toute désynchronisation est détectée immédiatement (notamment par perft
    /// et `cargo test`), sans aucun coût en release.
    #[inline]
    pub fn piece_at(&self, sq: u8) -> Option<(Piece, Color)> {
        debug_assert_eq!(
            self.piece_on[sq as usize],
            self.piece_at_scan(sq),
            "mailbox piece_on désynchronisé sur la case {}", sq
        );
        self.piece_on[sq as usize]
    }

    /// Scan linéaire des bitboards (ancienne implémentation de piece_at).
    /// Conservé UNIQUEMENT comme référence de cohérence pour le debug_assert de
    /// piece_at — jamais appelé en release (#[allow(dead_code)] car la référence
    /// vit dans un bloc `debug_assert!`, compilé hors en release).
    #[allow(dead_code)]
    fn piece_at_scan(&self, sq: u8) -> Option<(Piece, Color)> {
        for color in [Color::White, Color::Black] {
            for piece in [Piece::Pawn, Piece::Knight, Piece::Bishop,
                          Piece::Rook, Piece::Queen, Piece::King] {
                if get_bit(self.pieces[color.index()][piece.index()], sq) {
                    return Some((piece, color));
                }
            }
        }
        None
    }

    /// Retourne la case du roi de la couleur donnée.
    /// Précondition : le roi est présent sur le plateau (garanti par from_fen).
    pub fn king_square(&self, color: Color) -> u8 {
        let bb = self.pieces[color.index()][Piece::King.index()];
        debug_assert_ne!(bb, 0, "king_square : aucun roi {:?} sur le plateau — position invalide", color);
        lsb(bb)
    }

    // =========================================================================
    // Placement et retrait de pièces
    // =========================================================================

    /// Place une pièce sur la case `sq` et met à jour les bitboards, le hash
    /// et les scores incrémentaux eval_mg / eval_eg.
    ///
    /// eval_mg / eval_eg sont en perspective Blanc (Blanc − Noir) :
    ///   - Blanc : +mat +PST
    ///   - Noir  : −mat −PST
    ///
    /// Le roi compte pour la PST mais pas pour le matériel (incr_value = 0).
    pub fn place_piece(&mut self, color: Color, piece: Piece, sq: u8) {
        set_bit(&mut self.pieces[color.index()][piece.index()], sq);
        set_bit(&mut self.occupancy[color.index()], sq);
        set_bit(&mut self.all_pieces, sq);
        // Mailbox : la pièce est désormais présente sur `sq` (lecture O(1) par piece_at).
        self.piece_on[sq as usize] = Some((piece, color));
        self.hash ^= ZOBRIST.piece(color, piece, sq);

        // Mise à jour incrémentale du score (matériel + PST).
        let (pst_mg, pst_eg) = piece_square_values(piece, color, sq);
        let sign = if color == Color::White { 1i32 } else { -1i32 };
        self.eval_mg += sign * (piece.incr_value() + pst_mg);
        self.eval_eg += sign * (piece.incr_value() + pst_eg);

        // Compteur de pièces (pour is_insufficient_material O(1)).
        self.piece_count[color.index()][piece.index()] += 1;
    }

    /// Retire une pièce de la case `sq` et met à jour les bitboards, le hash
    /// et les scores incrémentaux eval_mg / eval_eg.
    pub fn remove_piece(&mut self, color: Color, piece: Piece, sq: u8) {
        clear_bit(&mut self.pieces[color.index()][piece.index()], sq);
        clear_bit(&mut self.occupancy[color.index()], sq);
        clear_bit(&mut self.all_pieces, sq);
        // Mailbox : la case `sq` est désormais vide.
        self.piece_on[sq as usize] = None;
        self.hash ^= ZOBRIST.piece(color, piece, sq);

        // Annulation de la contribution incrémentale.
        let (pst_mg, pst_eg) = piece_square_values(piece, color, sq);
        let sign = if color == Color::White { 1i32 } else { -1i32 };
        self.eval_mg -= sign * (piece.incr_value() + pst_mg);
        self.eval_eg -= sign * (piece.incr_value() + pst_eg);

        // Compteur de pièces (pour is_insufficient_material O(1)).
        // Invariant : le compteur doit être > 0 avant de décrémenter.
        // En mode release, le u8 wrappe silencieusement → debug_assert! pour détecter
        // toute corruption dès le développement, sans coût en production.
        debug_assert!(
            self.piece_count[color.index()][piece.index()] > 0,
            "remove_piece : piece_count[{:?}][{:?}] est déjà 0 — double suppression ?",
            color, piece
        );
        self.piece_count[color.index()][piece.index()] -= 1;
    }

    /// Déplace une pièce de `from` vers `to` sans mise à jour du hash (usage interne).
    fn move_piece_internal(&mut self, color: Color, piece: Piece, from: u8, to: u8) {
        self.remove_piece(color, piece, from);
        self.place_piece(color, piece, to);
    }

    // =========================================================================
    // Lecture FEN
    // =========================================================================

    /// Crée un plateau depuis une chaîne FEN.
    /// Retourne Err(message) si la FEN est invalide.
    pub fn from_fen(fen: &str) -> Result<Board, String> {
        let mut board = Board::empty();
        let parts: Vec<&str> = fen.split_whitespace().collect();

        if parts.len() < 4 {
            return Err(format!("FEN invalide : pas assez de champs (reçu {})", parts.len()));
        }

        // --- Champ 1 : placement des pièces ---
        let mut rank: i32 = 7;
        let mut file: i32 = 0;

        for c in parts[0].chars() {
            match c {
                '/' => {
                    rank -= 1;
                    file = 0;
                    if rank < 0 {
                        return Err("FEN invalide : trop de rangs".to_string());
                    }
                }
                '1'..='8' => {
                    file += (c as i32) - ('0' as i32);
                }
                _ => {
                    if let Some((piece, color)) = Piece::from_fen_char(c) {
                        if file > 7 || rank < 0 {
                            return Err("FEN invalide : case hors limites".to_string());
                        }
                        let sq = make_square(file as u8, rank as u8);
                        board.place_piece(color, piece, sq);
                        file += 1;
                    } else {
                        return Err(format!("FEN invalide : caractère inconnu '{}'", c));
                    }
                }
            }
        }

        // --- Validation : exactement un roi par camp ---
        // Nécessaire pour garantir que king_square() retourne toujours une case valide (0-63).
        // Sans cette vérification, un FEN malformé provoquerait un crash en pleine partie.
        let white_kings = board.pieces[Color::White.index()][Piece::King.index()].count_ones();
        let black_kings = board.pieces[Color::Black.index()][Piece::King.index()].count_ones();
        if white_kings != 1 {
            return Err(format!(
                "FEN invalide : {} roi(s) blanc(s) trouvé(s), exactement 1 requis",
                white_kings
            ));
        }
        if black_kings != 1 {
            return Err(format!(
                "FEN invalide : {} roi(s) noir(s) trouvé(s), exactement 1 requis",
                black_kings
            ));
        }

        // --- Champ 2 : trait ---
        board.side_to_move = match parts[1] {
            "w" => Color::White,
            "b" => Color::Black,
            _   => return Err(format!("FEN invalide : trait '{}' inconnu", parts[1])),
        };
        if board.side_to_move == Color::Black {
            board.hash ^= ZOBRIST.side;
        }

        // --- Champ 3 : droits de roque ---
        board.castling = CastlingRights::NONE;
        if parts[2] != "-" {
            for c in parts[2].chars() {
                match c {
                    'K' => board.castling.0 |= CastlingRights::WHITE_KINGSIDE,
                    'Q' => board.castling.0 |= CastlingRights::WHITE_QUEENSIDE,
                    'k' => board.castling.0 |= CastlingRights::BLACK_KINGSIDE,
                    'q' => board.castling.0 |= CastlingRights::BLACK_QUEENSIDE,
                    '-' => {}
                    _   => return Err(format!("FEN invalide : roque '{}' inconnu", c)),
                }
            }
        }

        // --- Validation des droits de roque ---
        //
        // Pour chaque droit actif, on vérifie que le roi ET la tour requise sont bien
        // présents sur leurs cases initiales standard (échecs classiques) :
        //
        //   K → Roi blanc en e1 (sq 4)  + Tour blanche en h1 (sq 7)
        //   Q → Roi blanc en e1 (sq 4)  + Tour blanche en a1 (sq 0)
        //   k → Roi noir  en e8 (sq 60) + Tour noire  en h8 (sq 63)
        //   q → Roi noir  en e8 (sq 60) + Tour noire  en a8 (sq 56)
        //
        // Stratégie : retrait silencieux du droit invalide plutôt que Err().
        //
        //   Raison : de nombreuses GUIs (Arena, Cutechess…) envoient des FEN avec
        //   des droits de roque résiduels (ex : "KQkq" alors que la tour a1 a bougé
        //   puis est revenue). Retourner Err() bloquerait le moteur sur un coup légal.
        //   On retire le droit invalide et le moteur continue proprement.
        //
        //   En build de développement (`debug_assert!`) l'incohérence est signalée
        //   immédiatement sans aucun coût en release.
        //
        // IMPORTANT : le hash Zobrist est calculé APRÈS cette correction pour refléter
        // les droits réels (pas ceux du FEN brut). Un hash calculé sur des droits
        // incorrects produirait de faux hits en table de transposition.
        {
            let w_rooks = board.pieces[Color::White.index()][Piece::Rook.index()];
            let b_rooks = board.pieces[Color::Black.index()][Piece::Rook.index()];
            let w_kings = board.pieces[Color::White.index()][Piece::King.index()];
            let b_kings = board.pieces[Color::Black.index()][Piece::King.index()];

            // Roi blanc sur e1 (sq 4) ?
            let white_king_on_e1 = get_bit(w_kings, 4);
            // Roi noir sur e8 (sq 60) ?
            let black_king_on_e8 = get_bit(b_kings, 60);

            if board.castling.0 & CastlingRights::WHITE_KINGSIDE != 0 {
                let rook_on_h1 = get_bit(w_rooks, 7);
                if !white_king_on_e1 || !rook_on_h1 {
                    debug_assert!(false,
                        "FEN : droit 'K' (petit roque blanc) invalide — \
                         roi blanc en e1={}, tour blanche en h1={}. Droit retiré.",
                        white_king_on_e1, rook_on_h1
                    );
                    board.castling.0 &= !CastlingRights::WHITE_KINGSIDE;
                }
            }

            if board.castling.0 & CastlingRights::WHITE_QUEENSIDE != 0 {
                let rook_on_a1 = get_bit(w_rooks, 0);
                if !white_king_on_e1 || !rook_on_a1 {
                    debug_assert!(false,
                        "FEN : droit 'Q' (grand roque blanc) invalide — \
                         roi blanc en e1={}, tour blanche en a1={}. Droit retiré.",
                        white_king_on_e1, rook_on_a1
                    );
                    board.castling.0 &= !CastlingRights::WHITE_QUEENSIDE;
                }
            }

            if board.castling.0 & CastlingRights::BLACK_KINGSIDE != 0 {
                let rook_on_h8 = get_bit(b_rooks, 63);
                if !black_king_on_e8 || !rook_on_h8 {
                    debug_assert!(false,
                        "FEN : droit 'k' (petit roque noir) invalide — \
                         roi noir en e8={}, tour noire en h8={}. Droit retiré.",
                        black_king_on_e8, rook_on_h8
                    );
                    board.castling.0 &= !CastlingRights::BLACK_KINGSIDE;
                }
            }

            if board.castling.0 & CastlingRights::BLACK_QUEENSIDE != 0 {
                let rook_on_a8 = get_bit(b_rooks, 56);
                if !black_king_on_e8 || !rook_on_a8 {
                    debug_assert!(false,
                        "FEN : droit 'q' (grand roque noir) invalide — \
                         roi noir en e8={}, tour noire en a8={}. Droit retiré.",
                        black_king_on_e8, rook_on_a8
                    );
                    board.castling.0 &= !CastlingRights::BLACK_QUEENSIDE;
                }
            }
        }

        // Hash calculé sur les droits corrigés (pas les droits bruts du FEN).
        board.hash ^= ZOBRIST.castling(board.castling);

        // --- Champ 4 : prise en passant ---
        board.en_passant = if parts[3] == "-" {
            None
        } else {
            let sq = square_from_str(parts[3])
                .ok_or_else(|| format!("FEN invalide : case en passant '{}'", parts[3]))?;

            // Validation du rang : la case en passant doit être sur le rang 3 (index 2,
            // noir vient de pousser deux cases, case cible pour les blancs) ou le rang 6
            // (index 5, blanc vient de pousser, case cible pour les noirs).
            // Un rang incorrect produirait une corruption silencieuse lors de la prise en passant
            // (to - 8 ou to + 8 pointerait hors de la zone attendue).
            let ep_rank = rank_of(sq);
            if ep_rank != 2 && ep_rank != 5 {
                return Err(format!(
                    "FEN invalide : case en passant '{}' sur le rang {} (attendu 3 ou 6)",
                    parts[3],
                    ep_rank + 1
                ));
            }

            board.hash ^= ZOBRIST.en_passant(file_of(sq));
            Some(sq)
        };

        // --- Champ 5 : compteur 50 coups (optionnel) ---
        if parts.len() > 4 {
            board.halfmove_clock = parts[4].parse::<u32>()
                .map_err(|_| format!("FEN invalide : compteur 50 coups '{}'", parts[4]))?;
        }

        // --- Champ 6 : numéro du coup complet (optionnel) ---
        if parts.len() > 5 {
            board.fullmove_number = parts[5].parse::<u32>()
                .map_err(|_| format!("FEN invalide : numéro de coup '{}'", parts[5]))?;
        }

        Ok(board)
    }

    /// Génère la chaîne FEN de la position actuelle.
    pub fn to_fen(&self) -> String {
        let mut fen = String::new();

        // Placement des pièces.
        // Invariant : `empty` est compté case par case sur 8 colonnes → max 8.
        // `char::from_digit(n, 10)` retourne None uniquement si n >= 10.
        // Puisque empty ∈ [1, 8], la conversion est toujours valide.
        // On utilise `(b'0' + empty as u8) as char` pour documenter l'invariant
        // explicitement et éviter tout unwrap superflu.
        for rank in (0..8).rev() {
            let mut empty = 0u8; // u8 suffit : max 8 cases vides par rang
            for file in 0..8u8 {
                let sq = make_square(file, rank);
                if let Some((piece, color)) = self.piece_at(sq) {
                    if empty > 0 {
                        debug_assert!(empty <= 8, "to_fen: empty={} > 8, impossible", empty);
                        fen.push((b'0' + empty) as char);
                        empty = 0;
                    }
                    fen.push(piece.to_fen_char(color));
                } else {
                    empty += 1;
                }
            }
            if empty > 0 {
                debug_assert!(empty <= 8, "to_fen: empty={} > 8, impossible", empty);
                fen.push((b'0' + empty) as char);
            }
            if rank > 0 { fen.push('/'); }
        }

        // Trait
        fen.push(' ');
        fen.push(if self.side_to_move == Color::White { 'w' } else { 'b' });

        // Droits de roque
        fen.push(' ');
        if self.castling.0 == 0 {
            fen.push('-');
        } else {
            if self.castling.0 & CastlingRights::WHITE_KINGSIDE  != 0 { fen.push('K'); }
            if self.castling.0 & CastlingRights::WHITE_QUEENSIDE != 0 { fen.push('Q'); }
            if self.castling.0 & CastlingRights::BLACK_KINGSIDE  != 0 { fen.push('k'); }
            if self.castling.0 & CastlingRights::BLACK_QUEENSIDE != 0 { fen.push('q'); }
        }

        // En passant
        fen.push(' ');
        match self.en_passant {
            None     => fen.push('-'),
            Some(sq) => {
                fen.push((b'a' + file_of(sq)) as char);
                fen.push((b'1' + rank_of(sq)) as char);
            }
        }

        // Compteurs
        fen.push_str(&format!(" {} {}", self.halfmove_clock, self.fullmove_number));

        fen
    }

    // =========================================================================
    // Jouer et annuler un coup
    // =========================================================================

    /// Joue le coup `mv` sur le plateau.
    /// L'état irréversible est sauvegardé dans `self.history` pour pouvoir
    /// annuler le coup avec unmake_move.
    pub fn make_move(&mut self, mv: Move) {
        let color   = self.side_to_move;
        let enemy   = color.opposite();
        let from    = mv.from;
        let to      = mv.to;

        // Récupérer la pièce qui bouge.
        // Invariant : from doit toujours contenir une pièce (garantie par le générateur légal).
        // En debug on panique immédiatement pour détecter toute corruption ; en release on
        // retourne proprement plutôt que de corrompre l'état du plateau.
        let piece = match self.piece_at(from) {
            Some((p, _)) => p,
            None => {
                debug_assert!(false, "make_move : aucune pièce sur la case de départ {}", from);
                return;
            }
        };

        // Récupérer la pièce capturée (si capture normale)
        let captured = if mv.flags.is_capture() && mv.flags != MoveFlags::EnPassant {
            self.piece_at(to).map(|(p, _)| p)
        } else {
            None
        };

        // Sauvegarder l'état irréversible
        self.history.push(BoardState {
            castling:        self.castling,
            en_passant:      self.en_passant,
            halfmove_clock:  self.halfmove_clock,
            captured_piece:  captured,
            hash:            self.hash,
        });

        // Mise à jour du hash : retirer l'ancien en passant et roque
        if let Some(ep_sq) = self.en_passant {
            self.hash ^= ZOBRIST.en_passant(file_of(ep_sq));
        }
        self.hash ^= ZOBRIST.castling(self.castling);

        // Réinitialiser la case en passant
        self.en_passant = None;

        // Mise à jour des droits de roque
        self.castling.remove_rights_for_square(from);
        self.castling.remove_rights_for_square(to);
        self.hash ^= ZOBRIST.castling(self.castling);

        // Mise à jour du compteur des 50 coups
        if piece == Piece::Pawn || mv.flags.is_capture() {
            self.halfmove_clock = 0;
        } else {
            self.halfmove_clock += 1;
        }

        match mv.flags {
            MoveFlags::Quiet => {
                // Déplacement simple
                self.move_piece_internal(color, piece, from, to);
                // Poussée de deux cases : mettre à jour la case en passant
                // (géré par DoublePush ci-dessous)
            }

            MoveFlags::DoublePush => {
                // Poussée de deux cases du pion
                self.move_piece_internal(color, Piece::Pawn, from, to);
                // La case en passant est entre from et to
                let ep_sq = if color == Color::White { from + 8 } else { from - 8 };
                self.en_passant = Some(ep_sq);
                self.hash ^= ZOBRIST.en_passant(file_of(ep_sq));
            }

            MoveFlags::Capture => {
                // Retirer la pièce capturée
                if let Some(cap) = captured {
                    self.remove_piece(enemy, cap, to);
                }
                self.move_piece_internal(color, piece, from, to);
            }

            MoveFlags::EnPassant => {
                // La pièce capturée est sur la case adjacente, pas sur `to`
                let cap_sq = if color == Color::White { to - 8 } else { to + 8 };
                self.remove_piece(enemy, Piece::Pawn, cap_sq);
                self.move_piece_internal(color, Piece::Pawn, from, to);
            }

            MoveFlags::CastleKingside => {
                // Déplacer le roi et la tour (côté roi)
                self.move_piece_internal(color, Piece::King, from, to);
                let (rook_from, rook_to) = if color == Color::White {
                    (7u8, 5u8)   // h1 → f1
                } else {
                    (63u8, 61u8) // h8 → f8
                };
                self.move_piece_internal(color, Piece::Rook, rook_from, rook_to);
            }

            MoveFlags::CastleQueenside => {
                // Déplacer le roi et la tour (côté dame)
                self.move_piece_internal(color, Piece::King, from, to);
                let (rook_from, rook_to) = if color == Color::White {
                    (0u8, 3u8)   // a1 → d1
                } else {
                    (56u8, 59u8) // a8 → d8
                };
                self.move_piece_internal(color, Piece::Rook, rook_from, rook_to);
            }

            MoveFlags::Promotion => {
                // Retirer le pion et placer la pièce de promotion
                self.remove_piece(color, Piece::Pawn, from);
                let promo = mv.promotion_piece().unwrap_or(Piece::Queen);
                self.place_piece(color, promo, to);
            }

            MoveFlags::PromotionCapture => {
                // Retirer la pièce capturée, retirer le pion, placer la promotion
                if let Some(cap) = captured {
                    self.remove_piece(enemy, cap, to);
                }
                self.remove_piece(color, Piece::Pawn, from);
                let promo = mv.promotion_piece().unwrap_or(Piece::Queen);
                self.place_piece(color, promo, to);
            }
        }

        // Changer le joueur actif
        self.side_to_move = enemy;
        self.hash ^= ZOBRIST.side;

        // Incrémenter le numéro du coup complet après les Noirs
        if color == Color::Black {
            self.fullmove_number += 1;
        }
    }

    /// Annule le dernier coup joué et restaure l'état précédent.
    /// Précondition : make_move a été appelé au moins une fois.
    pub fn unmake_move(&mut self, mv: Move) {
        // Restaurer le joueur actif (on revient au joueur qui venait de jouer)
        self.side_to_move = self.side_to_move.opposite();
        let color = self.side_to_move;
        let enemy = color.opposite();
        let from  = mv.from;
        let to    = mv.to;

        // Décrémenter le numéro du coup si c'était les Noirs qui avaient joué
        if color == Color::Black {
            self.fullmove_number -= 1;
        }

        // Restaurer l'état irréversible.
        // Invariant : history ne doit jamais être vide ici (make/unmake toujours symétriques).
        let state = match self.history.pop() {
            Some(s) => s,
            None    => {
                debug_assert!(false, "unmake_move : historique vide — make/unmake asymétriques");
                return;
            }
        };
        self.castling       = state.castling;
        self.en_passant     = state.en_passant;
        self.halfmove_clock = state.halfmove_clock;
        self.hash           = state.hash;

        // Récupérer la pièce qui était sur `to`.
        // Pour les promotions, on sait que c'était un pion (la pièce promue a été
        // retirée par remove_piece dans make_move, mais le bitboard Pion n'a pas encore
        // été restauré — on utilise directement Piece::Pawn).
        let piece = match mv.flags {
            MoveFlags::Promotion | MoveFlags::PromotionCapture => Piece::Pawn,
            _ => match self.piece_at(to).map(|(p, _)| p) {
                Some(p) => p,
                None    => {
                    debug_assert!(false,
                        "unmake_move : aucune pièce sur la case d'arrivée {} — bitboards incohérents", to);
                    return;
                }
            },
        };

        match mv.flags {
            MoveFlags::Quiet | MoveFlags::DoublePush => {
                // Déplacement simple : remettre la pièce à sa place
                self.move_piece_internal(color, piece, to, from);
            }

            MoveFlags::Capture => {
                // Remettre la pièce à sa place et restaurer la pièce capturée
                self.move_piece_internal(color, piece, to, from);
                if let Some(cap) = state.captured_piece {
                    self.place_piece(enemy, cap, to);
                }
            }

            MoveFlags::EnPassant => {
                // Remettre le pion à sa place et restaurer le pion capturé en passant
                self.move_piece_internal(color, Piece::Pawn, to, from);
                let cap_sq = if color == Color::White { to - 8 } else { to + 8 };
                self.place_piece(enemy, Piece::Pawn, cap_sq);
            }

            MoveFlags::CastleKingside => {
                // Remettre le roi et la tour à leur place
                self.move_piece_internal(color, Piece::King, to, from);
                let (rook_from, rook_to) = if color == Color::White {
                    (7u8, 5u8)
                } else {
                    (63u8, 61u8)
                };
                self.move_piece_internal(color, Piece::Rook, rook_to, rook_from);
            }

            MoveFlags::CastleQueenside => {
                // Remettre le roi et la tour à leur place
                self.move_piece_internal(color, Piece::King, to, from);
                let (rook_from, rook_to) = if color == Color::White {
                    (0u8, 3u8)
                } else {
                    (56u8, 59u8)
                };
                self.move_piece_internal(color, Piece::Rook, rook_to, rook_from);
            }

            MoveFlags::Promotion => {
                // Retirer la pièce de promotion et remettre le pion
                let promo = mv.promotion_piece().unwrap_or(Piece::Queen);
                self.remove_piece(color, promo, to);
                self.place_piece(color, Piece::Pawn, from);
            }

            MoveFlags::PromotionCapture => {
                // Retirer la pièce de promotion, remettre le pion et la pièce capturée
                let promo = mv.promotion_piece().unwrap_or(Piece::Queen);
                self.remove_piece(color, promo, to);
                self.place_piece(color, Piece::Pawn, from);
                if let Some(cap) = state.captured_piece {
                    self.place_piece(enemy, cap, to);
                }
            }
        }
    }

    /// Joue un coup "nul" (le joueur passe son tour).
    /// Utilisé dans la recherche pour le null move pruning.
    /// Retourne la case en passant précédente pour pouvoir annuler.
    pub fn make_null_move(&mut self) -> Option<u8> {
        let prev_ep = self.en_passant;

        // Retirer l'en passant du hash
        if let Some(ep_sq) = self.en_passant {
            self.hash ^= ZOBRIST.en_passant(file_of(ep_sq));
        }
        self.en_passant = None;

        // Changer le trait
        self.side_to_move = self.side_to_move.opposite();
        self.hash ^= ZOBRIST.side;
        self.halfmove_clock += 1;

        prev_ep
    }

    /// Annule un coup nul.
    pub fn unmake_null_move(&mut self, prev_ep: Option<u8>) {
        self.side_to_move = self.side_to_move.opposite();
        self.hash ^= ZOBRIST.side;
        self.halfmove_clock -= 1;

        // Restaurer l'en passant
        if let Some(ep_sq) = prev_ep {
            self.hash ^= ZOBRIST.en_passant(file_of(ep_sq));
        }
        self.en_passant = prev_ep;
    }
}

// =============================================================================
// Hachage Zobrist
//
// Le hachage Zobrist permet d'identifier une position de façon (quasi) unique
// avec un entier u64. Il est mis à jour incrémentalement à chaque coup joué,
// ce qui est très efficace. Utilisé par la table de transposition.
// =============================================================================

/// Table des nombres aléatoires Zobrist, initialisée statiquement.
pub struct ZobristTable {
    /// Nombre aléatoire pour chaque combinaison (couleur, pièce, case).
    pub pieces:     [[[u64; 64]; 6]; 2],
    /// Nombre aléatoire pour le trait (Noir joue).
    pub side:       u64,
    /// Nombres aléatoires pour les droits de roque (16 combinaisons possibles).
    pub castling:   [u64; 16],
    /// Nombres aléatoires pour la colonne de la case en passant (8 colonnes).
    pub en_passant: [u64; 8],
}

impl ZobristTable {
    /// Retourne le hash pour une pièce sur une case.
    #[inline]
    pub fn piece(&self, color: Color, piece: Piece, sq: u8) -> u64 {
        self.pieces[color.index()][piece.index()][sq as usize]
    }

    /// Retourne le hash pour les droits de roque.
    #[inline]
    pub fn castling(&self, rights: CastlingRights) -> u64 {
        self.castling[(rights.0 & 0xF) as usize]
    }

    /// Retourne le hash pour la colonne de la case en passant.
    #[inline]
    pub fn en_passant(&self, file: u8) -> u64 {
        self.en_passant[file as usize]
    }
}

/// Génère un nombre pseudo-aléatoire u64 depuis une graine (xorshift64).
/// Utilisé uniquement pour initialiser la table Zobrist.
const fn xorshift64(seed: u64) -> u64 {
    let x = seed ^ (seed << 13);
    let x = x ^ (x >> 7);
    x ^ (x << 17)
}

/// Table Zobrist initialisée à la compilation avec des constantes déterministes.
/// On utilise un générateur xorshift pour produire des valeurs bien distribuées.
pub static ZOBRIST: ZobristTable = {
    let mut pieces     = [[[0u64; 64]; 6]; 2];
    let mut castling   = [0u64; 16];
    let mut en_passant = [0u64; 8];
    let mut seed: u64  = 0x123456789ABCDEF0;

    // Remplir les nombres pour les pièces
    let mut c = 0usize;
    while c < 2 {
        let mut p = 0usize;
        while p < 6 {
            let mut s = 0usize;
            while s < 64 {
                seed = xorshift64(seed);
                pieces[c][p][s] = seed;
                s += 1;
            }
            p += 1;
        }
        c += 1;
    }

    // Remplir les nombres pour le trait
    seed = xorshift64(seed);
    let side = seed;

    // Remplir les nombres pour le roque
    let mut i = 0usize;
    while i < 16 {
        seed = xorshift64(seed);
        castling[i] = seed;
        i += 1;
    }

    // Remplir les nombres pour l'en passant
    let mut i = 0usize;
    while i < 8 {
        seed = xorshift64(seed);
        en_passant[i] = seed;
        i += 1;
    }

    ZobristTable { pieces, side, castling, en_passant }
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_initiale_fen() {
        let board = Board::start_position();
        // Les Blancs jouent en premier
        assert_eq!(board.side_to_move, Color::White);
        // 8 pions blancs
        assert_eq!(board.pieces[Color::White.index()][Piece::Pawn.index()].count_ones(), 8);
        // 8 pions noirs
        assert_eq!(board.pieces[Color::Black.index()][Piece::Pawn.index()].count_ones(), 8);
        // Tous les droits de roque
        assert_eq!(board.castling.0, CastlingRights::ALL.0);
        // Pas d'en passant
        assert!(board.en_passant.is_none());
    }

    #[test]
    fn test_fen_aller_retour() {
        let fen = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
        let board = Board::from_fen(fen).unwrap();
        assert_eq!(board.to_fen(), fen);
    }

    #[test]
    fn test_hash_unique() {
        let b1 = Board::start_position();
        let b2 = Board::from_fen("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1").unwrap();
        assert_ne!(b1.hash, b2.hash);
    }
}
