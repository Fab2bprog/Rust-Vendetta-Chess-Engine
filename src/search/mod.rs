// =============================================================================
// Vendetta Chess Motor — src/search/mod.rs
//
// Rôle : Coordinateur de la recherche. Implémente l'iterative deepening,
//        le multi-threading Lazy SMP, la gestion du temps, les niveaux de
//        difficulté, et l'interface entre l'UCI et l'algorithme alpha-bêta.
//
// Architecture multi-thread (Lazy SMP) :
//   - Le thread principal fait la recherche normale avec iterative deepening.
//   - Les threads secondaires font chacun leur propre recherche indépendante
//     sur leur propre copie du plateau (Board::clone()).
//   - Tous les threads partagent la même table de transposition (Arc<TT>).
//   - Un Arc<AtomicBool> sert de signal d'arrêt partagé :
//     quand le temps est écoulé (thread principal), tous les threads s'arrêtent.
//
// Bénéfice du Lazy SMP :
//   Les threads secondaires peuplent la TT partagée avec des évaluations
//   à diverses profondeurs. Le thread principal en bénéficie via les TT hits,
//   ce qui améliore l'ordonnancement et accélère la recherche.
//
// Iterative Deepening :
//   On explore profondeur 1, puis 2, puis 3, etc.
//   À chaque profondeur, on garde le meilleur coup trouvé.
//   Si le temps s'épuise, on retourne le meilleur coup de la dernière
//   profondeur complétée. Cela garantit toujours un coup valide.
// =============================================================================

pub mod transposition;
pub mod killers;
pub mod history;
pub mod countermove;
pub mod continuation_history;
pub mod see;
pub mod alphabeta;

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use crate::utils::types::{Move, SCORE_MATE};
use crate::board::state::Board;
use crate::moves::generate_legal_moves;
use transposition::TranspositionTable;
use killers::KillerMoves;
use history::HistoryTable;
use countermove::CountermoveTable;
use continuation_history::ContinuationHistoryTable;
use alphabeta::{alpha_beta, MAX_PLY};

/// Valeur sentinelle indiquant qu'aucune évaluation statique n'a été
/// enregistrée à ce ply pour la branche courante (nœud en échec, racine,
/// ou vérification SE — voir alphabeta.rs::static_eval_opt). Utilisée par
/// le drapeau "improving" (RFP/LMP/NMP) : distinguer "pas de donnée" d'une
/// évaluation réelle, qui reste toujours bornée par ±SCORE_MATE, largement
/// au-dessus de i32::MIN.
pub const EVAL_HISTORY_NONE: i32 = i32::MIN;

// =============================================================================
// Correction History — paramètres et structure (rework étapes A + B)
// =============================================================================
//
// Ajuste l'éval statique d'un nœud selon l'écart historique observé entre l'éval
// statique et le score réel de la recherche, pour des positions partageant une
// même sous-structure. Une éval mieux calibrée → meilleures décisions d'élagage
// (RFP, NMP, futility, improving). Tables PAR THREAD (rangées dans SearchInfo),
// indexées par le camp au trait. Désactivable via SearchInfo.toggles.disable_correction.
//
// Rework par rapport à la v1 (qui ressortait à -3 Elo en SPRT) :
//   A) Apprentissage PONDÉRÉ PAR LA PROFONDEUR, en POINT FIXE (résolution
//      sous-centipion) : une correction issue d'une recherche profonde (fiable)
//      pèse davantage qu'une issue d'une recherche superficielle (bruitée). La
//      v1 utilisait un pas fixe (CORRHIST_RATE) qui ignorait cette fiabilité.
//   B) PLUSIEURS tables combinées par MOYENNE PONDÉRÉE (le bruit d'une clé
//      isolée se moyenne, au lieu de perturber directement l'éval) :
//        - structure de pions,
//        - pièces non-pion blanches,
//        - pièces non-pion noires,
//        - continuation : (type de pièce, case d'arrivée) du dernier coup.

/// Nombre d'entrées par couleur pour les tables pion / non-pion (puissance de 2).
const CORRHIST_SIZE: usize = 1 << 14; // 16384
const CORRHIST_MASK: usize = CORRHIST_SIZE - 1;
/// Entrées par couleur de la table de continuation : (type de pièce × case).
const CORRHIST_CONT_SIZE: usize = 6 * 64;
/// Échelle de point fixe : les valeurs sont stockées en centipions × GRAIN.
const CORRHIST_GRAIN: i32 = 256;
/// Correction maximale stockée/appliquée PAR TABLE (± centipions).
const CORRHIST_MAX_CP: i32 = 64;
const CORRHIST_LIMIT: i32 = CORRHIST_MAX_CP * CORRHIST_GRAIN;
/// Dénominateur de la moyenne mobile (le poids effectif ∈ [1, WEIGHT_CAP]).
const CORRHIST_WEIGHT_SCALE: i32 = 256;
/// Poids maximal d'une mise à jour, atteint à grande profondeur.
const CORRHIST_WEIGHT_CAP: i32 = 16;
// Poids relatifs de chaque table dans la correction finale (moyenne pondérée).
const CW_PAWN:    i32 = 2;
const CW_NONPAWN: i32 = 1;
const CW_CONT:    i32 = 2;

/// Clés pré-calculées (une fois par nœud) pour indexer les tables de Correction
/// History. Calculées par `corr_keys()` dans alphabeta.rs (qui a accès au Board).
pub struct CorrKeys {
    /// Camp au trait (0 = Blancs, 1 = Noirs).
    pub stm:       usize,
    /// Clé de la structure de pions (les deux bitboards de pions mélangés).
    pub pawn:      u64,
    /// Clé des pièces non-pion blanches.
    pub nonpawn_w: u64,
    /// Clé des pièces non-pion noires.
    pub nonpawn_b: u64,
    /// Index continuation : `type_pièce × 64 + case` du dernier coup, ou None
    /// (racine, coup nul, ou case d'arrivée vide — ne devrait pas arriver).
    pub cont:      Option<usize>,
}

