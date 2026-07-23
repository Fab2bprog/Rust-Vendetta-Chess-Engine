// =============================================================================
// Vendetta Chess Motor — src/eval/pawns.rs
//
// Rôle : Évaluation de la structure de pions.
//        La structure de pions est fondamentale aux échecs : elle détermine
//        les plans stratégiques et les forces/faiblesses permanentes.
//
// Contenu :
//   - Détection des pions doublés (deux pions sur la même colonne)
//   - Détection des pions isolés (aucun pion allié sur les colonnes adjacentes)
//   - Détection des pions passés (aucun pion adverse ne peut les bloquer)
//   - Score global de structure de pions
//
// Pénalités et bonus (en centipions) :
//   - Pion doublé   : -20 (deux pions sur même colonne = faiblesse)
//   - Pion isolé    : -20 (pas de protection latérale = faiblesse)
//   - Pion passé    : +20 à +50 selon l'avancement (force stratégique majeure)
// =============================================================================

use std::sync::OnceLock;
use std::cell::RefCell;
use crate::utils::types::{Color, Piece};
use crate::board::state::Board;
use crate::board::bitboard::{Bitboard, file_mask, rank_mask};

/// Pénalité pour un pion doublé (par pion en trop sur la colonne).
/// Calibré par Texel Tuning v3 (était -20) — voir material.rs::PIECE_VALUE.
const DOUBLED_PAWN_PENALTY: i32 = -24;

/// Pénalité pour un pion isolé (sans pion ami sur les colonnes voisines).
/// Calibré par Texel Tuning v3 (était -20).
const ISOLATED_PAWN_PENALTY: i32 = -19;

/// Bonus pour un pion passé, par rang d'avancement (index = rang 0-7).
/// Plus le pion est avancé, plus il est dangereux.
/// Calibré par Texel Tuning v3 (était [0, 5, 10, 20, 35, 60, 100, 0]) —
/// strictement positif et croissant, contrairement aux tentatives de tuning
/// précédentes sans calibrage K qui avaient produit un signe incohérent
/// aux premiers rangs (voir material.rs::PIECE_VALUE pour le contexte complet).
const PASSED_PAWN_BONUS: [i32; 8] = [0, 7, 8, 33, 75, 138, 218, 0];

// =============================================================================
// Table précalculée des masques de pion passé
// =============================================================================

/// Retourne la table `PASSED_PAWN_MASK[color][sq]`.
///
/// Pour chaque case `sq` et chaque couleur, le masque couvre tous les rangs
/// "devant" le pion (selon sa couleur) sur la colonne du pion et les deux
/// colonnes adjacentes. Un pion est passé si `enemy_pawns & mask == 0`.
///
/// Précalculée une seule fois via OnceLock (thread-safe, zéro overhead ensuite).
/// Remplace la boucle O(8) `for r in (rank+1)..8 { mask |= rank_mask(r) & files; }`
/// par un lookup O(1) : un seul accès tableau au lieu de 8 OR + AND bitboard.
///
/// Indices :
///   [0][sq] = masque pour un pion Blanc à la case sq (rangs supérieurs)
///   [1][sq] = masque pour un pion Noir  à la case sq (rangs inférieurs)
#[inline]
fn get_passed_pawn_mask() -> &'static [[Bitboard; 64]; 2] {
    static TABLE: OnceLock<[[Bitboard; 64]; 2]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [[0u64; 64]; 2];
        for sq in 0u8..64 {
            let file = sq % 8;
            let rank = sq / 8;
            let left   = if file > 0 { file_mask(file - 1) } else { 0 };
            let center = file_mask(file);
            let right  = if file < 7 { file_mask(file + 1) } else { 0 };
            let cols   = left | center | right;

            // Blanc : rangs strictement au-dessus du pion
            let mut wm = 0u64;
            for r in (rank + 1)..8 {
                wm |= rank_mask(r) & cols;
            }
            t[0][sq as usize] = wm;

            // Noir : rangs strictement en-dessous du pion
            let mut bm = 0u64;
            for r in 0..rank {
                bm |= rank_mask(r) & cols;
            }
            t[1][sq as usize] = bm;
        }
        t
    })
}

/// Évalue la structure de pions pour une couleur donnée.
/// Retourne un score (positif = bon pour cette couleur).
pub fn pawn_structure_score(board: &Board, color: Color) -> i32 {
    let pawns = board.pieces[color.index()][Piece::Pawn.index()];
    let enemy_pawns = board.pieces[color.opposite().index()][Piece::Pawn.index()];
    let mut score = 0i32;

    // Pour chaque colonne, compter et analyser les pions de cette couleur
    for file in 0u8..8 {
        let col_mask = file_mask(file);
        let pawns_on_file = pawns & col_mask;
        let count = pawns_on_file.count_ones() as i32;

        if count == 0 { continue; }

        // --- Pions doublés ---
        // Si plus d'un pion sur la colonne, pénalité pour les pions en trop.
        if count > 1 {
            score += DOUBLED_PAWN_PENALTY * (count - 1);
        }

        // --- Pions isolés ---
        // Un pion est isolé si aucun pion ami sur les colonnes voisines.
        let left_file  = if file > 0 { file_mask(file - 1) } else { 0 };
        let right_file = if file < 7 { file_mask(file + 1) } else { 0 };
        let adjacent   = left_file | right_file;

        if pawns & adjacent == 0 {
            // Tous les pions sur cette colonne sont isolés
            score += ISOLATED_PAWN_PENALTY * count;
        }

        // --- Pions passés ---
        // Un pion est passé si aucun pion adverse ne se trouve devant lui
        // sur la même colonne ou les colonnes adjacentes.
        //
        // Optimisation : lookup O(1) dans PASSED_PAWN_MASK au lieu de la boucle
        // O(8) `for r in (rank+1)..8 { mask |= rank_mask(r) & blocking_files; }`.
        // La table est initialisée une seule fois (OnceLock), puis un simple
        // accès tableau remplace 8 opérations bitboard par nœud.
        let ppm = get_passed_pawn_mask();
        let color_idx = color.index();
        let mut bb = pawns_on_file;
        while bb != 0 {
            let sq = bb.trailing_zeros() as u8;
            bb    &= bb - 1;

            // Masque précalculé : O(1), zéro boucle.
            if enemy_pawns & ppm[color_idx][sq as usize] == 0 {
                let rank = sq / 8;
                let advancement = match color {
                    Color::White => rank as usize,
                    Color::Black => (7 - rank) as usize,
                };
                score += PASSED_PAWN_BONUS[advancement];
            }
        }
    }

    score
}

