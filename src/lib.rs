// =============================================================================
// Vendetta Chess Motor — src/lib.rs
//
// Rôle : Racine de la bibliothèque. Déclare tous les modules du projet et
//        les réexporte pour faciliter leur utilisation dans les tests et
//        les futurs composants externes (future GUI, etc.).
//
// Structure des modules :
//   utils   → types communs (Color, Piece, Move, constantes...)
//   board   → représentation de l'échiquier (bitboards, état, FEN)
//   moves   → génération des coups légaux
//   eval    → évaluation statique des positions
//   search  → algorithme de recherche alpha-bêta
//   game    → gestion de la partie (historique, règles de fin)
//   uci     → protocole UCI (communication avec l'interface graphique)
// =============================================================================

pub mod utils;
pub mod board;
pub mod moves;
pub mod eval;
pub mod search;
pub mod game;
pub mod uci;
