// =============================================================================
// Vendetta Chess Motor — src/utils/types.rs
//
// Rôle : Définit tous les types fondamentaux partagés par l'ensemble du projet.
//        Ce fichier est la base de tout — Color, Piece, Square, Move et les
//        constantes de score. Aucun autre module n'est importé ici.
//
// Contenu :
//   - Color : couleur d'une pièce ou d'un joueur (Blanc / Noir)
//   - Piece : type de pièce (Pion, Cavalier, Fou, Tour, Dame, Roi)
//   - MoveFlags : type de coup (normal, capture, roque, promotion, etc.)
//   - Move : représentation d'un coup
//   - Constantes : valeurs de score, indices de pièces
//
// Convention des cases (Square) :
//   - u8, valeur 0 à 63
//   - sq = rang * 8 + colonne
//   - Colonne : 0=a, 1=b, ..., 7=h
//   - Rang    : 0=rang1, 1=rang2, ..., 7=rang8
//   - Exemple : e4 → colonne 4, rang 3 → sq = 3*8+4 = 28
// =============================================================================

/// Couleur d'un joueur ou d'une pièce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    White = 0,
    Black = 1,
}

impl Color {
    /// Retourne la couleur opposée.
    #[inline]
    pub fn opposite(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }

    /// Retourne l'index numérique (0 pour Blanc, 1 pour Noir).
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Type de pièce aux échecs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    Pawn   = 0,
    Knight = 1,
    Bishop = 2,
    Rook   = 3,
    Queen  = 4,
    King   = 5,
}

impl Piece {
    /// Retourne l'index numérique de la pièce (0 à 5).
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Crée une pièce depuis son index numérique.
    /// Retourne None si l'index est invalide.
    pub fn from_index(i: usize) -> Option<Piece> {
        match i {
            0 => Some(Piece::Pawn),
            1 => Some(Piece::Knight),
            2 => Some(Piece::Bishop),
            3 => Some(Piece::Rook),
            4 => Some(Piece::Queen),
            5 => Some(Piece::King),
            _ => None,
        }
    }

    /// Retourne le caractère FEN de la pièce (majuscule = Blanc, minuscule = Noir).
    pub fn to_fen_char(self, color: Color) -> char {
        let c = match self {
            Piece::Pawn   => 'p',
            Piece::Knight => 'n',
            Piece::Bishop => 'b',
            Piece::Rook   => 'r',
            Piece::Queen  => 'q',
            Piece::King   => 'k',
        };
        if color == Color::White { c.to_ascii_uppercase() } else { c }
    }

    /// Crée une pièce depuis un caractère FEN.
    pub fn from_fen_char(c: char) -> Option<(Piece, Color)> {
        let color = if c.is_uppercase() { Color::White } else { Color::Black };
        let piece = match c.to_ascii_lowercase() {
            'p' => Piece::Pawn,
            'n' => Piece::Knight,
            'b' => Piece::Bishop,
            'r' => Piece::Rook,
            'q' => Piece::Queen,
            'k' => Piece::King,
            _   => return None,
        };
        Some((piece, color))
    }

    /// Valeur matérielle incrémentale de la pièce, utilisée par board::state
    /// pour maintenir eval_mg / eval_eg en temps réel.
    ///
    /// Le roi retourne 0 : il n'est PAS comptabilisé dans le matériel
    /// (cohérent avec material_eval qui l'exclut explicitement), mais il
    /// contribue tout de même à la PST via piece_square_values().
    ///
    /// Les valeurs sont identiques à eval::material::PIECE_VALUE[0..5].
    #[inline]
    pub fn incr_value(self) -> i32 {
        match self {
            Piece::Pawn   => 100,
            Piece::Knight => 320,
            Piece::Bishop => 330,
            Piece::Rook   => 500,
            Piece::Queen  => 900,
            Piece::King   =>   0, // Non comptabilisé dans le matériel.
        }
    }
}

