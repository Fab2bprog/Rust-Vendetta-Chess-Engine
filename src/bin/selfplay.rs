// =============================================================================
// Vendetta Chess Motor — src/bin/selfplay.rs
//
// Rôle : Mesurer si une modification du moteur AJOUTE de l'Elo, par un test
//        SPRT en self-play interne (le moteur joue contre lui-même, deux
//        variantes A et B, sans UCI ni sous-processus).
//
// Workflow (mode d'emploi complet : COMMENT_TESTER_SPRT.md) :
//   1. Un fichier de config `clé = valeur` décrit le test (préparé par toi/Claude).
//   2. Ce binaire joue A contre B en parties rapides (nœuds fixes par coup),
//      depuis des ouvertures aléatoires (couleurs alternées pour l'équité).
//   3. Le SPRT s'arrête tout seul dès qu'il conclut (PASS / FAIL), ou au plafond.
//   4. Un rapport `clé = valeur` est écrit (et ré-écrit régulièrement = autosave).
//
// Arrêt propre : crée un fichier `STOP` dans le dossier courant
//   (`touch STOP`) → le programme finit la partie en cours, écrit un rapport
//   final marqué "INTERROMPU", supprime STOP, et se termine.
//
// Lancement :
//   cargo run --release --bin selfplay -- <config.txt>
//   (défaut : selfplay_config.txt)
//
// Zéro dépendance externe (parsing maison, PRNG maison, SPRT maison).
// =============================================================================

use std::env;
use std::fs;
use std::path::Path;
use std::sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::thread;
use std::time::Duration;

use vendetta_chess_motor::board::state::Board;
use vendetta_chess_motor::board::bitboard::init_attack_tables;
use vendetta_chess_motor::search::transposition::TranspositionTable;
use vendetta_chess_motor::search::killers::KillerMoves;
use vendetta_chess_motor::search::history::HistoryTable;
use vendetta_chess_motor::search::countermove::CountermoveTable;
use vendetta_chess_motor::search::continuation_history::ContinuationHistoryTable;
use vendetta_chess_motor::search::alphabeta::alpha_beta;
use vendetta_chess_motor::search::SearchInfo;
use vendetta_chess_motor::utils::types::{Color, Move, SCORE_MATE};
use vendetta_chess_motor::moves::generate_legal_moves;
use vendetta_chess_motor::game::Game;
use vendetta_chess_motor::game::rules::GameResult;

// --- Constantes internes -----------------------------------------------------
const SELFPLAY_TT_MB:     usize = 8;   // petite TT par camp (recherche peu profonde)
const SELFPLAY_MAX_DEPTH: i32   = 64;  // borne de l'iterative deepening (le nœud-limit coupe avant)
const OPENING_PLIES:      usize = 8;   // demi-coups aléatoires en ouverture (variété)
const MAX_PLIES:          usize = 400; // garde-fou anti-partie infinie → nulle
const POLL_MS:            u64   = 500; // intervalle de scrutation du thread principal (ms)
const MAX_CONCURRENCY:    usize = 64;  // garde-fou : borne haute du parallélisme (anti-OOM)
const STOP_FILE:          &str  = "STOP";

// =============================================================================
// Configuration (fichier clé = valeur)
// =============================================================================

struct Config {
    nodes_a:     u64,   // nœuds par coup, variante A (référence)
    nodes_b:     u64,   // nœuds par coup, variante B (candidat)
    improving_a: bool,  // feature "improving" activée pour A ?
    improving_b: bool,  // feature "improving" activée pour B ?
    futility_a:  bool,  // Futility Pruning par coup activé pour A ?
    futility_b:  bool,  // Futility Pruning par coup activé pour B ?
    lmr_a:       bool,  // LMR enrichie (ajustements ±1) activée pour A ?
    lmr_b:       bool,  // LMR enrichie (ajustements ±1) activée pour B ?
    correction_a: bool, // Correction History activée pour A ?
    correction_b: bool, // Correction History activée pour B ?
    king_attack_a: bool, // Sécurité du roi par l'attaque activée pour A ?
    king_attack_b: bool, // Sécurité du roi par l'attaque activée pour B ?
    games_max:   u64,   // plafond de parties (arrêt si atteint sans conclusion)
    elo0:        f64,   // borne SPRT basse
    elo1:        f64,   // borne SPRT haute
    alpha:       f64,   // risque de 1re espèce
    beta:        f64,   // risque de 2e espèce
    concurrency: usize, // nb de parties jouées EN PARALLÈLE (0/1 = séquentiel)
    report:      String,// fichier de rapport
}

