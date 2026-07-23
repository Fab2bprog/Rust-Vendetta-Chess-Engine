// =============================================================================
// Vendetta Chess Engine — src/utils/types.rs
//
// Role: Defines all the fundamental types shared across the entire project.
//        This file is the foundation of everything — Color, Piece, Square, Move and the
//        score constants. No other module is imported here.
//
// Contents:
//   - Color: color of a piece or a player (White / Black)
//   - Piece: piece type (Pawn, Knight, Bishop, Rook, Queen, King)
//   - MoveFlags: move type (normal, capture, castling, promotion, etc.)
//   - Move: representation of a move
//   - Constants: score values, piece indices
//
// Square convention:
//   - u8, value 0 to 63
//   - sq = rank * 8 + file
//   - File: 0=a, 1=b, ..., 7=h
//   - Rank    : 0=rank1, 1=rank2, ..., 7=rank8
//   - Example: e4 → file 4, rank 3 → sq = 3*8+4 = 28
// =============================================================================

/// Color of a player or a piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    White = 0,
    Black = 1,
}

impl Color {
    /// Returns the opposite color.
    #[inline]
    pub fn opposite(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }

    /// Returns the numeric index (0 for White, 1 for Black).
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Chess piece type.
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
    /// Returns the numeric index of the piece (0 to 5).
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Creates a piece from its numeric index.
    /// Returns None if the index is invalid.
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

    /// Returns the FEN character of the piece (uppercase = White, lowercase = Black).
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

    /// Creates a piece from a FEN character.
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

    /// Incremental material value of the piece, used by board::state
    /// to maintain eval_mg / eval_eg in real time.
    ///
    /// The king returns 0: it is NOT counted in the material
    /// (consistent with material_eval, which explicitly excludes it), but it
    /// still contributes to the PST via piece_square_values().
    ///
    /// The values are identical to eval::material::PIECE_VALUE[0..5].
    #[inline]
    pub fn incr_value(self) -> i32 {
        match self {
            Piece::Pawn   => 100,
            Piece::Knight => 320,
            Piece::Bishop => 330,
            Piece::Rook   => 500,
            Piece::Queen  => 900,
            Piece::King   =>   0, // Not counted in the material.
        }
    }
}

/// Flags describing the type of move played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveFlags {
    /// Quiet move (simple move without capture).
    Quiet,
    /// Two-square pawn push from the initial position.
    DoublePush,
    /// Kingside castling.
    CastleKingside,
    /// Queenside castling.
    CastleQueenside,
    /// Simple capture.
    Capture,
    /// En passant capture.
    EnPassant,
    /// Promotion without capture.
    Promotion,
    /// Promotion with capture.
    PromotionCapture,
}

impl MoveFlags {
    /// Returns true if the move is a capture (including en passant and promotion-capture).
    #[inline]
    pub fn is_capture(self) -> bool {
        matches!(self, MoveFlags::Capture | MoveFlags::EnPassant | MoveFlags::PromotionCapture)
    }

    /// Returns true if the move is a promotion.
    #[inline]
    pub fn is_promotion(self) -> bool {
        matches!(self, MoveFlags::Promotion | MoveFlags::PromotionCapture)
    }

    /// Returns true if the move is a castling move.
    #[inline]
    pub fn is_castle(self) -> bool {
        matches!(self, MoveFlags::CastleKingside | MoveFlags::CastleQueenside)
    }
}

/// Representation of a chess move.
///
/// Compact structure (4 bytes) that can be copied efficiently.
/// The `promotion` field is 0 if it is not a promotion,
/// otherwise the index of the promotion piece (1=Knight, 2=Bishop, 3=Rook, 4=Queen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    /// Starting square (0-63).
    pub from: u8,
    /// Destination square (0-63).
    pub to: u8,
    /// Move type.
    pub flags: MoveFlags,
    /// Promotion piece: 0=none, 1=Knight, 2=Bishop, 3=Rook, 4=Queen.
    pub promotion: u8,
}

