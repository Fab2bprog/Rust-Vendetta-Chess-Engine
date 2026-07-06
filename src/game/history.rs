// =============================================================================
// Vendetta Chess Motor — src/game/history.rs
//
// Rôle : Suivi de l'historique des positions pour la détection de répétition.
//        La règle des 3 répétitions aux échecs stipule qu'une partie est nulle
//        si la même position se répète 3 fois (pas nécessairement consécutivement).
//
// Contenu :
//   - PositionHistory : stockage des hashs Zobrist des positions jouées
//   - count_repetitions() : compte combien de fois la position actuelle
//     a déjà été rencontrée dans la partie
//
// Note : On utilise le hash Zobrist comme identifiant de position.
//        Deux positions identiques ont le même hash (avec une infime probabilité
//        de collision qui est acceptable en pratique).
// =============================================================================

/// Historique des positions d'une partie.
pub struct PositionHistory {
    /// Liste des hashs Zobrist de toutes les positions jouées.
    hashes: Vec<u64>,
}

impl PositionHistory {
    /// Crée un historique vide.
    pub fn new() -> PositionHistory {
        PositionHistory {
            hashes: Vec::with_capacity(256),
        }
    }

    /// Ajoute la position courante à l'historique.
    pub fn push(&mut self, hash: u64) {
        self.hashes.push(hash);
    }

    /// Retire la dernière position de l'historique (lors d'un unmake_move).
    pub fn pop(&mut self) {
        self.hashes.pop();
    }

    /// Compte le nombre de fois que le hash donné apparaît dans l'historique.
    /// Utilisé pour détecter la répétition de position.
    pub fn count_occurrences(&self, hash: u64) -> u32 {
        self.hashes.iter().filter(|&&h| h == hash).count() as u32
    }

    /// Retourne true si la position (identifiée par son hash) s'est déjà
    /// répétée au moins 2 fois (donc cette occurrence serait la 3e → nulle).
    pub fn is_threefold_repetition(&self, hash: u64) -> bool {
        self.count_occurrences(hash) >= 2
    }

    /// Vide l'historique (début d'une nouvelle partie).
    pub fn clear(&mut self) {
        self.hashes.clear();
    }

    /// Retourne le nombre de positions dans l'historique.
    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    /// Retourne true si l'historique est vide (aucune position enregistrée).
    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }
}

impl Default for PositionHistory {
    fn default() -> Self {
        PositionHistory::new()
    }
}