impl Config {
    fn defaults() -> Config {
        Config {
            nodes_a:     20_000,
            nodes_b:     20_000,
            improving_a: true,
            improving_b: true,
            futility_a:  true,
            futility_b:  true,
            lmr_a:       true,
            lmr_b:       true,
            correction_a: true,
            correction_b: true,
            king_attack_a: true,
            king_attack_b: true,
            games_max:   4_000,
            elo0:        0.0,
            elo1:        5.0,
            alpha:       0.05,
            beta:        0.05,
            concurrency: 4,
            report:      "rapport_selfplay.txt".to_string(),
        }
    }
}

fn parse_bool(v: &str, default: bool) -> bool {
    match v.to_ascii_lowercase().as_str() {
        "true" | "1" | "oui" | "on"  => true,
        "false" | "0" | "non" | "off" => false,
        _ => default,
    }
}

fn parse_config(path: &str) -> Config {
    let mut c = Config::defaults();
    match fs::read_to_string(path) {
        Ok(content) => {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let k = k.trim();
                    let v = v.trim();
                    match k {
                        "nodes_a"     => c.nodes_a     = v.parse().unwrap_or(c.nodes_a),
                        "nodes_b"     => c.nodes_b     = v.parse().unwrap_or(c.nodes_b),
                        "improving_a" => c.improving_a = parse_bool(v, c.improving_a),
                        "improving_b" => c.improving_b = parse_bool(v, c.improving_b),
                        "futility_a"  => c.futility_a  = parse_bool(v, c.futility_a),
                        "futility_b"  => c.futility_b  = parse_bool(v, c.futility_b),
                        "lmr_a"       => c.lmr_a       = parse_bool(v, c.lmr_a),
                        "lmr_b"       => c.lmr_b       = parse_bool(v, c.lmr_b),
                        "correction_a" => c.correction_a = parse_bool(v, c.correction_a),
                        "correction_b" => c.correction_b = parse_bool(v, c.correction_b),
                        "king_attack_a" => c.king_attack_a = parse_bool(v, c.king_attack_a),
                        "king_attack_b" => c.king_attack_b = parse_bool(v, c.king_attack_b),
                        "games_max"   => c.games_max   = v.parse().unwrap_or(c.games_max),
                        "elo0"        => c.elo0        = v.parse().unwrap_or(c.elo0),
                        "elo1"        => c.elo1        = v.parse().unwrap_or(c.elo1),
                        "alpha"       => c.alpha       = v.parse().unwrap_or(c.alpha),
                        "beta"        => c.beta        = v.parse().unwrap_or(c.beta),
                        "concurrency" => c.concurrency = v.parse().unwrap_or(c.concurrency),
                        "report"      => c.report      = v.to_string(),
                        _ => eprintln!("⚠ clé inconnue ignorée : {}", k),
                    }
                }
            }
        }
        Err(_) => {
            eprintln!("⚠ config '{}' introuvable — valeurs par défaut utilisées.", path);
        }
    }
    c
}

// =============================================================================
// PRNG maison (pour les ouvertures aléatoires reproductibles)
// =============================================================================

fn next_rand(state: &mut u64) -> u64 {
    // LCG + mélange final (splitmix-like). Déterministe pour une graine donnée.
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut x = *state;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x
}

// =============================================================================
// État de recherche d'un camp (persistant sur toute une partie)
// =============================================================================

struct SideState {
    tt:                TranspositionTable,
    killers:           KillerMoves,
    history:           HistoryTable,
    countermoves:      CountermoveTable,
    cont_history:      ContinuationHistoryTable,
    node_limit:         u64,
    disable_improving:   bool,
    disable_futility:    bool,
    disable_lmr_tweaks:  bool,
    disable_correction:  bool,
    disable_king_attack: bool,
}

