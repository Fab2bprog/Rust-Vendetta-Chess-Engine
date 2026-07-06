// =============================================================================
// Vendetta Chess Motor — src/board/magic.rs
//
// Rôle : Magic Bitboards pour le calcul ultra-rapide des attaques des pièces
//        glissantes (tour et fou). Remplace les boucles de bitboard.rs par une
//        simple consultation de table en temps constant.
//
// Principe :
//   Pour une pièce sur la case `sq` avec l'occupancy `occ` :
//     1. Masquer l'occupancy pertinente : occ & mask[sq]
//     2. Multiplier par le nombre magique : masked × magic[sq]
//     3. Décaler à droite : >> shift[sq]
//     4. Consulter la table  : table[sq × TAILLE + index]
//
//   Cette formule, en une seule multiplication, compresse les bits pertinents
//   de l'occupancy vers les bits de poids fort, produisant un index compact.
//
// Nombres magiques :
//   Trouvés au démarrage par essai aléatoire (xorshift64 épars).
//   Convergence garantie : un bon magique existe pour chaque case.
//   Durée typique : < 10 ms pour les 128 cases (64 tours + 64 fous).
//
// Stockage (tables plates, offset = sq × TAILLE_MAX) :
//   Tours  : 64 × 4096 × 8 octets = 2 Mo  (max 12 bits de masque → 2^12 = 4096)
//   Fous   : 64 × 512  × 8 octets = 256 Ko (max  9 bits de masque → 2^9  =  512)
//   Total  : ~2,25 Mo alloués sur le tas, accessibles en lecture seule ensuite.
//
// Thread-safety :
//   OnceLock garantit que l'initialisation n'est exécutée qu'une seule fois,
//   même si plusieurs threads appellent init_magic_tables() simultanément.
// =============================================================================

use std::sync::OnceLock;

// =============================================================================
// Structure de données
// =============================================================================

struct MagicTables {
    /// Masque d'occupancy pertinente pour chaque case (tour).
    /// Les bords de l'échiquier sont exclus car ils ne changent pas la mobilité.
    rook_masks:    [u64; 64],
    /// Masque d'occupancy pertinente pour chaque case (fou).
    bishop_masks:  [u64; 64],
    /// Nombre magique par case (tour).
    rook_magics:   [u64; 64],
    /// Nombre magique par case (fou).
    bishop_magics: [u64; 64],
    /// Décalage = 64 − popcount(masque) par case (tour).
    rook_shifts:   [u32; 64],
    /// Décalage = 64 − popcount(masque) par case (fou).
    bishop_shifts: [u32; 64],
    /// Tables plates des attaques de tours : index = sq × 4096 + magic_index.
    rook_table:    Vec<u64>,
    /// Tables plates des attaques de fous  : index = sq × 512  + magic_index.
    bishop_table:  Vec<u64>,
}

/// Tables globales, initialisées une seule fois au démarrage.
static MAGIC_TABLES: OnceLock<MagicTables> = OnceLock::new();

// =============================================================================
// Calcul des masques
// =============================================================================

/// Masque d'occupancy pour une tour sur `sq`.
///
/// Contient toutes les cases sur la même rangée et la même colonne,
/// en excluant les cases de bord (elles ne bloquent pas la mobilité de la tour).
/// La case `sq` elle-même est également exclue.
fn rook_mask(sq: u8) -> u64 {
    let rank = (sq / 8) as i32;
    let file = (sq % 8) as i32;
    let mut mask = 0u64;

    // Même rangée : colonnes b–g seulement (a et h exclues comme bords)
    for f in 1..7_i32 {
        if f != file {
            mask |= 1u64 << (rank * 8 + f);
        }
    }
    // Même colonne : rangées 2–7 seulement (1 et 8 exclues comme bords)
    for r in 1..7_i32 {
        if r != rank {
            mask |= 1u64 << (r * 8 + file);
        }
    }
    mask
}

/// Masque d'occupancy pour un fou sur `sq`.
///
/// Contient toutes les cases sur les 4 diagonales, en excluant les bords.
fn bishop_mask(sq: u8) -> u64 {
    let rank = (sq / 8) as i32;
    let file = (sq % 8) as i32;
    let mut mask = 0u64;

    // 4 directions diagonales, en excluant strictement les bords (r ∈ ]0,7[, f ∈ ]0,7[)
    for (dr, df) in [(1_i32, 1_i32), (1, -1), (-1, 1), (-1, -1)] {
        let (mut r, mut f) = (rank + dr, file + df);
        while r > 0 && r < 7 && f > 0 && f < 7 {
            mask |= 1u64 << (r * 8 + f);
            r += dr;
            f += df;
        }
    }
    mask
}

// =============================================================================
// Calcul lent des attaques (utilisé uniquement à l'initialisation)
// =============================================================================

