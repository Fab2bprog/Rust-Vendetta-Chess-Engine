// =============================================================================
// Vendetta Chess Motor — src/search/transposition.rs
//
// Rôle : Table de transposition (TT) thread-safe et lock-free.
//        Cache des positions déjà analysées. Partagée entre tous les threads
//        Lazy SMP via Arc<TranspositionTable>.
//
// Architecture multi-thread :
//   Chaque entrée est stockée sous forme de deux AtomicU64 :
//     - key  : hash Zobrist de la position
//     - data : données compressées (score, profondeur, flag, coup)
//
//   Les lectures/écritures utilisent Ordering::Relaxed pour la performance.
//   Les "races" bénignes (lecture d'une entrée en cours d'écriture par un
//   autre thread) se traduisent par un simple cache-miss — jamais par une
//   erreur de logique.
//
// Encodage du champ `data` (64 bits) :
//   bits  0-20 : score + 1_000_000 (21 bits, valeurs [0, 2_000_000])
//   bits 21-27 : profondeur (7 bits, valeurs [0, 127])
//   bits 28-29 : TTFlag (2 bits : 0=Exact, 1=LowerBound, 2=UpperBound)
//   bits 30-35 : case départ du coup (6 bits)
//   bits 36-41 : case arrivée du coup (6 bits)
//   bits 42-44 : MoveFlags enum (3 bits, valeurs 0-7)
//   bits 45-47 : pièce de promotion (3 bits, valeurs 0-7)
//   bits 48-63 : inutilisés
//
// Politique de remplacement avec numéro de génération (par commande "go") :
//   - Entrée périmée (génération différente) → toujours remplacée.
//   - Même génération → remplacée si profondeur >= ancienne.
// La génération est encodée dans les bits 48-55 du champ data (8 bits, 256 valeurs).
// =============================================================================

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use crate::utils::types::{Move, MoveFlags};

/// Type d'entrée dans la table de transposition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TTFlag {
    /// Score exact (fenêtre alpha-bêta complète traversée).
    Exact      = 0,
    /// Borne inférieure (fail high — score >= beta).
    LowerBound = 1,
    /// Borne supérieure (fail low — score <= alpha).
    UpperBound = 2,
}

/// Une entrée décodée de la table de transposition.
/// Utilisée uniquement comme valeur de retour — pas stockée directement.
#[derive(Clone, Copy, Debug)]
pub struct TTEntry {
    /// Hash Zobrist de la position (pour détecter les collisions).
    pub hash:      u64,
    /// Score de la position.
    pub score:     i32,
    /// Profondeur à laquelle ce score a été calculé.
    pub depth:     i32,
    /// Type du score (exact, borne inférieure ou supérieure).
    pub flag:      TTFlag,
    /// Meilleur coup trouvé pour cette position (pour l'ordonnancement).
    pub best_move: Move,
}

// =============================================================================
// Slot atomique (paire key/data)
// =============================================================================

/// Un slot atomique dans la table de transposition.
/// key et data sont chacun un AtomicU64 pour les accès lock-free.
struct TtSlot {
    /// Hash Zobrist — sert à vérifier qu'on lit la bonne position.
    key:  AtomicU64,
    /// Données compressées (voir encodage dans l'en-tête du fichier).
    data: AtomicU64,
}

// =============================================================================
// Fonctions d'encodage / décodage
// =============================================================================

/// Compresse score, profondeur, flag, coup ET génération dans un u64.
///
/// Encodage (64 bits) :
///   bits  0-20 : score + 1_000_000 (21 bits, [0, 2_000_000])
///   bits 21-27 : profondeur        (7 bits, [0, 127])
///   bits 28-29 : TTFlag            (2 bits : 0=Exact, 1=Lower, 2=Upper)
///   bits 30-35 : case départ       (6 bits)
///   bits 36-41 : case arrivée      (6 bits)
///   bits 42-44 : MoveFlags         (3 bits)
///   bits 45-47 : pièce promotion   (3 bits)
///   bits 48-55 : génération        (8 bits) ← NOUVEAU
///   bits 56-63 : inutilisés
fn pack_data(score: i32, depth: i32, flag: TTFlag, mv: Move, gen: u8) -> u64 {
    let s  = (score + 1_000_000) as u64;   // 21 bits
    let d  = depth as u64;                  //  7 bits
    let f  = flag as u64;                   //  2 bits
    let fr = mv.from as u64;               //  6 bits
    let to = mv.to as u64;                 //  6 bits
    let mf = mv.flags as u64;             //  3 bits
    let p  = mv.promotion as u64;          //  3 bits
    let g  = gen as u64;                   //  8 bits

    s | (d << 21) | (f << 28) | (fr << 30) | (to << 36) | (mf << 42) | (p << 45) | (g << 48)
}

