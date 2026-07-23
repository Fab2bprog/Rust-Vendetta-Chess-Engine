// =============================================================================
// Vendetta Chess Motor — src/search/killers.rs
//
// Rôle : Gestion des "killer moves" (coups tueurs).
//        Un killer move est un coup silencieux (sans capture) qui a causé une
//        coupure bêta à la même profondeur dans l'arbre de recherche.
//        On le teste en priorité car il peut être efficace dans d'autres branches.
//
// Contenu :
//   - KillerMoves : stockage de 2 killer moves par profondeur
//   - Mise à jour lors d'une coupure bêta
//   - Vérification si un coup est un killer move
//
// Pourquoi 2 killers par profondeur ?
//   Un seul killer n'est pas toujours applicable (même si bon, il peut être
//   illégal dans la position courante). Deux killers augmentent les chances
//   d'en avoir un valide.
// =============================================================================

use crate::utils::types::Move;
use super::alphabeta::MAX_PLY;

// Profondeur maximale de recherche supportée.
//
// BUG CORRIGÉ (audit post-session) : ce fichier définissait auparavant sa
// PROPRE constante MAX_PLY = 128, distincte de celle d'alphabeta.rs (192).
// Au-delà de la profondeur 128, les killer moves étaient donc silencieusement
// désactivés (store()/is_killer()/get() retournaient tous un no-op), alors que
// la recherche peut légitimement atteindre jusqu'à 192 plies avec les
// extensions. Pas un crash, mais une perte d'efficacité inutile et une source
// de confusion (deux constantes de même nom, valeurs différentes, aucun lien
// entre elles). Importée depuis alphabeta.rs : une seule source de vérité.

/// Gestionnaire des killer moves.
/// Pour chaque niveau de profondeur (ply), on stocke jusqu'à 2 killer moves.
pub struct KillerMoves {
    /// killers[ply][0] et killers[ply][1] : les deux killers pour ce ply.
    killers: [[Move; 2]; MAX_PLY],
}

impl KillerMoves {
    /// Crée un nouveau gestionnaire de killers (vide).
    pub fn new() -> KillerMoves {
        KillerMoves {
            killers: [[Move::NULL; 2]; MAX_PLY],
        }
    }

    /// Enregistre un killer move pour la profondeur `ply`.
    /// Si le coup est déjà le premier killer, ne fait rien.
    /// Sinon, décale le premier en deuxième et met le nouveau en premier.
    pub fn store(&mut self, mv: Move, ply: usize) {
        if ply >= MAX_PLY { return; }

        // Ne pas stocker les captures comme killers
        if mv.flags.is_capture() { return; }

        // Éviter les doublons
        if self.killers[ply][0] == mv { return; }

        // Décaler et stocker
        self.killers[ply][1] = self.killers[ply][0];
        self.killers[ply][0] = mv;
    }

    /// Retourne true si le coup est un killer move pour la profondeur `ply`.
    pub fn is_killer(&self, mv: Move, ply: usize) -> bool {
        if ply >= MAX_PLY { return false; }
        self.killers[ply][0] == mv || self.killers[ply][1] == mv
    }

    /// Retourne les killers pour une profondeur donnée.
    pub fn get(&self, ply: usize) -> [Move; 2] {
        if ply >= MAX_PLY { return [Move::NULL; 2]; }
        self.killers[ply]
    }

    /// Remet à zéro tous les killers (entre deux recherches).
    pub fn clear(&mut self) {
        self.killers = [[Move::NULL; 2]; MAX_PLY];
    }
}

impl Default for KillerMoves {
    fn default() -> Self {
        KillerMoves::new()
    }
}