/// Attaques d'une tour sur `sq` avec l'occupancy `occ` — version classique lente.
/// Utilisé uniquement pour peupler les tables magiques au démarrage.
fn slow_rook_attacks(sq: u8, occ: u64) -> u64 {
    let rank = (sq / 8) as i32;
    let file = (sq % 8) as i32;
    let mut attacks = 0u64;

    for (dr, df) in [(1_i32, 0_i32), (-1, 0), (0, 1), (0, -1)] {
        let (mut r, mut f) = (rank + dr, file + df);
        while (0..8).contains(&r) && (0..8).contains(&f) {
            let s = (r * 8 + f) as u8;
            attacks |= 1u64 << s;
            if occ & (1u64 << s) != 0 { break; }
            r += dr;
            f += df;
        }
    }
    attacks
}

/// Attaques d'un fou sur `sq` avec l'occupancy `occ` — version classique lente.
/// Utilisé uniquement pour peupler les tables magiques au démarrage.
fn slow_bishop_attacks(sq: u8, occ: u64) -> u64 {
    let rank = (sq / 8) as i32;
    let file = (sq % 8) as i32;
    let mut attacks = 0u64;

    for (dr, df) in [(1_i32, 1_i32), (1, -1), (-1, 1), (-1, -1)] {
        let (mut r, mut f) = (rank + dr, file + df);
        while (0..8).contains(&r) && (0..8).contains(&f) {
            let s = (r * 8 + f) as u8;
            attacks |= 1u64 << s;
            if occ & (1u64 << s) != 0 { break; }
            r += dr;
            f += df;
        }
    }
    attacks
}

// =============================================================================
// Recherche des nombres magiques
// =============================================================================

/// Générateur de nombres pseudo-aléatoires xorshift64.
/// Rapide, suffisant pour la recherche de magiques.
#[inline]
fn xorshift64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Génère un nombre aléatoire épars (peu de bits à 1).
/// Les bons nombres magiques tendent à être épars — cette heuristique
/// accélère la convergence d'un facteur 5 à 10 en moyenne.
#[inline]
fn sparse_random(state: &mut u64) -> u64 {
    xorshift64(state) & xorshift64(state) & xorshift64(state)
}

/// Trouve un nombre magique valide pour la case `sq`.
///
/// Algorithme :
///   1. Énumérer tous les 2^N sous-ensembles du masque (carry-rippler).
///   2. Pour chaque candidat magique, vérifier l'absence de collision :
///      deux occupancies différentes doivent produire des indices différents
///      (ou le même indice si leurs attaques sont identiques — constructive).
///   3. Recommencer avec un nouveau candidat jusqu'à trouver un magique valide.
///
/// La graine est unique par case pour diversifier les espaces de recherche.
fn find_magic(sq: u8, mask: u64, is_rook: bool) -> u64 {
    let bits  = mask.count_ones() as usize;
    let shift = (64 - bits) as u32;
    let n     = 1usize << bits;

    // Pré-calculer tous les sous-ensembles du masque et leurs attaques correspondantes.
    // La carry-rippler énumère les 2^bits sous-ensembles dans l'ordre décroissant.
    let mut occs    = vec![0u64; n];
    let mut attacks = vec![0u64; n];

    let mut subset = mask;
    let mut i = 0usize;
    loop {
        occs[i] = subset;
        attacks[i] = if is_rook {
            slow_rook_attacks(sq, subset)
        } else {
            slow_bishop_attacks(sq, subset)
        };
        i += 1;
        if subset == 0 { break; }
        subset = (subset - 1) & mask; // Prochain sous-ensemble (carry-rippler)
    }

    // Table temporaire pour la vérification des collisions.
    // Une entrée à 0 signifie "jamais visitée" (les attaques sont toujours > 0).
    let mut used = vec![0u64; n];

    // Graine unique par case pour diversifier la recherche
    let mut rng = 0xDEADBEEFCAFEBABEu64
        ^ ((sq as u64).wrapping_mul(0x9E3779B97F4A7C15));

    // Compteur de sécurité : en théorie, un nombre magique valide existe toujours
    // pour les cases d'un échiquier 8×8. En pratique, la convergence est < 10 ms
    // pour les 128 cases (tours + fous). Si cette borne est atteinte, cela indique
    // un bug dans la génération du masque (mask == 0, bits incorrects, etc.).
    // Valeur choisie : 100 millions >> largement supérieure aux pires cas observés
    // (~10 000 tentatives pour les cases difficiles), sans risque de faux positif.
    const MAX_MAGIC_ATTEMPTS: u64 = 100_000_000;
    let mut attempts: u64 = 0;

    'outer: loop {
        attempts += 1;
        if attempts > MAX_MAGIC_ATTEMPTS {
            panic!(
                "find_magic : impossible de trouver un nombre magique valide pour la case {} \
                 après {} tentatives. mask=0x{:016X}, is_rook={}. \
                 Cela indique un bug dans la génération du masque.",
                sq, MAX_MAGIC_ATTEMPTS, mask, is_rook
            );
        }

        let magic = sparse_random(&mut rng);

        // Filtre rapide : un bon magique doit "disperser" les bits du masque.
        // Heuristique classique : au moins 6 bits à 1 dans les 8 bits de poids fort
        // du produit (mask × magic). Rejette ~80 % des mauvais candidats d'emblée.
        if (mask.wrapping_mul(magic) >> 56).count_ones() < 6 {
            continue;
        }

        // Réinitialiser la table de vérification
        used.fill(0);

        // Vérifier l'absence de collision pour tous les sous-ensembles
        for j in 0..n {
            let idx = ((occs[j].wrapping_mul(magic)) >> shift) as usize;

            if used[idx] == 0 {
                // Première fois que cet index est utilisé : enregistrer l'attaque
                used[idx] = attacks[j];
            } else if used[idx] != attacks[j] {
                // Collision destructive : deux attaques différentes sur le même index
                continue 'outer;
            }
            // Collision constructive (used[idx] == attacks[j]) : acceptable
        }

        // Aucune collision destructive → magique valide trouvé
        return magic;
    }
}