/// Décompresse un u64 en (score, profondeur, flag, coup, génération).
fn unpack_data(data: u64) -> (i32, i32, TTFlag, Move, u8) {
    let score = (data & 0x1F_FFFF) as i32 - 1_000_000;
    let depth = ((data >> 21) & 0x7F) as i32;
    let flag  = match (data >> 28) & 0x3 {
        0 => TTFlag::Exact,
        1 => TTFlag::LowerBound,
        _ => TTFlag::UpperBound,
    };
    let from  = ((data >> 30) & 0x3F) as u8;
    let to    = ((data >> 36) & 0x3F) as u8;
    let mf    = match (data >> 42) & 0x7 {
        0 => MoveFlags::Quiet,
        1 => MoveFlags::DoublePush,
        2 => MoveFlags::CastleKingside,
        3 => MoveFlags::CastleQueenside,
        4 => MoveFlags::Capture,
        5 => MoveFlags::EnPassant,
        6 => MoveFlags::Promotion,
        _ => MoveFlags::PromotionCapture,
    };
    let promo = ((data >> 45) & 0x7) as u8;
    let gen   = ((data >> 48) & 0xFF) as u8;
    let mv    = Move { from, to, flags: mf, promotion: promo };

    (score, depth, flag, mv, gen)
}

// =============================================================================
// Table de transposition
// =============================================================================

/// Table de transposition lock-free, partageable entre threads via Arc.
///
/// Utilise des paires AtomicU64 (key, data) pour chaque entrée.
/// Les races bénignes (lecture d'une entrée en cours d'écriture) se
/// traduisent par un cache-miss — jamais par une erreur de logique.
pub struct TranspositionTable {
    /// Tableau de slots atomiques.
    slots:      Vec<TtSlot>,
    /// Masque d'indexation (slots.len() - 1, toujours une puissance de 2).
    mask:       u64,
    /// Numéro de génération courant (incrémenté à chaque commande "go").
    /// Permet de distinguer les entrées fraîches des entrées périmées.
    generation: AtomicU8,
}

// SAFETY : TtSlot contient uniquement des AtomicU64 qui sont Sync.
// TranspositionTable est donc Sync, et donc Arc<TranspositionTable> est Send+Sync.
unsafe impl Send for TranspositionTable {}
unsafe impl Sync for TranspositionTable {}

impl TranspositionTable {
    /// Tente de créer une table de transposition de `size_mb` Mo, SANS jamais
    /// avorter le programme. La taille réelle est arrondie à la puissance de 2
    /// inférieure. Retourne `None` si l'allocation échoue (mémoire insuffisante).
    ///
    /// Robustesse : l'allocation utilise `Vec::try_reserve_exact`, qui renvoie
    /// une erreur au lieu d'appeler `handle_alloc_error` (abandon du processus)
    /// quand l'allocateur ne peut pas fournir la mémoire. C'est la base du repli
    /// gracieux côté UCI : un réglage `Hash` trop ambitieux ne tue plus le moteur.
    ///
    /// Limite honnête : `try_reserve` capte les REFUS FRANCS de l'allocateur (le
    /// vecteur de crash le plus courant). Sur les systèmes à sur-engagement
    /// mémoire (overcommit), une réservation peut « réussir » virtuellement puis
    /// pressurer la RAM lors du remplissage — ce cas-là exigerait d'interroger la
    /// RAM physique (hors bibliothèque standard). Le repli reste une nette
    /// amélioration : plus aucun abandon sur refus d'allocation.
    pub fn try_new(size_mb: usize) -> Option<TranspositionTable> {
        // 2 AtomicU64 par slot = 16 octets par slot
        let bytes_per_slot = 16usize;
        let total_bytes    = size_mb.saturating_mul(1024 * 1024);
        let num_slots_raw  = total_bytes / bytes_per_slot;

        // Puissance de 2 inférieure ou égale
        let mut num_slots = 1usize;
        while num_slots * 2 <= num_slots_raw {
            num_slots *= 2;
        }
        if num_slots == 0 { num_slots = 1; }

        let mask = (num_slots - 1) as u64;

        // Allocation FAILLIBLE : try_reserve_exact renvoie Err au lieu d'avorter.
        let mut slots: Vec<TtSlot> = Vec::new();
        if slots.try_reserve_exact(num_slots).is_err() {
            return None;
        }
        // La capacité est désormais garantie → ces push ne réallouent jamais
        // (donc ne peuvent pas échouer).
        for _ in 0..num_slots {
            slots.push(TtSlot { key: AtomicU64::new(0), data: AtomicU64::new(0) });
        }

        Some(TranspositionTable { slots, mask, generation: AtomicU8::new(0) })
    }