impl SideState {
    fn new(node_limit: u64, disable_improving: bool, disable_futility: bool, disable_lmr_tweaks: bool, disable_correction: bool, disable_king_attack: bool) -> SideState {
        SideState {
            tt:                TranspositionTable::new(SELFPLAY_TT_MB),
            killers:           KillerMoves::new(),
            history:           HistoryTable::new(),
            countermoves:      CountermoveTable::new(),
            cont_history:      ContinuationHistoryTable::new(),
            node_limit,
            disable_improving,
            disable_futility,
            disable_lmr_tweaks,
            disable_correction,
            disable_king_attack,
        }
    }

    /// Cherche le meilleur coup pour la position courante, dans la limite de
    /// nœuds fixée. Réutilise ses tables (TT/history…) d'un coup à l'autre.
    fn best_move(&mut self, board: &mut Board) -> Move {
        let mut info = SearchInfo::new_with_stop(
            Duration::from_secs(3600),
            Arc::new(AtomicBool::new(false)),
        );
        info.max_nodes        = Some(self.node_limit);
        info.toggles.disable_improving   = self.disable_improving;
        info.toggles.disable_futility    = self.disable_futility;
        info.toggles.disable_lmr_tweaks  = self.disable_lmr_tweaks;
        info.toggles.disable_correction  = self.disable_correction;
        info.toggles.disable_king_attack = self.disable_king_attack;

        let mut chosen = Move::NULL;
        for depth in 1..=SELFPLAY_MAX_DEPTH {
            if info.should_stop() {
                break;
            }
            info.best_move = Move::NULL;
            alpha_beta(
                board, depth, -SCORE_MATE, SCORE_MATE, 0,
                &self.tt, &mut self.killers, &mut self.history,
                &mut self.countermoves, &mut self.cont_history, Move::NULL,
                &mut info, Move::NULL, &[],
            );
            // Si la limite de nœuds a coupé EN COURS de profondeur, on garde le
            // coup de la dernière profondeur COMPLÈTE (déjà dans `chosen`).
            if info.should_stop() {
                break;
            }
            chosen = info.best_move;
        }

        if chosen.is_null() {
            chosen = info.best_move; // résultat partiel d'une profondeur interrompue
        }
        if chosen.is_null() {
            // Ultime filet (ne devrait pas arriver si la position n'est pas terminale).
            let legal = generate_legal_moves(board);
            if !legal.is_empty() {
                chosen = legal[0];
            }
        }
        chosen
    }
}

// =============================================================================
// Déroulement d'une partie
// =============================================================================

/// Construit une ouverture aléatoire reproductible (graine = numéro de paire).
fn random_opening(seed: u64) -> Game {
    let mut game = Game::new();
    let mut rng = seed ^ 0x9E3779B97F4A7C15;
    for _ in 0..OPENING_PLIES {
        let legal = generate_legal_moves(&mut game.board);
        if legal.is_empty() {
            break; // position terminale atteinte (rare) — on s'arrête là
        }
        let idx = (next_rand(&mut rng) as usize) % legal.len();
        game.make_move(legal[idx]);
    }
    game
}

/// Joue une partie complète depuis `game`. `a_is_white` indique la couleur de A.
/// Retourne le résultat DU POINT DE VUE DE B (le candidat) :
///   +1 = B gagne, 0 = nulle, -1 = B perd.
fn play_out(mut game: Game, a_is_white: bool, side_a: &mut SideState, side_b: &mut SideState) -> i32 {
    let mut plies = 0usize;
    loop {
        match game.result() {
            GameResult::Ongoing => {}
            GameResult::Checkmate => {
                // Le camp au trait est maté → il perd.
                let loser_is_white = game.board.side_to_move == Color::White;
                let b_is_white     = !a_is_white;
                let b_loses        = loser_is_white == b_is_white;
                return if b_loses { -1 } else { 1 };
            }
            _ => return 0, // toute nulle (50 coups, répétition, matériel, pat)
        }

        if plies >= MAX_PLIES {
            return 0; // garde-fou : partie trop longue → nulle
        }

        let stm_is_white = game.board.side_to_move == Color::White;
        let a_to_move    = stm_is_white == a_is_white;

        let mv = if a_to_move {
            side_a.best_move(&mut game.board)
        } else {
            side_b.best_move(&mut game.board)
        };

        if mv.is_null() {
            return 0; // sécurité : aucun coup trouvé (ne devrait pas arriver)
        }
        game.make_move(mv);
        plies += 1;
    }
}

