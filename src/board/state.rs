// =============================================================================
// Vendetta Chess Engine — src/board/state.rs
//
// Role: Complete representation of a chess position's state.
//        It is the central structure of the engine around which everything revolves.
//
// Contents:
//   - CastlingRights: castling rights encoded in 4 bits
//   - BoardState: irreversible state saved before each move
//     (to be able to undo a move — "unmake move")
//   - Board: main structure with 12 bitboards, complete state, Zobrist hash
//   - FEN reading/writing
//   - make_move / unmake_move
//   - Zobrist hashing (unique identification of a position)
//
// Technical choice: we use 12 bitboards (6 types × 2 colors) to
// represent all the pieces, plus 2 occupancy bitboards (one per color)
// and 1 global bitboard for speed of operations.
// =============================================================================

use crate::utils::types::{Color, Piece, Move, MoveFlags, file_of, rank_of, make_square, square_from_str};
use crate::board::bitboard::{
    Bitboard, set_bit, clear_bit, get_bit, lsb,
    init_attack_tables,
};
use crate::eval::tables::piece_square_values;

// =============================================================================
// Castling rights
// =============================================================================

/// Castling rights encoded in 4 bits:
/// - bit 0: white kingside castling (king side)
/// - bit 1: white queenside castling (queen side)
/// - bit 2: black kingside castling (king side)
/// - bit 3: black queenside castling (queen side)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CastlingRights(pub u8);

impl CastlingRights {
    pub const NONE: CastlingRights = CastlingRights(0);
    pub const ALL:  CastlingRights = CastlingRights(0b1111);

    pub const WHITE_KINGSIDE:  u8 = 0b0001;
    pub const WHITE_QUEENSIDE: u8 = 0b0010;
    pub const BLACK_KINGSIDE:  u8 = 0b0100;
    pub const BLACK_QUEENSIDE: u8 = 0b1000;

    /// Returns true if kingside castling is available for the given color.
    #[inline]
    pub fn can_castle_kingside(self, color: Color) -> bool {
        let flag = if color == Color::White {
            Self::WHITE_KINGSIDE
        } else {
            Self::BLACK_KINGSIDE
        };
        self.0 & flag != 0
    }

    /// Returns true if queenside castling is available for the given color.
    #[inline]
    pub fn can_castle_queenside(self, color: Color) -> bool {
        let flag = if color == Color::White {
            Self::WHITE_QUEENSIDE
        } else {
            Self::BLACK_QUEENSIDE
        };
        self.0 & flag != 0
    }

    /// Removes the castling rights associated with a square (called when a piece moves).
    #[inline]
    pub fn remove_rights_for_square(&mut self, sq: u8) {
        // If the king or rook moves, remove the corresponding rights.
        let mask = CASTLING_RIGHTS_MASK[sq as usize];
        self.0 &= !mask;
    }
}

/// Castling rights update mask for each square.
/// If a piece moves from (or to) this square, these rights are removed.
const CASTLING_RIGHTS_MASK: [u8; 64] = {
    let mut mask = [0u8; 64];
    // White rook king side: h1 = square 7
    mask[7]  = CastlingRights::WHITE_KINGSIDE;
    // White rook queen side: a1 = square 0
    mask[0]  = CastlingRights::WHITE_QUEENSIDE;
    // White king: e1 = square 4
    mask[4]  = CastlingRights::WHITE_KINGSIDE | CastlingRights::WHITE_QUEENSIDE;
    // Black rook king side: h8 = square 63
    mask[63] = CastlingRights::BLACK_KINGSIDE;
    // Black rook queen side: a8 = square 56
    mask[56] = CastlingRights::BLACK_QUEENSIDE;
    // Black king: e8 = square 60
    mask[60] = CastlingRights::BLACK_KINGSIDE | CastlingRights::BLACK_QUEENSIDE;
    mask
};

// =============================================================================
// Irreversible state (saved to undo a move)
// =============================================================================