    /// Crée une table de transposition de `size_mb` Mo avec REPLI GRACIEUX
    /// garanti : si l'allocation échoue, la taille est divisée par deux jusqu'à
    /// réussir. Ne panique JAMAIS — cohérent avec la priorité robustesse du
    /// moteur. Utilisé au démarrage (où une petite taille réussit de toute façon).
    /// Pour piloter finement le repli (message UCI, taille réelle retenue), la
    /// couche UCI appelle plutôt `try_new()` directement.
    pub fn new(size_mb: usize) -> TranspositionTable {
        let mut try_size = size_mb.max(1);
        loop {
            if let Some(tt) = Self::try_new(try_size) {
                return tt;
            }
            if try_size <= 1 {
                break; // même 1 Mo échoue : repli minimal ci-dessous
            }
            try_size /= 2;
        }

        // Dernier recours absolu (système quasiment sans mémoire) : table d'un
        // seul slot (mask = 0 → tout indexe le slot 0). Inefficace mais VALIDE
        // et sans crash — toujours mieux que d'avorter.
        TranspositionTable {
            slots: vec![TtSlot { key: AtomicU64::new(0), data: AtomicU64::new(0) }],
            mask: 0,
            generation: AtomicU8::new(0),
        }
    }

    /// Calcule l'index d'un hash dans la table.
    #[inline]
    fn index(&self, hash: u64) -> usize {
        (hash & self.mask) as usize
    }

    /// Précharge dans le cache la ligne contenant le slot associé à `hash`, sans
    /// le lire ni rien retourner. À appeler dès que le hash de la position ENFANT
    /// est connu (juste après make_move), AVANT la descente récursive : la latence
    /// d'accès à la TT (souvent 64 Mio, fréquemment hors cache) est alors masquée
    /// par le travail qui suit (gives_check, extensions, LMP…), de sorte que le
    /// slot est déjà chaud quand l'enfant appelle probe().
    ///
    /// SÛRETÉ des blocs `unsafe` : les instructions de préchargement (`prfm` sur
    /// aarch64, `_mm_prefetch` sur x86-64) sont de simples INDICATIONS matérielles.
    /// Elles ne font JAMAIS faute — même sur une adresse invalide — et ne modifient
    /// aucun état architectural observable. De plus le pointeur provient d'un slot
    /// indexé dans les bornes (index() applique `& mask`), donc toujours valide.
    /// Sur les autres architectures : no-op (le préchargement n'est qu'une
    /// optimisation, jamais une nécessité de correction).
    #[inline(always)]
    pub fn prefetch(&self, hash: u64) {
        let slot_ptr = &self.slots[self.index(hash)] as *const TtSlot;

        #[cfg(target_arch = "aarch64")]
        // SAFETY : prfm est une indication de préchargement qui ne faute jamais
        // et ne modifie aucun état observable (voir la doc ci-dessus).
        unsafe {
            core::arch::asm!(
                "prfm pldl1keep, [{ptr}]",
                ptr = in(reg) slot_ptr,
                options(nostack, readonly, preserves_flags),
            );
        }

        #[cfg(target_arch = "x86_64")]
        // SAFETY : _mm_prefetch est une indication de préchargement qui ne faute
        // jamais et ne modifie aucun état observable (voir la doc ci-dessus).
        unsafe {
            core::arch::x86_64::_mm_prefetch(
                slot_ptr as *const i8,
                core::arch::x86_64::_MM_HINT_T0,
            );
        }

        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        {
            // Autres architectures : no-op. `let _` évite un warning unused.
            let _ = slot_ptr;
        }
    }