/// Indicateurs décrivant le type de coup joué.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveFlags {
    /// Coup silencieux (déplacement simple sans capture).
    Quiet,
    /// Poussée de deux cases du pion depuis la position initiale.
    DoublePush,
    /// Petit roque (côté roi).
    CastleKingside,
    /// Grand roque (côté dame).
    CastleQueenside,
    /// Capture simple.
    Capture,
    /// Prise en passant.
    EnPassant,
    /// Promotion sans capture.
    Promotion,
    /// Promotion avec capture.
    PromotionCapture,
}

impl MoveFlags {
    /// Retourne true si le coup est une capture (y compris en passant et promotion-capture).
    #[inline]
    pub fn is_capture(self) -> bool {
        matches!(self, MoveFlags::Capture | MoveFlags::EnPassant | MoveFlags::PromotionCapture)
    }

    /// Retourne true si le coup est une promotion.
    #[inline]
    pub fn is_promotion(self) -> bool {
        matches!(self, MoveFlags::Promotion | MoveFlags::PromotionCapture)
    }

    /// Retourne true si le coup est un roque.
    #[inline]
    pub fn is_castle(self) -> bool {
        matches!(self, MoveFlags::CastleKingside | MoveFlags::CastleQueenside)
    }
}

/// Représentation d'un coup aux échecs.
///
/// Structure compacte (4 octets) copiable efficacement.
/// La case `promotion` vaut 0 si ce n'est pas une promotion,
/// sinon l'index de la pièce de promotion (1=Cavalier, 2=Fou, 3=Tour, 4=Dame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    /// Case de départ (0-63).
    pub from: u8,
    /// Case d'arrivée (0-63).
    pub to: u8,
    /// Type du coup.
    pub flags: MoveFlags,
    /// Pièce de promotion : 0=aucune, 1=Cavalier, 2=Fou, 3=Tour, 4=Dame.
    pub promotion: u8,
}

impl Move {
    /// Crée un coup silencieux (déplacement simple).
    #[inline]
    pub fn quiet(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::Quiet, promotion: 0 }
    }

    /// Crée une capture simple.
    #[inline]
    pub fn capture(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::Capture, promotion: 0 }
    }

    /// Crée une poussée de deux cases.
    #[inline]
    pub fn double_push(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::DoublePush, promotion: 0 }
    }

    /// Crée une prise en passant.
    #[inline]
    pub fn en_passant(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::EnPassant, promotion: 0 }
    }

    /// Crée un petit roque.
    #[inline]
    pub fn castle_kingside(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::CastleKingside, promotion: 0 }
    }

    /// Crée un grand roque.
    #[inline]
    pub fn castle_queenside(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::CastleQueenside, promotion: 0 }
    }

    /// Crée une promotion sans capture.
    /// `promo` : 1=Cavalier, 2=Fou, 3=Tour, 4=Dame.
    #[inline]
    pub fn promotion(from: u8, to: u8, promo: u8) -> Move {
        Move { from, to, flags: MoveFlags::Promotion, promotion: promo }
    }

    /// Crée une promotion avec capture.
    #[inline]
    pub fn promotion_capture(from: u8, to: u8, promo: u8) -> Move {
        Move { from, to, flags: MoveFlags::PromotionCapture, promotion: promo }
    }

    /// Retourne la pièce de promotion, ou None si ce n'est pas une promotion.
    pub fn promotion_piece(self) -> Option<Piece> {
        if !self.flags.is_promotion() { return None; }
        match self.promotion {
            1 => Some(Piece::Knight),
            2 => Some(Piece::Bishop),
            3 => Some(Piece::Rook),
            4 => Some(Piece::Queen),
            _ => None,
        }
    }

    /// Retourne la notation UCI du coup (ex: "e2e4", "e7e8q").
    pub fn to_uci(self) -> String {
        let from_col = self.from % 8;
        let from_row = self.from / 8;
        let to_col   = self.to % 8;
        let to_row   = self.to / 8;

        let mut s = String::new();
        s.push((b'a' + from_col) as char);
        s.push((b'1' + from_row) as char);
        s.push((b'a' + to_col) as char);
        s.push((b'1' + to_row) as char);

        if self.flags.is_promotion() {
            let promo_char = match self.promotion {
                1 => 'n',
                2 => 'b',
                3 => 'r',
                4 => 'q',
                _ => 'q',
            };
            s.push(promo_char);
        }
        s
    }

    /// Coup nul (utilisé comme valeur par défaut / sentinelle).
    ///
    /// Représenté par `(from=0, to=0)` — la case a1 vers a1, qui n'est jamais
    /// un coup légal dans les échecs. Le générateur légal ne produit jamais de coup
    /// avec `from == to`, donc la collision est impossible en pratique.
    ///
    /// Remarque : si un jour le moteur devait supporter des variantes où a1→a1
    /// serait encodable, il faudrait ajouter un champ `is_null: bool` ou utiliser
    /// une valeur de case hors-plage (ex: 255).
    pub const NULL: Move = Move { from: 0, to: 0, flags: MoveFlags::Quiet, promotion: 0 };

    /// Retourne true si c'est le coup nul sentinelle.
    ///
    /// Invariant garanti par le générateur légal : aucun coup légal n'a `from == to`.
    /// Cette détection est donc fiable dans tous les chemins de code normaux.
    #[inline]
    pub fn is_null(self) -> bool {
        self.from == 0 && self.to == 0
    }
}

