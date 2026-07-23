// =============================================================================
// Vendetta Chess Motor — src/board/bitboard.rs
//
// Rôle : Définit le type Bitboard (u64) et toutes les opérations associées.
//        Un bitboard est un entier 64 bits où chaque bit représente une case
//        de l'échiquier. Le bit i correspond à la case i (0=a1, 63=h8).
//
// Contenu :
//   - Type Bitboard et alias
//   - Fonctions de manipulation de bits (set, clear, get, pop, count, lsb)
//   - Masques de colonnes et de rangs précalculés
//   - Fonctions d'attaque pour les pièces glissantes (fou, tour, dame)
//     via Magic Bitboards (O(1) : une multiplication + un décalage + un lookup)
//   - Tables d'attaque précalculées pour cavalier et roi
//
// Choix technique : les attaques des pièces glissantes (tour, fou, dame)
// utilisent des Magic Bitboards (module magic.rs). Tables précalculées au
// démarrage en < 10 ms, accès O(1) pendant la recherche.
// =============================================================================

use std::sync::OnceLock;
use super::magic::{init_magic_tables, rook_attacks_magic, bishop_attacks_magic};

/// Type Bitboard : entier 64 bits représentant un ensemble de cases.
/// Bit i = 1 signifie que la case i est dans l'ensemble.
pub type Bitboard = u64;

// =============================================================================
// Opérations de base sur les bits
// =============================================================================

/// Active le bit correspondant à la case `sq`.
#[inline]
pub fn set_bit(bb: &mut Bitboard, sq: u8) {
    *bb |= 1u64 << sq;
}

/// Désactive le bit correspondant à la case `sq`.
#[inline]
pub fn clear_bit(bb: &mut Bitboard, sq: u8) {
    *bb &= !(1u64 << sq);
}

/// Retourne true si le bit de la case `sq` est activé.
#[inline]
pub fn get_bit(bb: Bitboard, sq: u8) -> bool {
    (bb >> sq) & 1 == 1
}

/// Retourne le nombre de bits activés (popcount).
#[inline]
pub fn count_bits(bb: Bitboard) -> u32 {
    bb.count_ones()
}

/// Retourne l'index du bit le moins significatif (LSB).
/// Précondition : bb != 0.
#[inline]
pub fn lsb(bb: Bitboard) -> u8 {
    bb.trailing_zeros() as u8
}

/// Retourne l'index du LSB et le désactive dans le bitboard.
/// Précondition : bb != 0.
#[inline]
pub fn pop_lsb(bb: &mut Bitboard) -> u8 {
    let sq = lsb(*bb);
    *bb &= *bb - 1;
    sq
}

// =============================================================================
// Masques de colonnes (files) et de rangs (ranks)
// =============================================================================

/// Masque de la colonne a (colonne 0).
pub const FILE_A: Bitboard = 0x0101_0101_0101_0101;
/// Masque de la colonne b (colonne 1).
pub const FILE_B: Bitboard = 0x0202_0202_0202_0202;
/// Masque de la colonne g (colonne 6).
pub const FILE_G: Bitboard = 0x4040_4040_4040_4040;
/// Masque de la colonne h (colonne 7).
pub const FILE_H: Bitboard = 0x8080_8080_8080_8080;

/// Masque du rang 1 (rang 0).
pub const RANK_1: Bitboard = 0x0000_0000_0000_00FF;
/// Masque du rang 2 (rang 1).
pub const RANK_2: Bitboard = 0x0000_0000_0000_FF00;
/// Masque du rang 7 (rang 6).
pub const RANK_7: Bitboard = 0x00FF_0000_0000_0000;
/// Masque du rang 8 (rang 7).
pub const RANK_8: Bitboard = 0xFF00_0000_0000_0000;

/// Retourne le masque de la colonne donnée (0=a, 7=h).
#[inline]
pub fn file_mask(file: u8) -> Bitboard {
    FILE_A << file
}

/// Retourne le masque du rang donné (0=rang1, 7=rang8).
#[inline]
pub fn rank_mask(rank: u8) -> Bitboard {
    RANK_1 << (rank * 8)
}

// =============================================================================
// Tables d'attaque précalculées pour cavalier et roi
// Ces tables sont calculées une seule fois au démarrage (voir init_attack_tables).
// =============================================================================