// =============================================================================
// Initialisation (appelée une seule fois au démarrage)
// =============================================================================

/// Initialise les tables magiques pour les 64 cases, tours et fous.
///
/// Cette fonction est appelée depuis `init_attack_tables()` dans bitboard.rs.
/// Elle est idempotente et thread-safe (OnceLock).
/// Durée typique : < 10 ms sur un CPU moderne.
pub fn init_magic_tables() {
    MAGIC_TABLES.get_or_init(|| {
        let mut rook_masks    = [0u64; 64];
        let mut bishop_masks  = [0u64; 64];
        let mut rook_magics   = [0u64; 64];
        let mut bishop_magics = [0u64; 64];
        let mut rook_shifts   = [0u32; 64];
        let mut bishop_shifts = [0u32; 64];

        // Tables plates allouées sur le tas pour éviter tout débordement de pile.
        // Offset d'accès : sq × TAILLE_MAX + magic_index
        let mut rook_table   = vec![0u64; 64 * 4096]; // 2 Mo
        let mut bishop_table = vec![0u64; 64 * 512];  // 256 Ko

        for sq in 0u8..64 {
            // ----------------------------------------------------------------
            // Tour
            // ----------------------------------------------------------------
            let r_mask  = rook_mask(sq);
            let r_magic = find_magic(sq, r_mask, true);
            let r_shift = 64 - r_mask.count_ones();

            rook_masks[sq as usize]  = r_mask;
            rook_magics[sq as usize] = r_magic;
            rook_shifts[sq as usize] = r_shift;

            // Peupler la table : pour chaque sous-ensemble de r_mask, calculer
            // l'index magique et stocker les attaques correspondantes.
            let base = sq as usize * 4096;
            let mut subset = r_mask;
            loop {
                let idx = ((subset.wrapping_mul(r_magic)) >> r_shift) as usize;
                rook_table[base + idx] = slow_rook_attacks(sq, subset);
                if subset == 0 { break; }
                subset = (subset - 1) & r_mask;
            }

            // ----------------------------------------------------------------
            // Fou
            // ----------------------------------------------------------------
            let b_mask  = bishop_mask(sq);
            let b_magic = find_magic(sq, b_mask, false);
            let b_shift = 64 - b_mask.count_ones();

            bishop_masks[sq as usize]  = b_mask;
            bishop_magics[sq as usize] = b_magic;
            bishop_shifts[sq as usize] = b_shift;

            let base = sq as usize * 512;
            let mut subset = b_mask;
            loop {
                let idx = ((subset.wrapping_mul(b_magic)) >> b_shift) as usize;
                bishop_table[base + idx] = slow_bishop_attacks(sq, subset);
                if subset == 0 { break; }
                subset = (subset - 1) & b_mask;
            }
        }

        MagicTables {
            rook_masks,
            bishop_masks,
            rook_magics,
            bishop_magics,
            rook_shifts,
            bishop_shifts,
            rook_table,
            bishop_table,
        }
    });
}

// =============================================================================
// Fonctions d'attaque publiques
// =============================================================================

/// Retourne les cases attaquées par une tour sur `sq` avec l'occupancy `occ`.
///
/// Version Magic Bitboards : O(1) — une multiplication, un décalage, un lookup.
#[inline]
pub fn rook_attacks_magic(sq: u8, occ: u64) -> u64 {
    let t     = MAGIC_TABLES.get()
        .expect("init_magic_tables() non appelée avant rook_attacks_magic()");
    let mask  = t.rook_masks[sq as usize];
    let magic = t.rook_magics[sq as usize];
    let shift = t.rook_shifts[sq as usize];
    let idx   = ((occ & mask).wrapping_mul(magic) >> shift) as usize;
    t.rook_table[sq as usize * 4096 + idx]
}

/// Retourne les cases attaquées par un fou sur `sq` avec l'occupancy `occ`.
///
/// Version Magic Bitboards : O(1) — une multiplication, un décalage, un lookup.
#[inline]
pub fn bishop_attacks_magic(sq: u8, occ: u64) -> u64 {
    let t     = MAGIC_TABLES.get()
        .expect("init_magic_tables() non appelée avant bishop_attacks_magic()");
    let mask  = t.bishop_masks[sq as usize];
    let magic = t.bishop_magics[sq as usize];
    let shift = t.bishop_shifts[sq as usize];
    let idx   = ((occ & mask).wrapping_mul(magic) >> shift) as usize;
    t.bishop_table[sq as usize * 512 + idx]
}