/// Irreversible position information, saved before each move.
/// Allow restoring exactly the previous state during an unmake_move.
#[derive(Clone, Copy, Debug)]
pub struct BoardState {
    /// Castling rights before the move.
    pub castling: CastlingRights,
    /// En passant target square before the move (None if none).
    pub en_passant: Option<u8>,
    /// Halfmove counter for the 50-move rule before this move.
    pub halfmove_clock: u32,
    /// Captured piece (None if a quiet move).
    pub captured_piece: Option<Piece>,
    /// Zobrist hash before the move.
    pub hash: u64,
}

// =============================================================================
// Main structure: Board
// =============================================================================

/// Complete representation of a chess position.
///
/// Uses 12 bitboards (6 piece types × 2 colors) to represent
/// all the pieces. Derived occupancy bitboards help speed up
/// frequent operations.
///
/// Clone is derived to allow Lazy SMP to give each thread
/// its own independent copy of the board.
#[derive(Clone)]
pub struct Board {
    /// Piece bitboards: pieces[color][piece_type].
    /// color: 0=White, 1=Black
    /// piece_type: 0=Pawn, 1=Knight, 2=Bishop, 3=Rook, 4=Queen, 5=King
    pub pieces: [[Bitboard; 6]; 2],

    /// Occupancy bitboard per color: all pieces of one color.
    pub occupancy: [Bitboard; 2],

    /// Bitboard of all pieces (all colors combined).
    pub all_pieces: Bitboard,

    /// Color of the player to move.
    pub side_to_move: Color,

    /// Current castling rights.
    pub castling: CastlingRights,

    /// En passant target square (None if no en passant capture is possible).
    pub en_passant: Option<u8>,

    /// Halfmove counter for the 50-move rule.
    /// Reset to zero after a capture or a pawn move.
    pub halfmove_clock: u32,

    /// Full move number (starts at 1, incremented after Black's move).
    pub fullmove_number: u32,

    /// Zobrist hash of the current position (unique identifier).
    pub hash: u64,

    /// Incremental material + PST score in the middlegame, White's perspective.
    /// White − Black: positive = White advantage.
    /// Updated in place_piece() and remove_piece() on every move.
    pub eval_mg: i32,

    /// Incremental material + PST score in the endgame, White's perspective.
    /// Same convention as eval_mg.
    pub eval_eg: i32,

    /// Piece count per color and type: piece_count[color][piece_type].
    /// Indices: color 0=White 1=Black; type 0=Pawn 1=Knight 2=Bishop 3=Rook 4=Queen 5=King.
    /// Updated in place_piece() (+1) and remove_piece() (-1).
    ///
    /// Allows is_insufficient_material() to replace 10 count_ones() calls
    /// with 10 u8 reads — ~10× cheaper, called at every alpha-beta node.
    /// u8 is enough: theoretical maximum = 9 pawns after promotion, never > 255.
    pub piece_count: [[u8; 6]; 2],

    /// Mailbox: piece present on each square (None = empty square), indexed by
    /// square (0..63). Maintained incrementally in place_piece()/remove_piece()
    /// — the ONLY mutation points for piece bitboards (verified: no
    /// direct bitboard writes elsewhere in the code).
    ///
    /// Goal: make piece_at() O(1) (a single indexed read) instead of a
    /// linear scan of 12 bitboards. piece_at() is on the hottest path of the
    /// engine (make_move, SEE, move ordering, capture detection)
    /// — hence a gain in NPS, with no change in result (zero Elo lost).
    pub piece_on: [Option<(Piece, Color)>; 64],

    /// History of irreversible states (one entry per move played).
    pub history: Vec<BoardState>,
}

impl Board {
    /// Creates an empty board (no pieces).
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

    /// Creates a board with the initial chess position.
    pub fn start_position() -> Board {
        Board::from_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1")
            .expect("La position initiale FEN est valide")
    }

    // =========================================================================
    // Access to the bitboards
    // =========================================================================

    /// Returns the piece present on square `sq`, or None if empty.
    ///
    /// O(1): a simple read of the `piece_on` mailbox (maintained incrementally
    /// in place_piece()/remove_piece()). In development builds, a
    /// debug_assert re-checks the mailbox's consistency with a scan of the bitboards
    /// — any desynchronization is detected immediately (in particular by perft
    /// and `cargo test`), with no cost in release.
    #[inline]
    pub fn piece_at(&self, sq: u8) -> Option<(Piece, Color)> {
        debug_assert_eq!(
            self.piece_on[sq as usize],
            self.piece_at_scan(sq),
            "mailbox piece_on désynchronisé sur la case {}", sq
        );
        self.piece_on[sq as usize]
    }