impl Move {
    /// Creates a quiet move (simple move).
    #[inline]
    pub fn quiet(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::Quiet, promotion: 0 }
    }

    /// Creates a simple capture.
    #[inline]
    pub fn capture(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::Capture, promotion: 0 }
    }

    /// Creates a two-square push.
    #[inline]
    pub fn double_push(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::DoublePush, promotion: 0 }
    }

    /// Creates an en passant capture.
    #[inline]
    pub fn en_passant(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::EnPassant, promotion: 0 }
    }

    /// Creates a kingside castling move.
    #[inline]
    pub fn castle_kingside(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::CastleKingside, promotion: 0 }
    }

    /// Creates a queenside castling move.
    #[inline]
    pub fn castle_queenside(from: u8, to: u8) -> Move {
        Move { from, to, flags: MoveFlags::CastleQueenside, promotion: 0 }
    }

    /// Creates a promotion without capture.
    /// `promo`: 1=Knight, 2=Bishop, 3=Rook, 4=Queen.
    #[inline]
    pub fn promotion(from: u8, to: u8, promo: u8) -> Move {
        Move { from, to, flags: MoveFlags::Promotion, promotion: promo }
    }

    /// Creates a promotion with capture.
    #[inline]
    pub fn promotion_capture(from: u8, to: u8, promo: u8) -> Move {
        Move { from, to, flags: MoveFlags::PromotionCapture, promotion: promo }
    }

    /// Returns the promotion piece, or None if it is not a promotion.
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

    /// Returns the UCI notation of the move (e.g., "e2e4", "e7e8q").
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

    /// Null move (used as a default value / sentinel).
    ///
    /// Represented by `(from=0, to=0)` — the square a1 to a1, which is never
    /// a legal move in chess. The legal move generator never produces a move
    /// with `from == to`, so the collision is impossible in practice.
    ///
    /// Note: if the engine were ever to support variants where a1→a1
    /// were encodable, an `is_null: bool` field would need to be added, or
    /// an out-of-range square value (e.g., 255) would need to be used.
    pub const NULL: Move = Move { from: 0, to: 0, flags: MoveFlags::Quiet, promotion: 0 };

    /// Returns true if this is the sentinel null move.
    ///
    /// Invariant guaranteed by the legal move generator: no legal move has `from == to`.
    /// This detection is therefore reliable in all normal code paths.
    #[inline]
    pub fn is_null(self) -> bool {
        self.from == 0 && self.to == 0
    }
}

// =============================================================================
// Global constants
// =============================================================================

/// Infinite score (used as a bound in alpha-beta search).
pub const SCORE_INF: i32 = 1_000_000;

/// Score of a checkmate. The depth is subtracted to prefer faster mates.
pub const SCORE_MATE: i32 = 900_000;

/// Draw score (equal position).
pub const SCORE_DRAW: i32 = 0;

/// Number of piece types.
pub const NUM_PIECES: usize = 6;

/// Number of colors.
pub const NUM_COLORS: usize = 2;

/// Number of squares on the board.
pub const NUM_SQUARES: usize = 64;

// =============================================================================
// Utility functions for squares
// =============================================================================

/// Returns the file of a square (0=a, 7=h).
#[inline]
pub fn file_of(sq: u8) -> u8 {
    sq % 8
}

/// Returns the rank of a square (0=rank1, 7=rank8).
#[inline]
pub fn rank_of(sq: u8) -> u8 {
    sq / 8
}

/// Creates a square from file and rank.
#[inline]
pub fn make_square(file: u8, rank: u8) -> u8 {
    rank * 8 + file
}

/// Converts algebraic notation (e.g., "e4") into a square number.
pub fn square_from_str(s: &str) -> Option<u8> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 { return None; }
    let file = bytes[0].wrapping_sub(b'a');
    let rank = bytes[1].wrapping_sub(b'1');
    if file > 7 || rank > 7 { return None; }
    Some(make_square(file, rank))
}

/// Converts a square number into algebraic notation (e.g., 28 → "e4").
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
        // e4 = file 4, rank 3 → sq = 28
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
        let m = Move::promotion(52, 60, 4); // e7 → e8 queen
        assert_eq!(m.to_uci(), "e7e8q");
    }
}