/// Joue UNE partie complète et autonome (crée ses propres camps). Sert d'unité
/// de travail aux threads parallèles. `seed` fixe l'ouverture, `a_white` la
/// couleur de A. Retourne +1/0/-1 du point de vue de B.
fn play_one_game(seed: u64, a_white: bool, cfg: &Config) -> i32 {
    let game = random_opening(seed);
    let mut side_a = SideState::new(cfg.nodes_a, !cfg.improving_a, !cfg.futility_a, !cfg.lmr_a, !cfg.correction_a, !cfg.king_attack_a);
    let mut side_b = SideState::new(cfg.nodes_b, !cfg.improving_b, !cfg.futility_b, !cfg.lmr_b, !cfg.correction_b, !cfg.king_attack_b);
    play_out(game, a_white, &mut side_a, &mut side_b)
}

// =============================================================================
// Statistiques SPRT (modèle normal, du point de vue de B = candidat)
// =============================================================================

fn score_from_elo(elo: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf(-elo / 400.0))
}

fn elo_from_score(s: f64) -> f64 {
    let s = s.clamp(1e-6, 1.0 - 1e-6);
    -400.0 * (1.0 / s - 1.0).log10()
}

/// Rapport de vraisemblance log (approximation normale).
fn llr(w: u64, d: u64, l: u64, elo0: f64, elo1: f64) -> f64 {
    let n = (w + d + l) as f64;
    if n == 0.0 {
        return 0.0;
    }
    let sum_x  = w as f64 + 0.5 * d as f64;        // Σ score
    let sum_x2 = w as f64 + 0.25 * d as f64;       // Σ score²
    let mean   = sum_x / n;
    let var    = sum_x2 / n - mean * mean;          // variance par partie
    if var <= 1e-9 {
        return 0.0; // pas d'information (que des nulles, etc.)
    }
    let s0 = score_from_elo(elo0);
    let s1 = score_from_elo(elo1);
    (s1 - s0) / var * (sum_x - n * (s0 + s1) / 2.0)
}

/// Elo estimé et demi-marge (95 %).
fn elo_and_margin(w: u64, d: u64, l: u64) -> (f64, f64) {
    let n = (w + d + l) as f64;
    if n == 0.0 {
        return (0.0, 0.0);
    }
    let sum_x  = w as f64 + 0.5 * d as f64;
    let sum_x2 = w as f64 + 0.25 * d as f64;
    let mean   = sum_x / n;
    let var    = (sum_x2 / n - mean * mean).max(0.0);
    let se     = (var / n).sqrt();
    let elo    = elo_from_score(mean);
    let lo     = elo_from_score(mean - 1.96 * se);
    let hi     = elo_from_score(mean + 1.96 * se);
    (elo, (hi - lo) / 2.0)
}

// =============================================================================
// Affichage et rapport
// =============================================================================

fn print_progress(cfg: &Config, w: u64, d: u64, l: u64, llr_val: f64, upper: f64, lower: f64) {
    let n = w + d + l;
    let (elo, margin) = elo_and_margin(w, d, l);
    let pct_cap = (n as f64 / cfg.games_max as f64 * 100.0).min(100.0);
    let (bound, dir) = if llr_val >= 0.0 { (upper, "PASS") } else { (lower, "FAIL") };
    let pct_verdict = if bound != 0.0 { (llr_val / bound * 100.0).clamp(0.0, 100.0) } else { 0.0 };
    println!(
        "[{:5.1}%]  {}/{} parties  |  B {}-{}-{} (G-N-P)  |  Elo {:+.1} ±{:.1}  |  LLR {:.2}/{:.2} → {} ({:.0}%)",
        pct_cap, n, cfg.games_max, w, d, l, elo, margin, llr_val, bound, dir, pct_verdict
    );
}