    /// Linear scan of the bitboards (former implementation of piece_at).
    /// Kept ONLY as a consistency reference for the debug_assert of
    /// piece_at — never called in release (#[allow(dead_code)] because the reference
    /// lives inside a `debug_assert!` block, compiled out in release).
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

    /// Returns the square of the king of the given color.
    /// Precondition: the king is present on the board (guaranteed by from_fen).
    pub fn king_square(&self, color: Color) -> u8 {
        let bb = self.pieces[color.index()][Piece::King.index()];
        debug_assert_ne!(bb, 0, "king_square : aucun roi {:?} sur le plateau — position invalide", color);
        lsb(bb)
    }

    // =========================================================================
    // Placing and removing pieces
    // =========================================================================

    /// Places a piece on square `sq` and updates the bitboards, the hash
    /// and the incremental eval_mg / eval_eg scores.
    ///
    /// eval_mg / eval_eg are in White's perspective (White − Black):
    ///   - White: +mat +PST
    ///   - Black: −mat −PST
    ///
    /// The king counts for the PST but not for the material (incr_value = 0).
    pub fn place_piece(&mut self, color: Color, piece: Piece, sq: u8) {
        set_bit(&mut self.pieces[color.index()][piece.index()], sq);
        set_bit(&mut self.occupancy[color.index()], sq);
        set_bit(&mut self.all_pieces, sq);
        // Mailbox: the piece is now present on `sq` (O(1) read via piece_at).
        self.piece_on[sq as usize] = Some((piece, color));
        self.hash ^= ZOBRIST.piece(color, piece, sq);

        // Incremental score update (material + PST).
        let (pst_mg, pst_eg) = piece_square_values(piece, color, sq);
        let sign = if color == Color::White { 1i32 } else { -1i32 };
        self.eval_mg += sign * (piece.incr_value() + pst_mg);
        self.eval_eg += sign * (piece.incr_value() + pst_eg);

        // Piece counter (for is_insufficient_material O(1)).
        self.piece_count[color.index()][piece.index()] += 1;
    }

    /// Removes a piece from square `sq` and updates the bitboards, the hash
    /// and the incremental eval_mg / eval_eg scores.
    pub fn remove_piece(&mut self, color: Color, piece: Piece, sq: u8) {
        clear_bit(&mut self.pieces[color.index()][piece.index()], sq);
        clear_bit(&mut self.occupancy[color.index()], sq);
        clear_bit(&mut self.all_pieces, sq);
        // Mailbox: square `sq` is now empty.
        self.piece_on[sq as usize] = None;
        self.hash ^= ZOBRIST.piece(color, piece, sq);

        // Cancellation of the incremental contribution.
        let (pst_mg, pst_eg) = piece_square_values(piece, color, sq);
        let sign = if color == Color::White { 1i32 } else { -1i32 };
        self.eval_mg -= sign * (piece.incr_value() + pst_mg);
        self.eval_eg -= sign * (piece.incr_value() + pst_eg);

        // Piece counter (for is_insufficient_material O(1)).
        // Invariant: the counter must be > 0 before decrementing.
        // In release mode, the u8 wraps silently → debug_assert! to detect
        // any corruption during development, at no cost in production.
        debug_assert!(
            self.piece_count[color.index()][piece.index()] > 0,
            "remove_piece : piece_count[{:?}][{:?}] est déjà 0 — double suppression ?",
            color, piece
        );
        self.piece_count[color.index()][piece.index()] -= 1;
    }

    /// Moves a piece from `from` to `to` without updating the hash (internal use).
    fn move_piece_internal(&mut self, color: Color, piece: Piece, from: u8, to: u8) {
        self.remove_piece(color, piece, from);
        self.place_piece(color, piece, to);
    }

    // =========================================================================
    // FEN reading
    // =========================================================================