// =============================================================================
// Pawn hash table — cache de l'évaluation de structure de pions
// =============================================================================
//
// L'évaluation de structure de pions (pions doublés/isolés/passés) ne dépend
// QUE de la position des pions des deux couleurs — jamais du roi ni des autres
// pièces (vérifié : pawn_structure_score ne lit que les bitboards de pions).
// Or les pions bougent rarement : la même structure réapparaît dans une immense
// proportion des nœuds de recherche. On met donc en cache la valeur calculée,
// ce qui évite de re-balayer 8 colonnes × 2 couleurs (+ détection de pions
// passés) à chaque appel d'evaluate().
//
// CLÉ DU CACHE = la paire de bitboards de pions (blanc, noir) elle-même,
// vérifiée par comparaison EXACTE lors du lookup. Conséquence : AUCUNE fausse
// correspondance possible (contrairement à un hash Zobrist tronqué) — une
// collision d'index provoque au pire un remplacement (recalcul), jamais une
// valeur erronée. Cette approche n'exige AUCUNE modification de make_move /
// unmake_move / Board : tout est contenu ici.
//
// VALEUR mise en cache = score BLANC-RELATIF (blanc − noir), indépendant du
// trait. L'orientation selon le joueur au trait est appliquée APRÈS le lookup,
// exactement comme avant — donc résultat strictement identique (zéro Elo).
//
// Cache THREAD-LOCAL : chaque thread Lazy SMP a le sien (pas de partage, donc
// pas de synchronisation). Les entrées restent valides indéfiniment (une
// structure de pions donnée a toujours le même score — les constantes d'éval
// ne changent pas à l'exécution), donc jamais besoin de vider le cache.

/// Nombre d'entrées du cache (puissance de 2). 8192 × ~24 octets ≈ 192 Kio par
/// thread — largement suffisant vu le faible nombre de structures de pions
/// distinctes rencontrées dans un arbre de recherche.
const PAWN_CACHE_SIZE: usize = 1 << 13;
const PAWN_CACHE_MASK: usize = PAWN_CACHE_SIZE - 1;

/// Une entrée du cache de structure de pions.
#[derive(Clone, Copy)]
struct PawnCacheEntry {
    /// Bitboard des pions blancs (partie de la clé exacte).
    white_pawns: u64,
    /// Bitboard des pions noirs (partie de la clé exacte).
    black_pawns: u64,
    /// Score blanc-relatif (blanc − noir) mémorisé.
    value:       i32,
    /// false = slot vide (jamais écrit) ; distingue un slot libre d'une entrée
    /// réelle dont la valeur vaut 0 (position sans pion, par ex.).
    valid:       bool,
}

const EMPTY_PAWN_ENTRY: PawnCacheEntry = PawnCacheEntry {
    white_pawns: 0,
    black_pawns: 0,
    value:       0,
    valid:       false,
};

thread_local! {
    /// Cache thread-local (Vec alloué sur le tas → pas de gros tableau temporaire
    /// sur la pile à l'initialisation).
    static PAWN_CACHE: RefCell<Vec<PawnCacheEntry>> =
        RefCell::new(vec![EMPTY_PAWN_ENTRY; PAWN_CACHE_SIZE]);
}

/// Mélange les deux bitboards de pions en un index bien distribué (la clé
/// exacte reste les deux bitboards eux-mêmes, vérifiés au lookup).
#[inline]
fn pawn_cache_index(white_pawns: u64, black_pawns: u64) -> usize {
    let mut h = white_pawns.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= black_pawns.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    (h as usize) & PAWN_CACHE_MASK
}

/// Calcule le différentiel de structure de pions du point de vue du joueur actif.
///
/// Passe d'abord par le cache thread-local (clé = bitboards de pions). En cas de
/// miss, calcule le score blanc-relatif via pawn_structure_score() et le mémorise.
/// Le résultat est strictement identique à un calcul direct — seul le coût change.
pub fn pawn_eval(board: &Board) -> i32 {
    let white_pawns = board.pieces[Color::White.index()][Piece::Pawn.index()];
    let black_pawns = board.pieces[Color::Black.index()][Piece::Pawn.index()];

    let white_relative = PAWN_CACHE.with(|cache| {
        let mut c   = cache.borrow_mut();
        let idx     = pawn_cache_index(white_pawns, black_pawns);
        let entry   = c[idx];

        // Hit : même structure exacte (comparaison des deux bitboards complets).
        if entry.valid
            && entry.white_pawns == white_pawns
            && entry.black_pawns == black_pawns
        {
            return entry.value;
        }

        // Miss : calcul puis mémorisation (remplacement simple en cas de collision).
        let value = pawn_structure_score(board, Color::White)
                  - pawn_structure_score(board, Color::Black);
        c[idx] = PawnCacheEntry { white_pawns, black_pawns, value, valid: true };
        value
    });

    if board.side_to_move == Color::White { white_relative } else { -white_relative }
}