/// Tables de Correction History (par thread). Valeurs en point fixe (× GRAIN).
pub struct CorrectionHistory {
    pawn:      Vec<i32>, // [stm × CORRHIST_SIZE + (pawn & MASK)]
    nonpawn_w: Vec<i32>,
    nonpawn_b: Vec<i32>,
    cont:      Vec<i32>, // [stm × CORRHIST_CONT_SIZE + (type_pièce × 64 + case)]
}

impl Default for CorrectionHistory {
    fn default() -> CorrectionHistory { CorrectionHistory::new() }
}

impl CorrectionHistory {
    pub fn new() -> CorrectionHistory {
        CorrectionHistory {
            pawn:      vec![0; 2 * CORRHIST_SIZE],
            nonpawn_w: vec![0; 2 * CORRHIST_SIZE],
            nonpawn_b: vec![0; 2 * CORRHIST_SIZE],
            cont:      vec![0; 2 * CORRHIST_CONT_SIZE],
        }
    }

    #[inline]
    fn pawn_idx(k: &CorrKeys) -> usize { k.stm * CORRHIST_SIZE + (k.pawn as usize & CORRHIST_MASK) }
    #[inline]
    fn npw_idx(k: &CorrKeys) -> usize { k.stm * CORRHIST_SIZE + (k.nonpawn_w as usize & CORRHIST_MASK) }
    #[inline]
    fn npb_idx(k: &CorrKeys) -> usize { k.stm * CORRHIST_SIZE + (k.nonpawn_b as usize & CORRHIST_MASK) }
    #[inline]
    fn cont_idx(k: &CorrKeys, c: usize) -> usize { k.stm * CORRHIST_CONT_SIZE + c }

    /// Correction (centipions) à AJOUTER à l'éval statique : moyenne PONDÉRÉE des
    /// tables (calculée en point fixe), bornée à ±CORRHIST_MAX_CP. Moyenner
    /// plusieurs clés réduit le bruit d'une table isolée — c'était la faiblesse
    /// de la v1 (une seule table pion → bruit direct sur l'éval, -3 Elo).
    #[inline]
    pub fn value(&self, k: &CorrKeys) -> i32 {
        let mut sum = self.pawn[Self::pawn_idx(k)]      * CW_PAWN
                    + self.nonpawn_w[Self::npw_idx(k)]  * CW_NONPAWN
                    + self.nonpawn_b[Self::npb_idx(k)]  * CW_NONPAWN;
        let mut wtot = CW_PAWN + 2 * CW_NONPAWN;
        if let Some(c) = k.cont {
            sum  += self.cont[Self::cont_idx(k, c)] * CW_CONT;
            wtot += CW_CONT;
        }
        // sum est en (cp × GRAIN × poids) → on revient en cp.
        let cp = sum / (CORRHIST_GRAIN * wtot);
        cp.clamp(-CORRHIST_MAX_CP, CORRHIST_MAX_CP)
    }

    /// Apprentissage PONDÉRÉ PAR LA PROFONDEUR : chaque table glisse vers `diff`
    /// (= score de recherche − éval corrigée du nœud), d'un pas proportionnel à
    /// la profondeur (une recherche profonde est plus fiable, donc pèse plus).
    #[inline]
    pub fn update(&mut self, k: &CorrKeys, diff: i32, depth: i32) {
        let target = diff.clamp(-CORRHIST_MAX_CP, CORRHIST_MAX_CP) * CORRHIST_GRAIN;
        let weight = (depth + 1).clamp(1, CORRHIST_WEIGHT_CAP);
        Self::blend(&mut self.pawn[Self::pawn_idx(k)], target, weight);
        Self::blend(&mut self.nonpawn_w[Self::npw_idx(k)], target, weight);
        Self::blend(&mut self.nonpawn_b[Self::npb_idx(k)], target, weight);
        if let Some(c) = k.cont {
            let idx = Self::cont_idx(k, c);
            Self::blend(&mut self.cont[idx], target, weight);
        }
    }

    /// Moyenne mobile en point fixe : `entry += (target − entry) × weight / SCALE`,
    /// puis bornage à ±CORRHIST_LIMIT. (entry et target en cp × GRAIN.)
    #[inline]
    fn blend(entry: &mut i32, target: i32, weight: i32) {
        *entry += (target - *entry) * weight / CORRHIST_WEIGHT_SCALE;
        *entry = (*entry).clamp(-CORRHIST_LIMIT, CORRHIST_LIMIT);
    }
}

// =============================================================================
// Structures de données de la recherche
// =============================================================================

/// Interrupteurs RUNTIME des heuristiques de recherche, regroupés ici pour ne
/// pas éparpiller des champs « réservés aux tests » dans SearchInfo.
///
/// TOUS à false en jeu normal → aucun effet, aucun surcoût (branches toujours
/// non prises, parfaitement prédites). Seul le binaire `selfplay` les bascule
/// pour ISOLER une feature dans un match SPRT (clés `*_a` / `*_b` du fichier de
/// config). Mettre un champ à true DÉSACTIVE la feature correspondante :
///   - disable_improving  : drapeau `improving` (RFP/NMP/LMP/LMR)
///   - disable_futility   : Futility Pruning par coup
///   - disable_lmr_tweaks : ajustements ±1 de la LMR enrichie (base conservée)
///   - disable_correction : Correction History (éval brute, rien lu/appris)
///   - disable_king_attack: terme "sécurité du roi par l'attaque" (éval)
#[derive(Default)]
pub struct FeatureToggles {
    pub disable_improving:  bool,
    pub disable_futility:   bool,
    pub disable_lmr_tweaks: bool,
    pub disable_correction: bool,
    pub disable_king_attack: bool,
}