    /// Creates a board from a FEN string.
    /// Returns Err(message) if the FEN is invalid.
    pub fn from_fen(fen: &str) -> Result<Board, String> {
        let mut board = Board::empty();
        let parts: Vec<&str> = fen.split_whitespace().collect();

        if parts.len() < 4 {
            return Err(format!("FEN invalide : pas assez de champs (reçu {})", parts.len()));
        }

        // --- Field 1: piece placement ---
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

        // --- Validation: exactly one king per side ---
        // Necessary to guarantee that king_square() always returns a valid square (0-63).
        // Without this check, a malformed FEN would cause a crash in the middle of a game.
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

        // --- Field 2: side to move ---
        board.side_to_move = match parts[1] {
            "w" => Color::White,
            "b" => Color::Black,
            _   => return Err(format!("FEN invalide : trait '{}' inconnu", parts[1])),
        };
        if board.side_to_move == Color::Black {
            board.hash ^= ZOBRIST.side;
        }

        // --- Field 3: castling rights ---
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

        // --- Validation of castling rights ---
        //
        // For each active right, we check that the king AND the required rook are
        // indeed present on their standard starting squares (classical chess):
        //
        //   K → White king on e1 (sq 4)  + White rook on h1 (sq 7)
        //   Q → White king on e1 (sq 4)  + White rook on a1 (sq 0)
        //   k → Black king on e8 (sq 60) + Black rook on h8 (sq 63)
        //   q → Black king on e8 (sq 60) + Black rook on a8 (sq 56)
        //
        // Strategy: silently remove the invalid right rather than Err().
        //
        //   Reason: many GUIs (Arena, Cutechess…) send FENs with
        //   residual castling rights (e.g. "KQkq" even though the a1 rook has moved
        //   and then come back). Returning Err() would block the engine on a legal move.
        //   We remove the invalid right and the engine continues cleanly.
        //
        //   In a development build (`debug_assert!`) the inconsistency is flagged
        //   immediately with no cost at all in release.
        //
        // IMPORTANT: the Zobrist hash is computed AFTER this correction to reflect
        // the real rights (not the raw FEN ones). A hash computed on
        // incorrect rights would produce false hits in the transposition table.
        {
            let w_rooks = board.pieces[Color::White.index()][Piece::Rook.index()];
            let b_rooks = board.pieces[Color::Black.index()][Piece::Rook.index()];
            let w_kings = board.pieces[Color::White.index()][Piece::King.index()];
            let b_kings = board.pieces[Color::Black.index()][Piece::King.index()];

            // White king on e1 (sq 4)?
            let white_king_on_e1 = get_bit(w_kings, 4);
            // Black king on e8 (sq 60)?
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

        // Hash computed on the corrected rights (not the raw FEN rights).
        board.hash ^= ZOBRIST.castling(board.castling);

        // --- Field 4: en passant ---
        board.en_passant = if parts[3] == "-" {
            None
        } else {
            let sq = square_from_str(parts[3])
                .ok_or_else(|| format!("FEN invalide : case en passant '{}'", parts[3]))?;

            // Rank validation: the en passant square must be on rank 3 (index 2,
            // black just pushed two squares, target square for white) or rank 6
            // (index 5, white just pushed, target square for black).
            // An incorrect rank would produce silent corruption during the en passant capture
            // (to - 8 or to + 8 would point outside the expected zone).
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

        // --- Field 5: 50-move counter (optional) ---
        if parts.len() > 4 {
            board.halfmove_clock = parts[4].parse::<u32>()
                .map_err(|_| format!("FEN invalide : compteur 50 coups '{}'", parts[4]))?;
        }

        // --- Field 6: full move number (optional) ---
        if parts.len() > 5 {
            board.fullmove_number = parts[5].parse::<u32>()
                .map_err(|_| format!("FEN invalide : numéro de coup '{}'", parts[5]))?;
        }

        Ok(board)
    }

    /// Generates the FEN string of the current position.
    pub fn to_fen(&self) -> String {
        let mut fen = String::new();

        // Piece placement.
        // Invariant: `empty` is counted square by square over 8 columns → max 8.
        // `char::from_digit(n, 10)` returns None only if n >= 10.
        // Since empty ∈ [1, 8], the conversion is always valid.
        // We use `(b'0' + empty as u8) as char` to document the invariant
        // explicitly and avoid any superfluous unwrap.
        for rank in (0..8).rev() {
            let mut empty = 0u8; // u8 is enough: max 8 empty squares per rank
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

        // Side to move
        fen.push(' ');
        fen.push(if self.side_to_move == Color::White { 'w' } else { 'b' });

        // Castling rights
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

        // Counters
        fen.push_str(&format!(" {} {}", self.halfmove_clock, self.fullmove_number));

        fen
    }

    // =========================================================================
    // Play and undo a move
    // =========================================================================

    /// Plays the move `mv` on the board.
    /// The irreversible state is saved in `self.history` so the move can
    /// be undone with unmake_move.
    pub fn make_move(&mut self, mv: Move) {
        let color   = self.side_to_move;
        let enemy   = color.opposite();
        let from    = mv.from;
        let to      = mv.to;

        // Retrieve the piece that is moving.
        // Invariant: from must always contain a piece (guaranteed by the legal generator).
        // In debug we panic immediately to detect any corruption; in release we
        // return cleanly rather than corrupting the board state.
        let piece = match self.piece_at(from) {
            Some((p, _)) => p,
            None => {
                debug_assert!(false, "make_move : aucune pièce sur la case de départ {}", from);
                return;
            }
        };

        // Retrieve the captured piece (if normal capture)
        let captured = if mv.flags.is_capture() && mv.flags != MoveFlags::EnPassant {
            self.piece_at(to).map(|(p, _)| p)
        } else {
            None
        };

        // Save the irreversible state
        self.history.push(BoardState {
            castling:        self.castling,
            en_passant:      self.en_passant,
            halfmove_clock:  self.halfmove_clock,
            captured_piece:  captured,
            hash:            self.hash,
        });

        // Hash update: remove the old en passant and castling
        if let Some(ep_sq) = self.en_passant {
            self.hash ^= ZOBRIST.en_passant(file_of(ep_sq));
        }
        self.hash ^= ZOBRIST.castling(self.castling);

        // Reset the en passant square
        self.en_passant = None;

        // Update castling rights
        self.castling.remove_rights_for_square(from);
        self.castling.remove_rights_for_square(to);
        self.hash ^= ZOBRIST.castling(self.castling);

        // Update the 50-move counter
        if piece == Piece::Pawn || mv.flags.is_capture() {
            self.halfmove_clock = 0;
        } else {
            self.halfmove_clock += 1;
        }

        match mv.flags {
            MoveFlags::Quiet => {
                // Simple move
                self.move_piece_internal(color, piece, from, to);
                // Two-square push: update the en passant square
                // (handled by DoublePush below)
            }

            MoveFlags::DoublePush => {
                // Pawn two-square push
                self.move_piece_internal(color, Piece::Pawn, from, to);
                // The en passant square is between from and to
                let ep_sq = if color == Color::White { from + 8 } else { from - 8 };
                self.en_passant = Some(ep_sq);
                self.hash ^= ZOBRIST.en_passant(file_of(ep_sq));
            }

            MoveFlags::Capture => {
                // Remove the captured piece
                if let Some(cap) = captured {
                    self.remove_piece(enemy, cap, to);
                }
                self.move_piece_internal(color, piece, from, to);
            }

            MoveFlags::EnPassant => {
                // The captured piece is on the adjacent square, not on `to`
                let cap_sq = if color == Color::White { to - 8 } else { to + 8 };
                self.remove_piece(enemy, Piece::Pawn, cap_sq);
                self.move_piece_internal(color, Piece::Pawn, from, to);
            }

            MoveFlags::CastleKingside => {
                // Move the king and the rook (king side)
                self.move_piece_internal(color, Piece::King, from, to);
                let (rook_from, rook_to) = if color == Color::White {
                    (7u8, 5u8)   // h1 → f1
                } else {
                    (63u8, 61u8) // h8 → f8
                };
                self.move_piece_internal(color, Piece::Rook, rook_from, rook_to);
            }

            MoveFlags::CastleQueenside => {
                // Move the king and the rook (queen side)
                self.move_piece_internal(color, Piece::King, from, to);
                let (rook_from, rook_to) = if color == Color::White {
                    (0u8, 3u8)   // a1 → d1
                } else {
                    (56u8, 59u8) // a8 → d8
                };
                self.move_piece_internal(color, Piece::Rook, rook_from, rook_to);
            }

            MoveFlags::Promotion => {
                // Remove the pawn and place the promotion piece
                self.remove_piece(color, Piece::Pawn, from);
                let promo = mv.promotion_piece().unwrap_or(Piece::Queen);
                self.place_piece(color, promo, to);
            }

            MoveFlags::PromotionCapture => {
                // Remove the captured piece, remove the pawn, place the promotion
                if let Some(cap) = captured {
                    self.remove_piece(enemy, cap, to);
                }
                self.remove_piece(color, Piece::Pawn, from);
                let promo = mv.promotion_piece().unwrap_or(Piece::Queen);
                self.place_piece(color, promo, to);
            }
        }

        // Switch the side to move
        self.side_to_move = enemy;
        self.hash ^= ZOBRIST.side;

        // Increment the full move number after Black
        if color == Color::Black {
            self.fullmove_number += 1;
        }
    }

    /// Undoes the last move played and restores the previous state.
    /// Precondition: make_move has been called at least once.
    pub fn unmake_move(&mut self, mv: Move) {
        // Restore the side to move (back to the player who had just moved)
        self.side_to_move = self.side_to_move.opposite();
        let color = self.side_to_move;
        let enemy = color.opposite();
        let from  = mv.from;
        let to    = mv.to;

        // Decrement the move number if it was Black who had played
        if color == Color::Black {
            self.fullmove_number -= 1;
        }

        // Restore the irreversible state.
        // Invariant: history must never be empty here (make/unmake are always symmetric).
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

        // Retrieve the piece that was on `to`.
        // For promotions, we know it was a pawn (the promoted piece was
        // removed by remove_piece in make_move, but the Pawn bitboard has not yet
        // been restored — we use Piece::Pawn directly).
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
                // Simple move: put the piece back in place
                self.move_piece_internal(color, piece, to, from);
            }

            MoveFlags::Capture => {
                // Put the piece back in place and restore the captured piece
                self.move_piece_internal(color, piece, to, from);
                if let Some(cap) = state.captured_piece {
                    self.place_piece(enemy, cap, to);
                }
            }

            MoveFlags::EnPassant => {
                // Put the pawn back in place and restore the pawn captured en passant
                self.move_piece_internal(color, Piece::Pawn, to, from);
                let cap_sq = if color == Color::White { to - 8 } else { to + 8 };
                self.place_piece(enemy, Piece::Pawn, cap_sq);
            }

            MoveFlags::CastleKingside => {
                // Put the king and the rook back in place
                self.move_piece_internal(color, Piece::King, to, from);
                let (rook_from, rook_to) = if color == Color::White {
                    (7u8, 5u8)
                } else {
                    (63u8, 61u8)
                };
                self.move_piece_internal(color, Piece::Rook, rook_to, rook_from);
            }

            MoveFlags::CastleQueenside => {
                // Put the king and the rook back in place
                self.move_piece_internal(color, Piece::King, to, from);
                let (rook_from, rook_to) = if color == Color::White {
                    (0u8, 3u8)
                } else {
                    (56u8, 59u8)
                };
                self.move_piece_internal(color, Piece::Rook, rook_to, rook_from);
            }

            MoveFlags::Promotion => {
                // Remove the promotion piece and put the pawn back
                let promo = mv.promotion_piece().unwrap_or(Piece::Queen);
                self.remove_piece(color, promo, to);
                self.place_piece(color, Piece::Pawn, from);
            }

            MoveFlags::PromotionCapture => {
                // Remove the promotion piece, put the pawn back and the captured piece
                let promo = mv.promotion_piece().unwrap_or(Piece::Queen);
                self.remove_piece(color, promo, to);
                self.place_piece(color, Piece::Pawn, from);
                if let Some(cap) = state.captured_piece {
                    self.place_piece(enemy, cap, to);
                }
            }
        }
    }

    /// Plays a "null" move (the player passes their turn).
    /// Used in search for null move pruning.
    /// Returns the previous en passant square so it can be undone.
    pub fn make_null_move(&mut self) -> Option<u8> {
        let prev_ep = self.en_passant;

        // Remove the en passant from the hash
        if let Some(ep_sq) = self.en_passant {
            self.hash ^= ZOBRIST.en_passant(file_of(ep_sq));
        }
        self.en_passant = None;

        // Switch the side to move
        self.side_to_move = self.side_to_move.opposite();
        self.hash ^= ZOBRIST.side;
        self.halfmove_clock += 1;

        prev_ep
    }

    /// Undoes a null move.
    pub fn unmake_null_move(&mut self, prev_ep: Option<u8>) {
        self.side_to_move = self.side_to_move.opposite();
        self.hash ^= ZOBRIST.side;
        self.halfmove_clock -= 1;

        // Restore the en passant
        if let Some(ep_sq) = prev_ep {
            self.hash ^= ZOBRIST.en_passant(file_of(ep_sq));
        }
        self.en_passant = prev_ep;
    }
}

// =============================================================================
// Zobrist hashing
//
// Zobrist hashing makes it possible to identify a position in a (quasi) unique way
// with a u64 integer. It is updated incrementally on each move played,
// which is very efficient. Used by the transposition table.
// =============================================================================

/// Table of Zobrist random numbers, statically initialized.
pub struct ZobristTable {
    /// Random number for each combination (color, piece, square).
    pub pieces:     [[[u64; 64]; 6]; 2],
    /// Random number for the side to move (Black to move).
    pub side:       u64,
    /// Random numbers for castling rights (16 possible combinations).
    pub castling:   [u64; 16],
    /// Random numbers for the en passant square's column (8 columns).
    pub en_passant: [u64; 8],
}

impl ZobristTable {
    /// Returns the hash for a piece on a square.
    #[inline]
    pub fn piece(&self, color: Color, piece: Piece, sq: u8) -> u64 {
        self.pieces[color.index()][piece.index()][sq as usize]
    }

