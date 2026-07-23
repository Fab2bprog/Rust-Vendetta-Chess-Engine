// =============================================================================
// Vendetta Chess Engine — src/eval/position.rs
//
// Role: Position tables (Piece-Square Tables).
//        Each table gives a positional bonus or penalty to a piece depending on
//        the square it is on. These tables encode the classic
//        positional wisdom of chess.
//
// Contents:
//   - Tables for each piece type (opening/middlegame)
//   - Endgame tables (the king should go to the center)
//   - Positional evaluation function
//
// Convention:
//   - Tables are defined from White's point of view (rank 1 at the bottom)
//   - For Black, the table is read in reverse (mirrored rank)
//   - Values are in centipawns
// =============================================================================

use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::pop_lsb;
use crate::eval::tables::{
    PAWN_TABLE, KNIGHT_TABLE, BISHOP_TABLE, ROOK_TABLE, QUEEN_TABLE,
    KING_MIDDLEGAME_TABLE, KING_ENDGAME_TABLE, mirror_square,
};

/// Returns the positional bonus of a piece on a square depending on the phase.
/// Delegates to the centralized tables in eval::tables.
pub fn piece_square_value(piece: Piece, color: Color, sq: u8, is_endgame: bool) -> i32 {
    let idx = if color == Color::White {
        sq as usize
    } else {
        mirror_square(sq) as usize
    };

    match piece {
        Piece::Pawn   => PAWN_TABLE[idx],
        Piece::Knight => KNIGHT_TABLE[idx],
        Piece::Bishop => BISHOP_TABLE[idx],
        Piece::Rook   => ROOK_TABLE[idx],
        Piece::Queen  => QUEEN_TABLE[idx],
        Piece::King   => {
            if is_endgame { KING_ENDGAME_TABLE[idx] } else { KING_MIDDLEGAME_TABLE[idx] }
        }
    }
}

/// Computes the total positional score for a given color.
pub fn positional_score(board: &Board, color: Color, is_endgame: bool) -> i32 {
    let mut score = 0i32;

    for piece in [Piece::Pawn, Piece::Knight, Piece::Bishop,
                  Piece::Rook, Piece::Queen, Piece::King] {
        let mut bb = board.pieces[color.index()][piece.index()];
        while bb != 0 {
            let sq = pop_lsb(&mut bb);
            score += piece_square_value(piece, color, sq, is_endgame);
        }
    }

    score
}

/// Computes the positional differential from the point of view of the active player.
pub fn positional_eval(board: &Board, is_endgame: bool) -> i32 {
    let white_score = positional_score(board, Color::White, is_endgame);
    let black_score = positional_score(board, Color::Black, is_endgame);
    let diff = white_score - black_score;

    if board.side_to_move == Color::White { diff } else { -diff }
}