/// Informations partagées pendant une recherche.
/// Le signal d'arrêt est un Arc<AtomicBool> partagé entre tous les threads.
pub struct SearchInfo {
    /// Heure de début de la recherche.
    pub start_time: Instant,
    /// Limite de temps allouée pour cette recherche.
    pub time_limit: Duration,
    /// Nombre de nœuds explorés (par ce thread).
    pub nodes: u64,
    /// Meilleur coup trouvé jusqu'ici (par ce thread).
    pub best_move: Move,
    /// Score associé au meilleur coup.
    pub best_score: i32,
    /// Profondeur atteinte.
    pub depth_reached: i32,
    /// Profondeur sélective maximale atteinte (quiescence incluse).
    /// Réinitialisée à chaque nouvelle profondeur dans l'itération.
    /// Utilisée pour le champ "seldepth" UCI.
    pub seldepth: i32,
    /// Limite de nœuds pour cette recherche (commande UCI "go nodes <x>").
    /// None = pas de limite (comportement par défaut, piloté par le temps).
    /// Vérifiée uniquement sur le thread principal — sous Lazy SMP, le total
    /// de nœuds réellement explorés (tous threads confondus) peut légèrement
    /// dépasser cette limite, comme pour les limites de temps existantes.
    pub max_nodes: Option<u64>,
    /// Signal d'arrêt partagé entre tous les threads Lazy SMP.
    /// Quand le thread principal épuise son temps, il met ce flag à true,
    /// et tous les threads secondaires s'arrêtent à leur prochain check.
    pub stop: Arc<AtomicBool>,
    /// Active l'émission de "info currmove/currmovenumber" (racine uniquement).
    ///
    /// false par défaut — IMPORTANT : alpha_beta() est appelée directement par
    /// plusieurs outils en dehors du vrai moteur UCI (notamment src/bin/
    /// benchmark.rs, qui construit son propre SearchInfo pour mesurer le NPS
    /// brut sans la couche UCI). Si cette ligne s'imprimait sans condition,
    /// elle polluerait la sortie de ces outils — c'est exactement ce qui s'est
    /// produit avant ce correctif (benchmark noyé sous des lignes currmove).
    /// Seul SearchEngine::search() (la vraie recherche pilotée par l'UCI)
    /// active ce drapeau explicitement sur l'instance du thread principal.
    pub show_currmove: bool,
    /// Évaluation statique par ply, pour le drapeau "improving" (RFP/LMP/NMP)
    /// — voir alphabeta.rs. Indexé directement par ply (0..MAX_PLY).
    /// EVAL_HISTORY_NONE = aucune valeur enregistrée à ce ply pour CETTE
    /// branche (voir static_eval_opt dans alphabeta.rs pour les conditions).
    pub eval_history: [i32; MAX_PLY],
    /// Facteur de contempt (option UCI "Contempt", centipions). 0 par défaut
    /// = comportement inchangé (SCORE_DRAW exact pour toute position nulle).
    /// Une valeur positive pénalise légèrement les nullités du point de vue
    /// du camp à la racine de la recherche — voir alphabeta.rs::draw_score().
    ///
    /// IMPORTANT : doit être IDENTIQUE sur tous les threads Lazy SMP d'une
    /// même recherche (la TT partagée stocke des scores qui doivent rester
    /// cohérents quel que soit le thread qui les a calculés) — voir
    /// SearchEngine::search() qui le copie sur le thread principal ET sur
    /// chaque thread secondaire depuis la même SearchConfig.
    pub contempt: i32,
    /// Correction History (par thread) : plusieurs tables combinées (pion,
    /// non-pion par couleur, continuation). Voir la struct CorrectionHistory et
    /// son usage dans alphabeta.rs (corr_keys + value/update). ⚠️ à valider SPRT.
    pub correction_history: CorrectionHistory,
    /// Interrupteurs runtime des heuristiques (tests SPRT du binaire selfplay).
    /// Tous à false en jeu normal. Voir la struct FeatureToggles.
    pub toggles: FeatureToggles,
}

impl SearchInfo {
    /// Crée une nouvelle instance avec son propre signal d'arrêt.
    pub fn new(time_limit: Duration) -> SearchInfo {
        SearchInfo {
            start_time:    Instant::now(),
            time_limit,
            nodes:         0,
            best_move:     Move::NULL,
            best_score:    0,
            depth_reached: 0,
            seldepth:      0,
            max_nodes:     None,
            stop:          Arc::new(AtomicBool::new(false)),
            show_currmove: false,
            eval_history:  [EVAL_HISTORY_NONE; MAX_PLY],
            contempt:      0,
            correction_history: CorrectionHistory::new(),
            toggles: FeatureToggles::default(),
        }
    }

    /// Crée une instance partagée avec un signal d'arrêt externe.
    /// Utilisée par les threads secondaires Lazy SMP.
    pub fn new_with_stop(time_limit: Duration, stop: Arc<AtomicBool>) -> SearchInfo {
        SearchInfo {
            start_time:    Instant::now(),
            time_limit,
            nodes:         0,
            best_move:     Move::NULL,
            best_score:    0,
            depth_reached: 0,
            seldepth:      0,
            max_nodes:     None,
            stop,
            show_currmove: false,
            eval_history:  [EVAL_HISTORY_NONE; MAX_PLY],
            contempt:      0,
            correction_history: CorrectionHistory::new(),
            toggles: FeatureToggles::default(),
        }
    }