// Fonction de SÉRIALISATION pure : chaque paramètre est un champ distinct et
// nommé du rapport. Les regrouper dans une struct uniquement pour satisfaire le
// lint n'apporterait aucune clarté ici — d'où l'autorisation explicite et
// justifiée de dépasser le seuil d'arguments.
#[allow(clippy::too_many_arguments)]
fn write_report(cfg: &Config, w: u64, d: u64, l: u64, llr_val: f64, upper: f64, lower: f64, statut: &str) {
    let n = w + d + l;
    let (elo, margin) = elo_and_margin(w, d, l);
    let verdict = if statut.contains("PASS") {
        "garder la modif (B est plus fort)"
    } else if statut.contains("FAIL") {
        "retirer la modif (pas de gain)"
    } else {
        "résultat partiel — relancer pour conclure"
    };
    let content = format!(
"# Rapport SPRT Vendetta Chess Motor (point de vue B = candidat)
statut              = {statut}
verdict             = {verdict}

parties             = {n}
B_gagnees           = {w}
nulles              = {d}
B_perdues           = {l}

elo_estime          = {elo:.1}
elo_demi_marge_95   = {margin:.1}

llr                 = {llr_val:.3}
llr_borne_pass      = {upper:.3}
llr_borne_fail      = {lower:.3}

# Rappel de la config testée
config_nodes_a      = {}
config_nodes_b      = {}
config_improving_a  = {}
config_improving_b  = {}
config_futility_a   = {}
config_futility_b   = {}
config_lmr_a        = {}
config_lmr_b        = {}
config_correction_a = {}
config_correction_b = {}
config_king_attack_a = {}
config_king_attack_b = {}
config_elo0         = {}
config_elo1         = {}
config_alpha        = {}
config_beta         = {}
config_games_max    = {}
",
        cfg.nodes_a, cfg.nodes_b, cfg.improving_a, cfg.improving_b,
        cfg.futility_a, cfg.futility_b, cfg.lmr_a, cfg.lmr_b,
        cfg.correction_a, cfg.correction_b,
        cfg.king_attack_a, cfg.king_attack_b,
        cfg.elo0, cfg.elo1, cfg.alpha, cfg.beta, cfg.games_max,
    );
    if let Err(e) = fs::write(&cfg.report, content) {
        eprintln!("⚠ impossible d'écrire le rapport '{}' : {}", cfg.report, e);
    }
}

// =============================================================================
// Point d'entrée
// =============================================================================