// =============================================================================
// Constantes globales
// =============================================================================

/// Score infini (utilisé comme borne dans la recherche alpha-bêta).
pub const SCORE_INF: i32 = 1_000_000;

/// Score d'un échec et mat. On soustrait la profondeur pour préférer les mats rapides.
pub const SCORE_MATE: i32 = 900_000;

/// Score nul (position égale).
pub const SCORE_DRAW: i32 = 0;

/// Nombre de types de pièces.
pub const NUM_PIECES: usize = 6;

/// Nombre de couleurs.
pub const NUM_COLORS: usize = 2;

/// Nombre de cases sur l'échiquier.
pub const NUM_SQUARES: usize = 64;

// =============================================================================
// Fonctions utilitaires sur les cases
// =============================================================================

/// Retourne la colonne d'une case (0=a, 7=h).
#[inline]
pub fn file_of(sq: u8) -> u8 {
    sq % 8
}

/// Retourne le rang d'une case (0=rang1, 7=rang8).
#[inline]
pub fn rank_of(sq: u8) -> u8 {
    sq / 8
}

/// Crée une case depuis colonne et rang.
#[inline]
pub fn make_square(file: u8, rank: u8) -> u8 {
    rank * 8 + file
}

/// Convertit une notation algébrique (ex: "e4") en numéro de case.
pub fn square_from_str(s: &str) -> Option<u8> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 { return None; }
    let file = bytes[0].wrapping_sub(b'a');
    let rank = bytes[1].wrapping_sub(b'1');
    if file > 7 || rank > 7 { return None; }
    Some(make_square(file, rank))
}

/// Convertit un numéro de case en notation algébrique (ex: 28 → "e4").
pub fn square_to_str(sq: u8) -> String {
    let file = file_of(sq);
    let rank = rank_of(sq);
    let mut s = String::new();
    s.push((b'a' + file) as char);
    s.push((b'1' + rank) as char);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_couleur_opposee() {
        assert_eq!(Color::White.opposite(), Color::Black);
        assert_eq!(Color::Black.opposite(), Color::White);
    }

    #[test]
    fn test_case_coordonnees() {
        // e4 = colonne 4, rang 3 → sq = 28
        assert_eq!(make_square(4, 3), 28);
        assert_eq!(file_of(28), 4);
        assert_eq!(rank_of(28), 3);
    }

    #[test]
    fn test_case_depuis_str() {
        assert_eq!(square_from_str("e4"), Some(28));
        assert_eq!(square_from_str("a1"), Some(0));
        assert_eq!(square_from_str("h8"), Some(63));
    }

    #[test]
    fn test_coup_uci() {
        let m = Move::quiet(12, 28); // e2 → e4
        assert_eq!(m.to_uci(), "e2e4");
    }

    #[test]
    fn test_promotion_uci() {
        let m = Move::promotion(52, 60, 4); // e7 → e8 dame
        assert_eq!(m.to_uci(), "e7e8q");
    }
}