    /// Retourne true si la recherche doit s'arrêter.
    #[inline]
    pub fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Vérifie si le temps OU la limite de nœuds (si définie) sont atteints.
    /// À appeler périodiquement. Vérifie toutes les 4096 nœuds pour ne pas
    /// appeler Instant::now() à chaque nœud (coûteux).
    pub fn check_time(&mut self) {
        if self.nodes & 0xFFF == 0 {
            if self.start_time.elapsed() >= self.time_limit {
                self.stop.store(true, Ordering::Relaxed);
            }
            if let Some(max) = self.max_nodes {
                if self.nodes >= max {
                    self.stop.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    /// Met à jour le meilleur coup depuis la racine (ply == 0 uniquement).
    pub fn update_best_move(&mut self, mv: Move, score: i32, ply: usize) {
        if ply == 0 {
            self.best_move  = mv;
            self.best_score = score;
        }
    }

    /// Retourne le temps écoulé en millisecondes.
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }
}

/// Configuration d'une recherche.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Temps disponible pour les Blancs (en millisecondes).
    pub wtime: Option<u64>,
    /// Temps disponible pour les Noirs (en millisecondes).
    pub btime: Option<u64>,
    /// Incrément par coup pour les Blancs (en millisecondes).
    pub winc: Option<u64>,
    /// Incrément par coup pour les Noirs (en millisecondes).
    pub binc: Option<u64>,
    /// Nombre de coups avant le contrôle de temps.
    pub movestogo: Option<u32>,
    /// Profondeur maximale de recherche.
    pub depth: Option<i32>,
    /// Temps de réflexion fixe (en millisecondes).
    pub movetime: Option<u64>,
    /// Recherche infinie (jusqu'à stop).
    pub infinite: bool,
    /// Niveau de difficulté (1-64). 64 = pleine puissance.
    pub skill_level: u8,
    /// Mode ponder : le moteur réfléchit sur le temps de l'adversaire.
    /// La recherche tourne en mode infini jusqu'à ponderhit ou stop.
    pub ponder: bool,
    /// Liste de coups à analyser en notation UCI (ex : ["e2e4", "d2d4"]).
    /// Vide = tous les coups légaux (comportement par défaut).
    /// Correspond au paramètre "searchmoves" de la commande "go".
    pub searchmoves: Vec<String>,
    /// Limite de nœuds pour cette recherche ("go nodes <x>").
    /// None = pas de limite (comportement par défaut).
    pub nodes: Option<u64>,
    /// Recherche d'un mat forcé en <x> coups ("go mate <x>").
    /// Traduit en profondeur = 2×x plies (un mat en N coups complets se
    /// trouve en au plus 2N-1 demi-coups ; 2N est une borne légèrement
    /// large mais sûre). None = pas de recherche de mat spécifique.
    pub mate: Option<u32>,
    /// Nombre de variantes principales à afficher ("option MultiPV").
    /// 1 = comportement standard (une seule meilleure ligne).
    pub multipv: usize,
    /// Marge de sécurité (en ms) retirée du budget de temps calculé, pour
    /// compenser la latence de communication GUI/réseau ("option Move
    /// Overhead"). Remplace l'ancienne marge fixe de 50 ms codée en dur dans
    /// compute_time_limit() — même valeur par défaut, mais désormais réglable
    /// (important en ligne/tournoi : sans marge suffisante, le moteur peut
    /// perdre au temps simplement à cause du délai de relais des commandes).
    pub move_overhead: u64,
    /// Facteur de contempt (option UCI "Contempt", centipions). 0 par défaut
    /// = comportement inchangé. Copié dans SearchInfo::contempt — voir
    /// alphabeta.rs::draw_score() pour le détail de l'ajustement.
    pub contempt: i32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            wtime:       None,
            btime:       None,
            winc:        None,
            binc:        None,
            movestogo:   None,
            depth:       None,
            movetime:    None,
            infinite:    false,
            skill_level: 64,
            ponder:      false,
            searchmoves: vec![],
            nodes:       None,
            mate:        None,
            multipv:     1,
            move_overhead: 50, // identique à l'ancienne marge fixe — zéro régression par défaut
            contempt:    0,
        }
    }
}

/// Résultat d'une recherche.
pub struct SearchResult {
    /// Meilleur coup trouvé.
    pub best_move: Move,
    /// Coup de réponse prédit de l'adversaire (pour le pondering).
    /// Obtenu par sonde de la table de transposition après best_move.
    /// Move::NULL si aucune prédiction disponible (mat, position inconnue, etc.).
    pub ponder_move: Move,
    /// Score en centipions.
    pub score: i32,
    /// Profondeur atteinte.
    pub depth: i32,
    /// Nombre de nœuds explorés (thread principal uniquement).
    pub nodes: u64,
    /// Temps de recherche en millisecondes.
    pub time_ms: u64,
}

// =============================================================================
// Moteur de recherche principal
// =============================================================================

/// Moteur de recherche. Contient la table de transposition partagée et les heuristiques.
pub struct SearchEngine {
    /// Table de transposition partagée entre tous les threads via Arc.
    /// AtomicU64 interne → pas besoin de Mutex, lock-free.
    pub tt:          Arc<TranspositionTable>,
    /// Killer moves (thread principal uniquement, non partagés).
    pub killers:     KillerMoves,
    /// History heuristic (thread principal uniquement, non partagée).
    pub history:     HistoryTable,
    /// Countermove heuristic (thread principal uniquement, non partagée).
    pub countermoves: CountermoveTable,
    /// Continuation history — généralisation cumulative du countermove
    /// (thread principal uniquement, non partagée). ~576 Kio, allouée sur
    /// le tas (voir continuation_history.rs pour le choix de stockage).
    pub cont_history: ContinuationHistoryTable,
    /// Nombre de threads de recherche (1 = mono-thread, >1 = Lazy SMP).
    pub num_threads: usize,
    /// Signal d'arrêt partagé avec le thread UCI.
    /// Mis à true par stop() pour interrompre immédiatement la recherche en cours.
    pub stop_flag:   Arc<AtomicBool>,
}

impl SearchEngine {
    /// Crée un nouveau moteur de recherche.
    pub fn new() -> SearchEngine {
        let default_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        SearchEngine {
            tt:           Arc::new(TranspositionTable::new(32)),
            killers:      KillerMoves::new(),
            history:      HistoryTable::new(),
            countermoves: CountermoveTable::new(),
            cont_history: ContinuationHistoryTable::new(),
            num_threads:  default_threads,
            stop_flag:    Arc::new(AtomicBool::new(false)),
        }
    }

    /// Réinitialise les heuristiques entre deux parties.
    pub fn new_game(&mut self) {
        self.tt.clear();
        self.killers.clear();
        self.history.clear();
        self.countermoves.clear();
        self.cont_history.clear();
    }