    /// Sonde la table pour un hash donné.
    /// Retourne Some(entry) si une entrée valide est trouvée, None sinon.
    ///
    /// Thread-safe : les lectures Relaxed sont cohérentes pour une cache.
    pub fn probe(&self, hash: u64) -> Option<TTEntry> {
        let slot = &self.slots[self.index(hash)];
        let k    = slot.key.load(Ordering::Relaxed);
        let d    = slot.data.load(Ordering::Relaxed);

        // Vérifier le hash et que le slot n'est pas vide
        if k != hash || d == 0 {
            return None;
        }

        let (score, depth, flag, best_move, _gen) = unpack_data(d);
        Some(TTEntry { hash, score, depth, flag, best_move })
    }

    /// Stocke une entrée dans la table.
    ///
    /// Politique de remplacement par génération + profondeur :
    ///   - Slot vide → toujours écrire.
    ///   - Génération différente (entrée périmée d'une recherche précédente)
    ///     → toujours remplacer : une entrée fraîche à profondeur 1 vaut mieux
    ///     qu'une entrée périmée à profondeur 8.
    ///   - Même génération → remplacer uniquement si profondeur >= ancienne.
    ///
    /// Thread-safe : les écritures Relaxed sont suffisantes pour une cache.
    pub fn store(
        &self,
        hash:      u64,
        score:     i32,
        depth:     i32,
        flag:      TTFlag,
        best_move: Move,
    ) {
        let slot        = &self.slots[self.index(hash)];
        let old_data    = slot.data.load(Ordering::Relaxed);
        let current_gen = self.generation.load(Ordering::Relaxed);

        if old_data != 0 {
            let (_, old_depth, _, _, old_gen) = unpack_data(old_data);
            // Même génération : conserver les entrées plus profondes.
            // Génération différente : toujours remplacer (entrée périmée).
            if old_gen == current_gen && depth < old_depth {
                return;
            }
        }

        let data = pack_data(score, depth, flag, best_move, current_gen);
        // Écrire data AVANT key pour minimiser les races bénignes
        slot.data.store(data, Ordering::Relaxed);
        slot.key.store(hash,  Ordering::Relaxed);
    }

    /// Incrémente le numéro de génération au début d'une nouvelle recherche.
    ///
    /// Toutes les entrées existantes seront considérées périmées lors du prochain
    /// store(), et remplaceables même par des entrées à profondeur plus faible.
    /// Appeler une fois au début de chaque commande "go".
    pub fn new_search(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Vide toute la table (entre deux parties).
    /// Thread-safe : utilise des stores atomiques.
    pub fn clear(&self) {
        for slot in &self.slots {
            slot.key.store(0,  Ordering::Relaxed);
            slot.data.store(0, Ordering::Relaxed);
        }
    }

    /// Estime le taux de remplissage de la table en permills (0–1000).
    ///
    /// Échantillonne les 1 000 premiers slots (ou tous si la table est plus petite).
    /// Chaque slot dont le champ `data` est non nul est considéré comme occupé.
    /// Utilisé par le protocole UCI (commande "info hashfull <n>").
    pub fn hashfull(&self) -> u32 {
        let sample = self.slots.len().min(1000);
        if sample == 0 { return 0; }
        let filled = self.slots[..sample]
            .iter()
            .filter(|s| s.data.load(Ordering::Relaxed) != 0)
            .count();
        (filled * 1000 / sample) as u32
    }

    /// Ajuste un score de mat issu de la table pour la profondeur actuelle.
    /// Les scores de mat sont stockés relatifs à la racine.
    pub fn adjust_score_from_tt(score: i32, ply: i32) -> i32 {
        use crate::utils::types::SCORE_MATE;
        if score > SCORE_MATE - 200 {
            score - ply
        } else if score < -SCORE_MATE + 200 {
            score + ply
        } else {
            score
        }
    }

    /// Ajuste un score de mat pour le stockage dans la table.
    pub fn adjust_score_for_tt(score: i32, ply: i32) -> i32 {
        use crate::utils::types::SCORE_MATE;
        if score > SCORE_MATE - 200 {
            score + ply
        } else if score < -SCORE_MATE + 200 {
            score - ply
        } else {
            score
        }
    }
}