fn main() {
    // Initialisation OBLIGATOIRE des tables d'attaque / magic (comme perft/benchmark).
    init_attack_tables();

    let args: Vec<String> = env::args().collect();
    let config_path = args.get(1).map(|s| s.as_str()).unwrap_or("selfplay_config.txt");
    let cfg = parse_config(config_path);

    let upper = ((1.0 - cfg.beta) / cfg.alpha).ln();
    let lower = (cfg.beta / (1.0 - cfg.alpha)).ln();

    // Nettoyer un éventuel STOP résiduel d'un run précédent.
    let _ = fs::remove_file(STOP_FILE);

    println!("=== SPRT self-play Vendetta Chess Motor ===");
    println!("config            : {}", config_path);
    println!("A (référence)     : nodes={}  improving={}  futility={}  lmr={}  correction={}  king_attack={}", cfg.nodes_a, cfg.improving_a, cfg.futility_a, cfg.lmr_a, cfg.correction_a, cfg.king_attack_a);
    println!("B (candidat)      : nodes={}  improving={}  futility={}  lmr={}  correction={}  king_attack={}", cfg.nodes_b, cfg.improving_b, cfg.futility_b, cfg.lmr_b, cfg.correction_b, cfg.king_attack_b);
    println!("bornes SPRT       : elo0={} elo1={} (alpha={} beta={})", cfg.elo0, cfg.elo1, cfg.alpha, cfg.beta);
    println!("plafond           : {} parties", cfg.games_max);
    println!("parallélisme      : {} parties simultanées", cfg.concurrency.max(1));
    println!("rapport           : {}", cfg.report);
    println!("arrêt propre      : créer un fichier nommé '{}' (ex: touch {})", STOP_FILE, STOP_FILE);
    println!();

    // --- État partagé entre threads (tout en atomique, zéro verrou) ----------
    // Chaque partie est indépendante : ses propres camps, son propre échiquier.
    // Le SEUL état partagé est ce bloc de compteurs atomiques.
    let cfg          = Arc::new(cfg);
    let wins         = Arc::new(AtomicU64::new(0)); // B gagne
    let draws        = Arc::new(AtomicU64::new(0));
    let losses       = Arc::new(AtomicU64::new(0)); // B perd
    let game_counter = Arc::new(AtomicU64::new(0)); // index de la prochaine partie à jouer
    let stop         = Arc::new(AtomicBool::new(false));

    let concurrency = cfg.concurrency.clamp(1, MAX_CONCURRENCY);

    // --- Threads ouvriers : jouent des parties tant que `stop` est faux ------
    // Index de partie g → ouverture seed = g/2, couleur a_white = (g pair).
    // Ainsi chaque ouverture est jouée dans les deux sens (équité des couleurs),
    // exactement comme l'ancien schéma de paires, mais réparti sur les threads.
    let mut handles = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let cfg          = Arc::clone(&cfg);
        let wins         = Arc::clone(&wins);
        let draws        = Arc::clone(&draws);
        let losses       = Arc::clone(&losses);
        let game_counter = Arc::clone(&game_counter);
        let stop         = Arc::clone(&stop);
        let h = thread::Builder::new()
            .stack_size(8 * 1024 * 1024) // marge pour la récursion alpha-bêta
            .spawn(move || {
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let g = game_counter.fetch_add(1, Ordering::Relaxed);
                    if g >= cfg.games_max {
                        break; // ne pas démarrer au-delà du plafond
                    }
                    let seed    = g / 2;
                    let a_white = g.is_multiple_of(2);
                    match play_one_game(seed, a_white, &cfg) {
                        1  => { wins.fetch_add(1, Ordering::Relaxed); }
                        -1 => { losses.fetch_add(1, Ordering::Relaxed); }
                        _  => { draws.fetch_add(1, Ordering::Relaxed); }
                    }
                }
            })
            .expect("échec du lancement d'un thread ouvrier");
        handles.push(h);
    }

    // --- Thread principal : scrute, affiche, sauvegarde, décide de l'arrêt ---
    // `statut` est assigné par CHAQUE branche de sortie de la boucle ci-dessous
    // (la boucle ne sort que par `break`), donc pas de valeur initiale inutile.
    let statut: String;
    // Nombre de parties au dernier affichage : évite de réimprimer une ligne
    // identique quand aucune partie ne s'est terminée entre deux scrutations
    // (fréquent à gros budget de nœuds, où une partie dure plusieurs scrutations).
    let mut last_reported = u64::MAX;
    loop {
        thread::sleep(Duration::from_millis(POLL_MS));

        let w = wins.load(Ordering::Relaxed);
        let d = draws.load(Ordering::Relaxed);
        let l = losses.load(Ordering::Relaxed);
        let llr_val = llr(w, d, l, cfg.elo0, cfg.elo1);

        // Arrêt propre via fichier STOP.
        if Path::new(STOP_FILE).exists() {
            statut = "INTERROMPU".to_string();
            let _ = fs::remove_file(STOP_FILE);
            break;
        }
        // Plafond de parties atteint.
        if w + d + l >= cfg.games_max {
            statut = "PLAFOND_ATTEINT".to_string();
            break;
        }
        // SPRT conclu ?
        if llr_val >= upper {
            statut = "SPRT_CONCLU_PASS".to_string();
            break;
        }
        if llr_val <= lower {
            statut = "SPRT_CONCLU_FAIL".to_string();
            break;
        }
        // Progression + autosave — seulement si le compteur de parties a bougé,
        // pour ne pas réimprimer une ligne identique (anti-spam).
        let n = w + d + l;
        if n != last_reported {
            last_reported = n;
            print_progress(&cfg, w, d, l, llr_val, upper, lower);
            write_report(&cfg, w, d, l, llr_val, upper, lower, "EN_COURS");
        }
    }

    // Signaler l'arrêt et attendre que les parties en cours se terminent.
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }

    // --- Rapport final (lecture après join : inclut les dernières parties) ---
    let w = wins.load(Ordering::Relaxed);
    let d = draws.load(Ordering::Relaxed);
    let l = losses.load(Ordering::Relaxed);
    let final_llr = llr(w, d, l, cfg.elo0, cfg.elo1);
    println!();
    print_progress(&cfg, w, d, l, final_llr, upper, lower);
    println!("--> statut final : {}", statut);
    write_report(&cfg, w, d, l, final_llr, upper, lower, &statut);
    println!("--> rapport écrit dans : {}", cfg.report);
}