    /// Interrompt immédiatement la recherche en cours (thread-safe).
    /// La recherche s'arrêtera au prochain cycle de vérification interne (~4 096 nœuds).
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }

    /// Lance une recherche sur la position donnée avec la configuration spécifiée.
    /// Retourne le meilleur coup trouvé.
    ///
    /// En mode multi-thread, les threads secondaires (Lazy SMP) peuplent la TT
    /// partagée en parallèle du thread principal.
    pub fn search(&mut self, board: &mut Board, config: &SearchConfig) -> SearchResult {
        // Incrémenter la génération TT : les entrées de la recherche précédente
        // sont désormais périmées et remplaçables même à profondeur inférieure.
        self.tt.new_search();

        // Calculer le temps alloué pour ce coup
        let time_limit = compute_time_limit(board, config);

        // Profondeur maximale : "mate <x>" prend la priorité sur "depth <x>"
        // s'il est spécifié (recherche d'un mat forcé en x coups → 2x plies,
        // borne sûre — voir SearchConfig::mate).
        let max_depth = if let Some(mate_in) = config.mate {
            (mate_in as i32).saturating_mul(2).max(1)
        } else {
            config.depth.unwrap_or(if config.infinite { 128 } else { 64 })
        };

        // S'assurer qu'il y a au moins un coup légal
        let legal_moves = generate_legal_moves(board);
        if legal_moves.is_empty() {
            return SearchResult {
                best_move:   Move::NULL,
                ponder_move: Move::NULL,
                score:       0,
                depth:       0,
                nodes:       0,
                time_ms:     0,
            };
        }

        // Pré-filtrage searchmoves — une seule fois, avant l'itération en profondeur.
        // Avantages vs l'ancien filtre dans alpha_beta :
        //   - to_uci() (allocation String) appelé O(|legal_moves|) au lieu de
        //     O(|legal_moves| × depth × aspiration_retries).
        //   - SearchInfo n'a plus de Vec<String> heap-alloué propagé à chaque nœud.
        //   - alpha_beta reçoit &[Move] (slice, zéro copie) uniquement à ply==0.
        let root_moves: Vec<Move> = if config.searchmoves.is_empty() {
            // Vec vide → alpha_beta génère les coups normalement.
            vec![]
        } else {
            let filtered: Vec<Move> = legal_moves.iter()
                .copied()
                .filter(|mv| config.searchmoves.iter().any(|s| *s == mv.to_uci()))
                .collect();
            // Défense : liste invalide → on laisse alpha_beta générer tout.
            if filtered.is_empty() { vec![] } else { filtered }
        };

        // Coup par défaut = premier coup légal (sécurité si le temps est épuisé
        // avant de compléter la première profondeur)
        let mut best_move  = legal_moves[0];
        let mut best_score = 0i32;

        // Profondeur maximale selon le niveau de difficulté
        let effective_max_depth = skill_level_max_depth(config.skill_level, max_depth);

        // Vieillissement de l'historique entre les recherches
        self.history.age();
        self.killers.clear();
        // Countermove : comme les killers, remis à zéro à chaque "go" — un
        // countermove pertinent dans une recherche n'a aucune raison de
        // l'être dans la position suivante (l'adversaire a réellement joué).
        self.countermoves.clear();
        // Continuation history : vieillie comme l'history (pas un clear()
        // complet) — un score CUMULATIF garde un intérêt à se transmettre,
        // atténué, d'une recherche à l'autre, contrairement au countermove
        // qui n'est qu'un seul slot par contexte.
        self.cont_history.age();

        // --- Signal d'arrêt partagé entre tous les threads ---
        // Réinitialiser pour ce cycle de recherche, puis partager via Arc.
        // Le thread UCI peut aussi positionner self.stop_flag via stop() à tout moment.
        self.stop_flag.store(false, Ordering::SeqCst);
        let stop_flag = Arc::clone(&self.stop_flag);

        // --- Lancement des threads secondaires (Lazy SMP) ---
        let mut handles = vec![];
        let num_threads = self.num_threads;

        if num_threads > 1 {
            for t in 1..num_threads {
                // Chaque thread secondaire a sa propre copie du plateau
                let mut board_copy = board.clone();
                // Tous partagent la même TT via Arc
                let tt_shared = Arc::clone(&self.tt);
                // Tous partagent le même signal d'arrêt
                let stop_shared = Arc::clone(&stop_flag);
                // Variation de profondeur pour diversifier les recherches
                let depth_variation = (t % 3) as i32;
                // root_moves partagés avec le thread principal (Clone O(searchmoves))
                let root_moves_smp = root_moves.clone();
                // Contempt copié par valeur (i32 : Copy) — voir la note sur
                // SearchInfo::contempt : DOIT être identique sur tous les
                // threads, la TT partagée stockerait des scores incohérents sinon.
                let contempt_smp = config.contempt;

                // Pile de 8 Mio (au lieu du défaut ~2 Mio des threads Rust) : la
                // recherche est fortement récursive (jusqu'à ~MAX_PLY plies +
                // quiescence) et chaque trame porte désormais des listes de coups
                // allouées sur la PILE (MoveList + tableau de captures scorées).
                // 8 Mio donne une marge large contre tout débordement de pile sur
                // ces threads secondaires (le thread principal a déjà ~8 Mio par
                // défaut sur macOS). En cas d'échec de création (ressources OS
                // épuisées), on continue simplement avec moins de threads — la
                // recherche reste correcte, juste un peu moins parallèle.
                let builder = std::thread::Builder::new().stack_size(8 * 1024 * 1024);
                let spawn_result = builder.spawn(move || {
                    // Heuristiques locales au thread (non partagées)
                    let mut killers      = KillerMoves::new();
                    let mut history      = HistoryTable::new();
                    let mut countermoves = CountermoveTable::new();
                    let mut cont_history = ContinuationHistoryTable::new();
                    // SearchInfo avec signal d'arrêt partagé et temps illimité
                    // (le thread s'arrête uniquement via le stop_flag)
                    let mut info = SearchInfo::new_with_stop(
                        Duration::from_secs(3600),
                        stop_shared.clone(),
                    );
                    info.contempt = contempt_smp;

                    // Recherche avec légère variation de profondeur
                    let depth_max = effective_max_depth.saturating_add(depth_variation);
                    for depth in 1..=depth_max {
                        if stop_shared.load(Ordering::Relaxed) { break; }
                        alpha_beta(
                            &mut board_copy,
                            depth,
                            -SCORE_MATE,
                            SCORE_MATE,
                            0,
                            &tt_shared,
                            &mut killers,
                            &mut history,
                            &mut countermoves,
                            &mut cont_history,
                            Move::NULL, // prev_move : aucun coup précédent à la racine
                            &mut info,
                            Move::NULL,
                            &root_moves_smp,
                        );
                    }
                });
                match spawn_result {
                    Ok(handle) => handles.push(handle),
                    Err(_)     => { /* thread non créé : on continue avec moins de threads */ }
                }
            }
        }

        // --- Recherche principale (thread courant) avec Aspiration Windows ---
        let mut info = SearchInfo::new_with_stop(time_limit, Arc::clone(&stop_flag));
        info.max_nodes = config.nodes; // "go nodes <x>" — None si non spécifié
        // Seul ce thread (la vraie recherche pilotée par l'UCI) émet
        // "info currmove" — voir le commentaire sur SearchInfo::show_currmove.
        info.show_currmove = true;
        info.contempt = config.contempt;

        for depth in 1..=effective_max_depth {
            // Réinitialiser le meilleur coup et seldepth avant chaque nouvelle profondeur.
            // Garantit qu'un résultat de profondeur N ne pollue pas la profondeur N+1.
            info.best_move = Move::NULL;
            info.seldepth  = depth; // la recherche régulière atteint au moins `depth` plies

            // --- Aspiration Windows ---
            // À partir de la profondeur 4, on cherche d'abord dans une fenêtre
            // étroite autour du score précédent. Si ça échoue (fail-low/fail-high),
            // on élargit progressivement. En pratique, la fenêtre étroite suffit
            // la plupart du temps et accélère considérablement la recherche.
            let score;

            if depth >= 4 && best_score.abs() < SCORE_MATE - 200 {
                // Fenêtre initiale de ±50 centipions autour du score précédent
                let mut asp_delta = 50i32;
                let mut asp_alpha = (best_score - asp_delta).max(-SCORE_MATE);
                let mut asp_beta  = (best_score + asp_delta).min(SCORE_MATE);
                // Initialisation après la première itération de la boucle (jamais lue avant)
                let mut asp_score;

                'aspiration: loop {
                    // Réinitialiser avant chaque tentative dans la fenêtre
                    info.best_move = Move::NULL;

                    asp_score = alpha_beta(
                        board, depth, asp_alpha, asp_beta, 0,
                        &self.tt, &mut self.killers, &mut self.history,
                        &mut self.countermoves, &mut self.cont_history,
                        Move::NULL, &mut info,
                        Move::NULL,
                        &root_moves,
                    );

                    if info.should_stop() { break 'aspiration; }

                    if asp_score <= asp_alpha {
                        // Fail-low : le vrai score est EN DESSOUS de notre fenêtre.
                        // → score est au MAXIMUM asp_score (upperbound UCI).
                        let el  = info.elapsed_ms();
                        let nps = compute_nps(info.nodes, el);
                        println!(
                            "info depth {} seldepth {} score {} upperbound nodes {} nps {} time {} hashfull {}",
                            depth, info.seldepth, format_score(asp_score),
                            info.nodes, nps, el, self.tt.hashfull(),
                        );
                        asp_alpha  = (asp_alpha - asp_delta).max(-SCORE_MATE);
                        asp_delta  = asp_delta.saturating_mul(2);
                    } else if asp_score >= asp_beta {
                        // Fail-high : le vrai score est AU-DESSUS de notre fenêtre.
                        // → score est au MINIMUM asp_score (lowerbound UCI).
                        let el  = info.elapsed_ms();
                        let nps = compute_nps(info.nodes, el);
                        println!(
                            "info depth {} seldepth {} score {} lowerbound nodes {} nps {} time {} hashfull {}",
                            depth, info.seldepth, format_score(asp_score),
                            info.nodes, nps, el, self.tt.hashfull(),
                        );
                        asp_beta   = (asp_beta + asp_delta).min(SCORE_MATE);
                        asp_delta  = asp_delta.saturating_mul(2);
                    } else {
                        // Score dans la fenêtre : résultat exact, on s'arrête
                        break 'aspiration;
                    }

                    // Sécurité : si la fenêtre est maximale, ne plus retenter
                    if asp_alpha <= -SCORE_MATE && asp_beta >= SCORE_MATE {
                        info.best_move = Move::NULL;
                        asp_score = alpha_beta(
                            board, depth, -SCORE_MATE, SCORE_MATE, 0,
                            &self.tt, &mut self.killers, &mut self.history,
                            &mut self.countermoves, &mut self.cont_history,
                            Move::NULL, &mut info,
                            Move::NULL,
                            &root_moves,
                        );
                        break 'aspiration;
                    }
                }

                score = asp_score;
            } else {
                // Profondeurs 1-3 : fenêtre complète (pas d'aspiration)
                score = alpha_beta(
                    board, depth, -SCORE_MATE, SCORE_MATE, 0,
                    &self.tt, &mut self.killers, &mut self.history,
                    &mut self.countermoves, &mut self.cont_history,
                    Move::NULL, &mut info,
                    Move::NULL,
                    &root_moves,
                );
            }

            // Si la recherche a été interrompue, utiliser le résultat précédent
            if info.should_stop() && depth > 1 {
                break;
            }

            // Mettre à jour le meilleur coup si la profondeur est complète
            if !info.best_move.is_null() {
                best_move  = info.best_move;
                best_score = score;
                info.depth_reached = depth;
            }

            // Afficher les informations de progression (protocole UCI)
            let elapsed = info.elapsed_ms();
            let nps     = compute_nps(info.nodes, elapsed);
            println!(
                "info depth {} seldepth {} score {} nodes {} nps {} time {} hashfull {} pv {}",
                depth,
                info.seldepth,
                format_score(score),
                info.nodes,
                nps,
                elapsed,
                self.tt.hashfull(),
                best_move.to_uci(),
            );

            // Arrêt si le temps est épuisé après une profondeur complète
            if info.start_time.elapsed() >= time_limit && depth > 1 {
                break;
            }

            // Arrêt si on a trouvé un mat
            if score.abs() > SCORE_MATE - 200 {
                break;
            }
        }

        // --- Signaler l'arrêt aux threads secondaires ---
        stop_flag.store(true, Ordering::Relaxed);

        // --- Attendre la fin de tous les threads secondaires ---
        for h in handles {
            let _ = h.join();
        }

        // Introduire une erreur aléatoire pour les niveaux de difficulté faibles
        let final_move = apply_skill_level(board, best_move, config.skill_level);

        // --- Ponder move : coup de réponse prédit de l'adversaire ---
        // On joue final_move, on sonde la TT pour la position résultante,
        // on annule le coup. Le meilleur coup stocké dans la TT pour cette
        // position est le coup attendu de l'adversaire.
        let ponder_move = if !final_move.is_null() {
            board.make_move(final_move);
            let pm = self.tt.probe(board.hash)
                .map(|entry| entry.best_move)
                .unwrap_or(Move::NULL);
            board.unmake_move(final_move);
            pm
        } else {
            Move::NULL
        };

        SearchResult {
            best_move:   final_move,
            ponder_move,
            score:       best_score,
            depth:       info.depth_reached,
            nodes:       info.nodes,
            time_ms:     info.elapsed_ms(),
        }
    }

    /// Lance une recherche MultiPV : trouve les `config.multipv` meilleures
    /// variantes (classées de la meilleure à la moins bonne), au lieu d'une
    /// seule. Retourne un vecteur ordonné — index 0 = meilleure ligne.
    ///
    /// Principe (volontairement simple, réutilise search() sans le modifier) :
    ///   Pour trouver la N-ième meilleure ligne, on relance une recherche
    ///   complète (avec son propre iterative deepening, sa propre gestion du
    ///   temps, profitant du Lazy SMP comme d'habitude) en EXCLUANT les coups
    ///   déjà retenus pour les lignes précédentes — exactement le mécanisme
    ///   `searchmoves` déjà utilisé pour filtrer la racine (option UCI
    ///   "go searchmoves"). Aucune modification de search() ni de alpha_beta()
    ///   n'est nécessaire : MultiPV n'est qu'une orchestration par-dessus.
    ///
    /// Compromis assumé (documenté plutôt que caché) :
    ///   - Chaque ligne recherchée prend son propre temps complet (le budget
    ///     temps n'est pas divisé entre les lignes) — une recherche MultiPV=3
    ///     prend donc environ 3× plus de temps qu'une recherche normale.
    ///     C'est le comportement standard attendu pour MultiPV.
    ///   - La table de transposition est PARTAGÉE entre les appels successifs
    ///     (self.tt), donc les lignes 2, 3... bénéficient partiellement du
    ///     travail déjà fait pour la ligne 1 — pas un calcul totalement perdu.
    ///   - Si MultiPV <= 1 : strictement équivalent à appeler search()
    ///     directement (aucun changement de comportement par défaut).
    pub fn search_multipv(&mut self, board: &mut Board, config: &SearchConfig) -> Vec<SearchResult> {
        if config.multipv <= 1 {
            return vec![self.search(board, config)];
        }

        let legal_moves = generate_legal_moves(board);
        if legal_moves.is_empty() {
            return vec![SearchResult {
                best_move: Move::NULL, ponder_move: Move::NULL,
                score: 0, depth: 0, nodes: 0, time_ms: 0,
            }];
        }

        // Respecter un éventuel "searchmoves" déjà fourni par la GUI : il
        // restreint l'ensemble de départ avant même de répartir les lignes.
        let mut remaining: Vec<Move> = if config.searchmoves.is_empty() {
            legal_moves
        } else {
            let filtered: Vec<Move> = generate_legal_moves(board).into_iter()
                .filter(|mv| config.searchmoves.iter().any(|s| *s == mv.to_uci()))
                .collect();
            if filtered.is_empty() { generate_legal_moves(board) } else { filtered }
        };

        let slots = config.multipv.min(remaining.len()).max(1);
        let mut results = Vec::with_capacity(slots);

        for _ in 0..slots {
            let mut slot_config = config.clone();
            // Restreint cette recherche aux coups RESTANTS (pas encore classés).
            slot_config.searchmoves = remaining.iter().map(|mv| mv.to_uci()).collect();

            let result = self.search(board, &slot_config);
            if result.best_move.is_null() {
                // Plus aucun coup légal à classer (ne devrait arriver qu'en
                // tout début, déjà géré ci-dessus — sécurité supplémentaire).
                break;
            }

            remaining.retain(|mv| *mv != result.best_move);
            results.push(result);

            if remaining.is_empty() { break; }
        }

        results
    }
}

