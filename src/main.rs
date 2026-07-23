// =============================================================================
// Vendetta Chess Motor — src/main.rs
//
// Rôle : Point d'entrée du programme. Initialise les tables nécessaires
//        et lance la boucle principale UCI.
//
// Au démarrage :
//   1. Initialisation des tables d'attaque précalculées (cavalier, roi)
//   2. Lancement de la boucle UCI (lecture de stdin, écriture sur stdout)
//
// Le moteur communique exclusivement via stdin/stdout selon le protocole UCI.
// Il ne doit pas afficher d'interface graphique ni ouvrir de fenêtre.
// =============================================================================

use vendetta_chess_motor::board::bitboard::init_attack_tables;
use vendetta_chess_motor::uci::UciEngine;

fn main() {
    // Initialiser les tables d'attaque précalculées pour le cavalier et le roi.
    // Doit être fait avant toute utilisation des fonctions de génération de coups.
    init_attack_tables();

    // Lancer le moteur UCI
    let mut engine = UciEngine::new();
    engine.run();
}