/// Tables d'attaque précalculées — thread-safe via OnceLock.
/// Une fois initialisées, elles sont en lecture seule pour tous les threads.
static KNIGHT_ATTACKS_TABLE: OnceLock<[Bitboard; 64]> = OnceLock::new();
static KING_ATTACKS_TABLE:   OnceLock<[Bitboard; 64]> = OnceLock::new();

/// Initialise toutes les tables d'attaque précalculées :
///   - Cavalier et roi (tables OnceLock simples)
///   - Tour et fou via Magic Bitboards (tables OnceLock dans magic.rs)
///
/// Doit être appelée une seule fois au démarrage, avant tout threading.
/// Toutes les initialisations sont idempotentes et thread-safe (OnceLock).
pub fn init_attack_tables() {
    KNIGHT_ATTACKS_TABLE.get_or_init(|| {
        let mut table = [0u64; 64];
        for sq in 0u8..64 {
            table[sq as usize] = compute_knight_attacks(sq);
        }
        table
    });
    KING_ATTACKS_TABLE.get_or_init(|| {
        let mut table = [0u64; 64];
        for sq in 0u8..64 {
            table[sq as usize] = compute_king_attacks(sq);
        }
        table
    });
    // Tables magiques pour les pièces glissantes (tour et fou)
    init_magic_tables();
}

/// Calcule les cases attaquées par un cavalier sur la case `sq`.
fn compute_knight_attacks(sq: u8) -> Bitboard {
    let bb: Bitboard = 1u64 << sq;
    let mut attacks: Bitboard = 0;

    // Les 8 déplacements possibles du cavalier, en évitant les débordements de bord.
    // Nord-Nord-Est : +17, pas si colonne h
    attacks |= (bb << 17) & !FILE_A;
    // Nord-Nord-Ouest : +15, pas si colonne a
    attacks |= (bb << 15) & !FILE_H;
    // Nord-Est-Est : +10, pas si colonnes g ou h
    attacks |= (bb << 10) & !(FILE_A | FILE_B);
    // Nord-Ouest-Ouest : +6, pas si colonnes a ou b
    attacks |= (bb << 6)  & !(FILE_G | FILE_H);
    // Sud-Sud-Est : -15, pas si colonne h
    attacks |= (bb >> 15) & !FILE_A;
    // Sud-Sud-Ouest : -17, pas si colonne a
    attacks |= (bb >> 17) & !FILE_H;
    // Sud-Est-Est : -6, pas si colonnes g ou h
    attacks |= (bb >> 6)  & !(FILE_A | FILE_B);
    // Sud-Ouest-Ouest : -10, pas si colonnes a ou b
    attacks |= (bb >> 10) & !(FILE_G | FILE_H);

    attacks
}

/// Calcule les cases attaquées par un roi sur la case `sq`.
fn compute_king_attacks(sq: u8) -> Bitboard {
    let bb: Bitboard = 1u64 << sq;
    let mut attacks: Bitboard = 0;

    // Les 8 directions du roi, en évitant les débordements de bord.
    attacks |= bb << 8;                      // Nord
    attacks |= bb >> 8;                      // Sud
    attacks |= (bb << 1) & !FILE_A;         // Est
    attacks |= (bb >> 1) & !FILE_H;         // Ouest
    attacks |= (bb << 9) & !FILE_A;         // Nord-Est
    attacks |= (bb << 7) & !FILE_H;         // Nord-Ouest
    attacks |= (bb >> 7) & !FILE_A;         // Sud-Est
    attacks |= (bb >> 9) & !FILE_H;         // Sud-Ouest

    attacks
}

/// Retourne le bitboard des cases attaquées par un cavalier sur la case `sq`.
/// Thread-safe : lecture seule depuis OnceLock initialisé au démarrage.
/// Précondition : sq < 64 (garanti par pop_lsb / lsb appelés sur un bitboard non nul).
#[inline]
pub fn knight_attacks(sq: u8) -> Bitboard {
    debug_assert!(sq < 64, "knight_attacks : case invalide sq={} (doit être 0-63)", sq);
    KNIGHT_ATTACKS_TABLE.get()
        .expect("init_attack_tables() non appelée")[sq as usize]
}