// =============================================================================
// Gestion du temps
// =============================================================================

/// Calcule le temps alloué pour ce coup selon la configuration.
///
/// `config.move_overhead` (option UCI "Move Overhead", 50 ms par défaut)
/// remplace ce qui était auparavant une marge fixe codée en dur. Elle est
/// retirée du temps calculé pour compenser la latence de communication
/// GUI/réseau — sans cette marge, un délai de relais des commandes peut
/// faire perdre la partie au temps, en particulier en ligne (Lichess,
/// cutechess-cli) où la latence n'est pas négligeable.
fn compute_time_limit(board: &Board, config: &SearchConfig) -> Duration {
    // Temps fixe
    if let Some(movetime) = config.movetime {
        return Duration::from_millis(movetime.saturating_sub(config.move_overhead));
    }

    // Recherche infinie
    if config.infinite {
        return Duration::from_secs(3600);
    }

    // Temps de partie : calculer selon le temps restant
    let (time_remaining, increment) = match board.side_to_move {
        crate::utils::types::Color::White => (
            config.wtime.unwrap_or(30_000),
            config.winc.unwrap_or(0),
        ),
        crate::utils::types::Color::Black => (
            config.btime.unwrap_or(30_000),
            config.binc.unwrap_or(0),
        ),
    };

    // Estimation du nombre de coups restants.
    // .max(1) : défense contre movestogo = Some(0) (invalide selon la spec UCI mais
    // possible via l'API publique SearchConfig) — évite la division par zéro.
    let moves_to_go = config.movestogo.unwrap_or(30).max(1) as u64;

    // Allouer une fraction du temps restant + l'incrément
    let time_for_move = time_remaining / moves_to_go + increment / 2;

    // Ne jamais utiliser plus de la moitié du temps restant
    let max_time  = time_remaining / 2;
    let allocated = time_for_move.min(max_time).max(100);

    Duration::from_millis(allocated.saturating_sub(config.move_overhead))
}