    /// Returns the hash for castling rights.
    #[inline]
    pub fn castling(&self, rights: CastlingRights) -> u64 {
        self.castling[(rights.0 & 0xF) as usize]
    }

    /// Returns the hash for the en passant square's column.
    #[inline]
    pub fn en_passant(&self, file: u8) -> u64 {
        self.en_passant[file as usize]
    }
}

/// Generates a u64 pseudo-random number from a seed (xorshift64).
/// Used only to initialize the Zobrist table.
const fn xorshift64(seed: u64) -> u64 {
    let x = seed ^ (seed << 13);
    let x = x ^ (x >> 7);
    x ^ (x << 17)
}

/// Zobrist table initialized at compile time with deterministic constants.
/// We use a xorshift generator to produce well-distributed values.
pub static ZOBRIST: ZobristTable = {
    let mut pieces     = [[[0u64; 64]; 6]; 2];
    let mut castling   = [0u64; 16];
    let mut en_passant = [0u64; 8];
    let mut seed: u64  = 0x123456789ABCDEF0;

    // Fill in the numbers for the pieces
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

    // Fill in the numbers for the side to move
    seed = xorshift64(seed);
    let side = seed;

    // Fill in the numbers for castling
    let mut i = 0usize;
    while i < 16 {
        seed = xorshift64(seed);
        castling[i] = seed;
        i += 1;
    }

    // Fill in the numbers for en passant
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
        // White plays first
        assert_eq!(board.side_to_move, Color::White);
        // 8 white pawns
        assert_eq!(board.pieces[Color::White.index()][Piece::Pawn.index()].count_ones(), 8);
        // 8 black pawns
        assert_eq!(board.pieces[Color::Black.index()][Piece::Pawn.index()].count_ones(), 8);
        // All castling rights
        assert_eq!(board.castling.0, CastlingRights::ALL.0);
        // No en passant
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