/// Retourne le bitboard des cases attaquées par un roi sur la case `sq`.
/// Thread-safe : lecture seule depuis OnceLock initialisé au démarrage.
/// Précondition : sq < 64 (garanti par king_square, lui-même protégé par from_fen).
#[inline]
pub fn king_attacks(sq: u8) -> Bitboard {
    debug_assert!(sq < 64, "king_attacks : case invalide sq={} (doit être 0-63)", sq);
    KING_ATTACKS_TABLE.get()
        .expect("init_attack_tables() non appelée")[sq as usize]
}

// =============================================================================
// Attaques des pièces glissantes (approche classique par boucle)
//
// Pour chaque pièce glissante, on explore chaque direction jusqu'à rencontrer
// une pièce bloquante ou le bord de l'échiquier. La case bloquante est incluse
// (car on peut capturer) mais on s'arrête là.
// =============================================================================

/// Retourne les cases attaquées par une tour sur la case `sq`
/// avec le bitboard `occupied` représentant toutes les pièces présentes.
///
/// Implémentation Magic Bitboards : O(1) — multiplication + décalage + lookup.
/// 5 à 20× plus rapide que la version classique par boucle.
#[inline]
pub fn rook_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    rook_attacks_magic(sq, occupied)
}

/// Retourne les cases attaquées par un fou sur la case `sq`
/// avec le bitboard `occupied` représentant toutes les pièces présentes.
///
/// Implémentation Magic Bitboards : O(1) — multiplication + décalage + lookup.
/// 5 à 20× plus rapide que la version classique par boucle.
#[inline]
pub fn bishop_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    bishop_attacks_magic(sq, occupied)
}

/// Calcule les cases attaquées par une dame sur la case `sq`.
/// La dame combine les attaques de la tour et du fou.
#[inline]
pub fn queen_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    rook_attacks(sq, occupied) | bishop_attacks(sq, occupied)
}

// =============================================================================
// Attaques des pions
// Les pions n'attaquent pas comme ils avancent, donc on les gère séparément.
// =============================================================================

/// Calcule les cases attaquées par les pions blancs (vers le haut du plateau).
#[inline]
pub fn white_pawn_attacks(pawns: Bitboard) -> Bitboard {
    ((pawns << 9) & !FILE_A) | ((pawns << 7) & !FILE_H)
}

/// Calcule les cases attaquées par les pions noirs (vers le bas du plateau).
#[inline]
pub fn black_pawn_attacks(pawns: Bitboard) -> Bitboard {
    ((pawns >> 7) & !FILE_A) | ((pawns >> 9) & !FILE_H)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_clear_get() {
        let mut bb: Bitboard = 0;
        set_bit(&mut bb, 0);
        assert!(get_bit(bb, 0));
        clear_bit(&mut bb, 0);
        assert!(!get_bit(bb, 0));
    }

    #[test]
    fn test_lsb_pop() {
        let mut bb: Bitboard = 0b1010;
        let sq = pop_lsb(&mut bb);
        assert_eq!(sq, 1);
        assert_eq!(bb, 0b1000);
    }

    #[test]
    fn test_cavalier_centre() {
        init_attack_tables();
        // Un cavalier en e4 (sq=28) attaque 8 cases
        let attacks = knight_attacks(28);
        assert_eq!(count_bits(attacks), 8);
    }

    #[test]
    fn test_cavalier_coin() {
        init_attack_tables();
        // Un cavalier en a1 (sq=0) attaque 2 cases
        let attacks = knight_attacks(0);
        assert_eq!(count_bits(attacks), 2);
    }

    #[test]
    fn test_tour_echiquier_vide() {
        // init OBLIGATOIRE : rook_attacks délègue aux magic bitboards, qui
        // paniquent si init_magic_tables() (appelée par init_attack_tables())
        // n'a pas tourné. Sans cette ligne, le test dépendait de l'ordre
        // d'exécution parallèle des tests (flaky, panic possible) — rendu
        // autonome ici, comme les tests cavalier ci-dessus.
        init_attack_tables();
        // Une tour en e4 (sq=28) sur échiquier vide attaque 14 cases
        let attacks = rook_attacks(28, 0);
        assert_eq!(count_bits(attacks), 14);
    }

    #[test]
    fn test_fou_echiquier_vide() {
        // init OBLIGATOIRE (même raison que test_tour_echiquier_vide).
        init_attack_tables();
        // Un fou en e4 (sq=28) sur échiquier vide attaque 13 cases
        let attacks = bishop_attacks(28, 0);
        assert_eq!(count_bits(attacks), 13);
    }
}