// =============================================================================
// UCI_LimitStrength / UCI_Elo — conversion vers le système skill_level
// =============================================================================

/// Elo minimum et maximum couverts par la conversion UCI_Elo → skill_level.
/// ELO_MAX = ~2600, cohérent avec le niveau de jeu mesuré de Vendetta Chess Motor
/// (victoires confirmées contre Stockfish à 2500 Elo limité après le Texel
/// Tuning v3 — voir CLAUDE.md / README.md). ELO_MIN = 600, borne basse
/// raisonnable pour un "débutant absolu" (niveau skill_level = 1).
pub const ELO_MIN: u16 = 600;
pub const ELO_MAX: u16 = 2600;

/// Convertit une valeur UCI_Elo en niveau skill_level (1-64).
///
/// Interpolation LINÉAIRE simple entre (ELO_MIN → niveau 1) et
/// (ELO_MAX → niveau 64). Ce n'est pas une calibration Elo précise (qui
/// demanderait des centaines de parties par palier pour être rigoureuse) —
/// c'est une correspondance raisonnable permettant aux GUIs/plateformes
/// utilisant le mécanisme standard UCI_LimitStrength + UCI_Elo de brider
/// Vendetta Chess Motor, plutôt que de devoir connaître l'option maison
/// "Skill Level". Hors de la plage, la valeur est bornée (clamp).
pub fn elo_to_skill_level(elo: u16) -> u8 {
    let elo = elo.clamp(ELO_MIN, ELO_MAX);
    let fraction = (elo - ELO_MIN) as f32 / (ELO_MAX - ELO_MIN) as f32;
    let skill = 1.0 + fraction * 63.0;
    skill.round().clamp(1.0, 64.0) as u8
}

// =============================================================================
// Gestion des niveaux de difficulté
// =============================================================================

/// Profondeur maximale selon le niveau de difficulté (1-64).
///
/// Graduation continue sur 64 niveaux :
///   - Niveau  1 : profondeur 1  (débutant absolu)
///   - Niveau 16 : profondeur 4  (amateur)
///   - Niveau 32 : profondeur 7  (intermédiaire)
///   - Niveau 48 : profondeur 11 (avancé)
///   - Niveau 64 : pleine puissance (sans limite de profondeur)
///
/// Formule : profondeur = 1 + (skill - 1) * (max_depth - 1) / 63
/// interpolée de façon quadratique pour une graduation naturelle.
fn skill_level_max_depth(skill: u8, requested_depth: i32) -> i32 {
    // Niveau 64 = pleine puissance, aucune limite
    if skill >= 64 {
        return requested_depth;
    }

    // Interpolation quadratique entre profondeur 1 (niveau 1) et
    // profondeur 16 (niveau 63). Quadratique pour que les premiers
    // niveaux progressent doucement et les derniers plus vite.
    let s = (skill as f32 - 1.0) / 62.0; // [0.0, 1.0]
    let max_depth_for_level = (1.0 + s * s * 15.0).round() as i32;

    max_depth_for_level.min(requested_depth)
}

/// Introduit une erreur aléatoire pour simuler un joueur humain (niveaux 1-64).
///
/// Graduation continue :
///   - Niveau  1 : 90% de chance de jouer aléatoirement
///   - Niveau 16 : 40% de chance
///   - Niveau 32 : 10% de chance
///   - Niveau 48 :  2% de chance
///   - Niveau 57+ : aucune erreur (pleine puissance)
fn apply_skill_level(board: &mut Board, best_move: Move, skill: u8) -> Move {
    // Au-delà du niveau 56, toujours le meilleur coup
    if skill >= 57 {
        return best_move;
    }

    // Probabilité d'erreur : décroissance quadratique de 90% (niveau 1) à 1% (niveau 56)
    let s = (skill as f32 - 1.0) / 55.0; // [0.0, 1.0]
    let random_chance = ((1.0 - s * s) * 90.0).round() as u64;

    let pseudo_random = (board.hash
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
        >> 33) % 100;

    if pseudo_random < random_chance {
        let moves = generate_legal_moves(board);
        if !moves.is_empty() {
            return moves[(pseudo_random as usize) % moves.len()];
        }
    }

    best_move
}

// =============================================================================
// Formatage UCI
// =============================================================================

/// Formate un score pour l'affichage UCI.
pub fn format_score(score: i32) -> String {
    if score.abs() > SCORE_MATE - 200 {
        let mate_in = (SCORE_MATE - score.abs() + 1) / 2;
        if score > 0 {
            format!("mate {}", mate_in)
        } else {
            format!("mate -{}", mate_in)
        }
    } else {
        format!("cp {}", score)
    }
}

/// Nœuds par seconde (NPS) pour l'affichage UCI `info`. Retourne 0 si la durée
/// écoulée est nulle (cas du tout début d'une recherche) — évite la division
/// par zéro. `saturating_mul` borne le cas (théorique) d'un dépassement u64.
#[inline]
pub fn compute_nps(nodes: u64, elapsed_ms: u64) -> u64 {
    nodes.saturating_mul(1000).checked_div(elapsed_ms).unwrap_or(0)
}

impl Default for SearchEngine {
    fn default() -> Self {
        SearchEngine::new()
    }
}
