// =============================================================================
// Vendetta Chess Motor — src/uci/mod.rs
//
// Rôle : Implémentation complète du protocole UCI (Universal Chess Interface).
//        Boucle principale de communication entre le moteur et l'interface
//        graphique (Arena, Lichess, Chessbase, etc.).
//
// Contenu :
//   - UciEngine : structure principale qui orchestre tout
//   - run() : boucle principale UCI (lit stdin, écrit stdout)
//   - Gestion de toutes les commandes UCI obligatoires + pondering
//
// Architecture de la boucle UCI :
//   - La lecture de stdin se fait dans un thread dédié via un canal mpsc.
//     Cela permet à la boucle principale de rester réactive (notamment à
//     "stop" et "ponderhit") pendant qu'une recherche est en cours.
//   - La recherche s'exécute dans un thread séparé (spawn_search).
//
// Pondering (réflexion sur le temps adverse) :
//   Machine d'état à deux modes : Normal et Ponder.
//
//   Flux ponder complet :
//     1. Engine → GUI : "bestmove e2e4 ponder e7e5"
//     2. GUI  → Engine : "position ... moves e2e4 e7e5"
//                        "go ponder wtime 300000 btime 300000 ..."
//     3. Engine : lance une recherche INFINIE sur la position après e7e5
//                 (is_pondering = true, ponder_config sauvegardé)
//     4a. GUI → Engine : "ponderhit" (adversaire a joué e7e5)
//          → on stoppe la recherche ponder (TT déjà chaude)
//          → on relance une recherche NORMALE avec ponder_config (gestion du temps)
//     4b. GUI → Engine : "stop" (adversaire a joué autre chose)
//          → on stoppe la recherche ponder
//          → on envoie "bestmove <meilleur_coup_dans_position_ponder>"
//          → la GUI enverra ensuite "position" + "go" pour la vraie position
//
// Options UCI de Vendetta Chess Motor :
//   - Hash (MB)      : taille de la table de transposition (défaut 16 Mo)
//   - Skill Level    : niveau de difficulté 1-64 (défaut 64 = pleine puissance)
//   - Threads        : nombre de threads de recherche (défaut = cœurs disponibles)
//   - Ponder         : active le mode pondering (défaut = true)
// =============================================================================

pub mod parser;

use std::io::{self, BufRead, Write};
use std::sync::{Arc, mpsc};
use std::sync::atomic::Ordering;
use std::time::Duration;
use crate::game::Game;
use crate::search::{SearchEngine, SearchConfig, SearchResult, elo_to_skill_level, ELO_MIN, ELO_MAX};
use crate::search::killers::KillerMoves;
use crate::search::history::HistoryTable;
use crate::search::countermove::CountermoveTable;
use crate::search::continuation_history::ContinuationHistoryTable;
use crate::utils::types::Move;
use parser::{parse_command, parse_move_uci, UciCommand};

/// Nom et version du moteur.
pub const ENGINE_NAME:    &str = "Vendetta Chess Motor";
pub const ENGINE_VERSION: &str = "1.1.2";
/// Auteur du projet. Développé en coworking avec Claude (Anthropic) — voir
/// la section "Remerciements" du README.md pour le détail honnête de cette
/// collaboration humain/IA.
pub const ENGINE_AUTHOR:  &str = "Fabrice Garcia";

/// Moteur UCI principal.
pub struct UciEngine {
    /// Partie en cours.
    game: Game,
    /// Moteur de recherche.
    search_engine: SearchEngine,
    /// Niveau de difficulté actuel (1-64), piloté par l'option maison "Skill Level".
    skill_level: u8,
    /// Taille de la table de transposition en Mo.
    hash_size_mb: usize,

    // --- Limitation de force standard UCI ---

    /// true si "UCI_LimitStrength" est activé : dans ce cas, `elo` prend le
    /// pas sur `skill_level` pour déterminer la force de jeu (voir la
    /// commande Go). Permet aux GUIs/plateformes standards de brider
    /// Vendetta Chess Motor sans connaître l'option maison "Skill Level".
    limit_strength: bool,
    /// Force cible en Elo quand `limit_strength` est actif (option "UCI_Elo").
    elo: u16,
    /// Nombre de variantes principales à afficher (option "MultiPV").
    /// 1 = comportement standard (une seule meilleure ligne).
    multipv: usize,
    /// Marge de sécurité (ms) retirée du budget de temps (option "Move
    /// Overhead") — compense la latence GUI/réseau, évite les pertes au
    /// temps en ligne/tournoi. 50 ms par défaut (valeur de l'ancienne marge
    /// fixe codée en dur dans compute_time_limit(), désormais réglable).
    move_overhead_ms: u64,
    /// true si "UCI_AnalyseMode" est activé : force toujours le meilleur
    /// coup (skill_level = 64), quels que soient "Skill Level" ou
    /// "UCI_LimitStrength" — un outil d'analyse ne doit jamais recevoir
    /// une erreur volontaire du système de niveaux de difficulté.
    analyse_mode: bool,
    /// Facteur de contempt en centipions (option UCI "Contempt"). 0 par
    /// défaut = comportement inchangé (aucune pénalité sur les positions
    /// nulles). Une valeur positive pénalise légèrement la nullité du point
    /// de vue du camp que le moteur joue actuellement — utile contre un
    /// adversaire plus faible, pour continuer à chercher la victoire plutôt
    /// que de se contenter d'un partage des points. Voir
    /// alphabeta.rs::draw_score() pour le mécanisme exact.
    contempt: i32,
    /// Handle du thread de recherche en cours (None si inactif).
    search_handle: Option<std::thread::JoinHandle<SearchResult>>,

    // --- État du pondering ---

    /// true quand la recherche en cours est en mode ponder
    /// (réflexion sur le temps de l'adversaire).
    is_pondering: bool,
    /// Configuration UCI sauvegardée lors du "go ponder".
    /// Utilisée pour lancer la recherche normale au "ponderhit".
    ponder_config: Option<SearchConfig>,

    // --- Mode debug ---

    /// true si le mode debug UCI est activé ("debug on").
    /// Quand actif, le moteur peut émettre des "info string" supplémentaires
    /// pour faciliter le diagnostic.
    debug_mode: bool,
}

impl UciEngine {
    /// Crée un nouveau moteur UCI.
    pub fn new() -> UciEngine {
        UciEngine {
            game:          Game::new(),
            search_engine: SearchEngine::new(),
            skill_level:   64,
            hash_size_mb:  32,
            limit_strength: false,
            elo:            ELO_MAX,
            multipv:        1,
            move_overhead_ms: 50,
            analyse_mode:     false,
            contempt:         0,
            search_handle: None,
            is_pondering:  false,
            ponder_config: None,
            debug_mode:    false,
        }
    }

    /// Boucle principale UCI.
    ///
    /// Architecture :
    ///   - Thread dédié pour stdin → canal mpsc (recv_timeout 5 ms).
    ///   - Boucle principale non bloquante → réactive aux commandes en temps réel.
    ///   - Recherche dans un thread séparé (spawn_search).
    ///   - check_search_done() détecte la fin de recherche et émet bestmove.
    pub fn run(&mut self) {
        let stdout = io::stdout();

        // Thread dédié à la lecture de stdin (non-bloquant pour la boucle principale).
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l)  => { if tx.send(l).is_err() { break; } }
                    Err(_) => break,
                }
            }
        });

        'main: loop {
            // Vérifier si la recherche en cours est terminée → émettre bestmove
            self.check_search_done(&stdout);

            // Attendre une commande UCI (timeout pour rester réactif)
            let line = match rx.recv_timeout(Duration::from_millis(5)) {
                Ok(l)                                     => l,
                Err(mpsc::RecvTimeoutError::Timeout)      => continue 'main,
                Err(mpsc::RecvTimeoutError::Disconnected) => break 'main,
            };

            match parse_command(&line) {

                UciCommand::Uci => {
                    self.cmd_uci();
                }

                UciCommand::IsReady => {
                    println!("readyok");
                }

                UciCommand::UciNewGame => {
                    self.cmd_new_game();
                }

                UciCommand::Position { fen, moves } => {
                    // Si un ponder est en cours sur une position différente,
                    // il doit être stoppé (la GUI envoie une nouvelle position).
                    self.abort_ponder();
                    self.cmd_position(fen, moves);
                }

                UciCommand::Go(mut config) => {
                    // Priorité (de la plus forte à la plus faible) :
                    //   1. UCI_AnalyseMode : toujours le meilleur coup, jamais
                    //      d'erreur volontaire — un outil d'analyse ne doit
                    //      jamais recevoir une réponse délibérément affaiblie.
                    //   2. UCI_LimitStrength + UCI_Elo : mécanisme standard de
                    //      bridage, pour les GUIs/plateformes qui ne connaissent
                    //      pas l'option maison "Skill Level".
                    //   3. "Skill Level" : option maison, réglage par défaut.
                    config.skill_level = if self.analyse_mode {
                        64
                    } else if self.limit_strength {
                        elo_to_skill_level(self.elo)
                    } else {
                        self.skill_level
                    };
                    config.multipv       = self.multipv;
                    config.move_overhead = self.move_overhead_ms;
                    config.contempt      = self.contempt;

                    // Arrêter toute recherche en cours (normale ou ponder)
                    self.stop_current_search();

                    // Réinitialiser l'état ponder systématiquement avant tout nouveau Go,
                    // même si ce n'est pas un Go ponder (défense contre GUIs non conformes).
                    self.is_pondering  = false;
                    self.ponder_config = None;

                    if config.ponder {
                        // --- Mode ponder ---
                        // Sauvegarder la config pour l'utiliser lors du ponderhit.
                        // Lancer une recherche INFINIE sur la position adverse attendue.
                        self.ponder_config = Some(config.clone());
                        self.is_pondering  = true;

                        // Le thread de recherche tourne en mode infini :
                        // il ignorera les limites de temps et attendra stop/ponderhit.
                        let mut ponder_cfg     = config;
                        ponder_cfg.infinite    = true;
                        ponder_cfg.ponder      = false; // Le thread n'a pas besoin de le savoir
                        ponder_cfg.wtime       = None;  // Pas de gestion du temps en ponder
                        ponder_cfg.btime       = None;
                        ponder_cfg.movetime    = None;

                        match self.spawn_search(ponder_cfg) {
                            Ok(handle) => self.search_handle = Some(handle),
                            Err(_) => {
                                // Thread impossible à créer : le ponder est une
                                // recherche INFINIE, on ne peut donc PAS la faire
                                // en synchrone (elle bloquerait à jamais). On
                                // abandonne le ponder silencieusement ; la GUI
                                // enverra un vrai "go" (ou "ponderhit") ensuite.
                                self.is_pondering  = false;
                                self.ponder_config = None;
                            }
                        }
                    } else {
                        // --- Mode normal ---
                        match self.spawn_search(config.clone()) {
                            Ok(handle) => self.search_handle = Some(handle),
                            Err(_) => {
                                // REPLI ROBUSTE : l'OS ne peut pas créer le thread
                                // de recherche. Plutôt que de planter, on cherche
                                // en SYNCHRONE sur le thread courant, forcé à 1
                                // thread (les spawns SMP échoueraient aussi). La
                                // boucle UCI est bloquée le temps de la recherche,
                                // mais la limite de temps est respectée → un
                                // bestmove est bien émis.
                                let board = self.game.board.clone();
                                let tt    = Arc::clone(&self.search_engine.tt);
                                let stop  = Arc::clone(&self.search_engine.stop_flag);
                                stop.store(false, Ordering::SeqCst);
                                let result = run_search(tt, stop, 1, board, config);
                                self.emit_bestmove(&result, &stdout);
                            }
                        }
                    }
                }

                UciCommand::PonderHit => {
                    // L'adversaire a joué le coup prédit : on sort du mode ponder
                    // et on démarre une recherche normale avec les paramètres sauvegardés.
                    if self.is_pondering {
                        // 1. Stopper la recherche ponder (la TT reste chaude)
                        self.search_engine.stop();
                        if let Some(h) = self.search_handle.take() {
                            let _ = h.join(); // Attendre la fin propre (~quelques ms)
                        }
                        self.is_pondering = false;

                        // 2. Lancer la recherche normale avec les paramètres du "go ponder"
                        if let Some(mut real_cfg) = self.ponder_config.take() {
                            real_cfg.ponder = false;
                            // Réinitialiser le stop_flag avant de lancer la nouvelle recherche
                            self.search_engine.stop_flag.store(false, Ordering::SeqCst);
                            match self.spawn_search(real_cfg.clone()) {
                                Ok(handle) => self.search_handle = Some(handle),
                                Err(_) => {
                                    // Même repli robuste que pour un "go" normal :
                                    // recherche synchrone mono-thread plutôt que panic.
                                    let board = self.game.board.clone();
                                    let tt    = Arc::clone(&self.search_engine.tt);
                                    let stop  = Arc::clone(&self.search_engine.stop_flag);
                                    stop.store(false, Ordering::SeqCst);
                                    let result = run_search(tt, stop, 1, board, real_cfg);
                                    self.emit_bestmove(&result, &stdout);
                                }
                            }
                        }
                    }
                    // Si ponderhit arrive sans go ponder préalable : on l'ignore.
                }

                UciCommand::Stop => {
                    // Arrêt de toute recherche en cours.
                    // En mode ponder : la GUI a décidé d'arrêter (adversaire a joué autre chose).
                    // bestmove sera émis par check_search_done() à la prochaine itération.
                    self.is_pondering = false;
                    self.ponder_config = None;
                    self.search_engine.stop();
                }

                UciCommand::Debug { on } => {
                    self.debug_mode = on;
                    if self.debug_mode {
                        println!("info string debug mode enabled");
                    }
                }

                UciCommand::Register => {
                    // Vendetta Chess Motor n'a AUCUNE protection anti-copie : on
                    // accepte la commande sans rien faire. La spec interdit
                    // d'émettre une réponse "registration" si le moteur n'en a
                    // pas besoin — donc no-op. Signalé seulement en mode debug.
                    if self.debug_mode {
                        println!("info string register ignoré (aucune protection anti-copie)");
                    }
                }

                UciCommand::SetOption { name, value } => {
                    self.cmd_setoption(&name, &value);
                }

                UciCommand::Quit => {
                    // Arrêt propre avant de quitter
                    self.search_engine.stop();
                    if let Some(h) = self.search_handle.take() {
                        let _ = h.join();
                    }
                    break 'main;
                }

                UciCommand::Unknown => {
                    // Ignorer silencieusement (requis par la spec UCI)
                }
            }

            let _ = stdout.lock().flush();
        }
    }

    // =========================================================================
    // Gestion du thread de recherche
    // =========================================================================

    /// Stoppe la recherche en cours et attend la fin du thread.
    /// Ne touche pas à is_pondering (appelant responsable).
    fn stop_current_search(&mut self) {
        if self.search_handle.is_some() {
            self.search_engine.stop();
            if let Some(h) = self.search_handle.take() {
                let _ = h.join();
            }
        }
    }

    /// Stoppe un ponder en cours sans émettre bestmove.
    /// Utilisé quand la GUI envoie une nouvelle position pendant un ponder.
    fn abort_ponder(&mut self) {
        if self.is_pondering {
            self.search_engine.stop();
            if let Some(h) = self.search_handle.take() {
                let _ = h.join();
            }
            self.is_pondering  = false;
            self.ponder_config = None;
        }
    }

    /// Vérifie si la recherche est terminée et émet bestmove le cas échéant.
    ///
    /// En mode ponder, cette fonction est appelée à chaque itération mais
    /// n'émet JAMAIS bestmove tant que is_pondering est true : la recherche
    /// ponder tourne jusqu'à ponderhit ou stop.
    fn check_search_done(&mut self, stdout: &io::Stdout) {
        // En mode ponder actif, la recherche tourne librement — on ne fait rien.
        if self.is_pondering {
            // Vérification de sécurité : si le thread se termine de lui-même pendant
            // un ponder (impossible en mode infini, mais défense en profondeur), on nettoie
            // sans émettre bestmove (ce n'est pas une fin de recherche normale).
            let thread_done = self.search_handle
                .as_ref()
                .map(|h| h.is_finished())
                .unwrap_or(false);
            if thread_done {
                if let Some(h) = self.search_handle.take() {
                    let _ = h.join();
                }
            }
            return;
        }

        // Mode normal : émettre bestmove dès que le thread est terminé.
        if let Some(handle) = self.search_handle.take() {
            if handle.is_finished() {
                match handle.join() {
                    Ok(result) => {
                        self.emit_bestmove(&result, stdout);
                    }
                    Err(_) => {
                        eprintln!("info string Erreur interne : le thread de recherche a planté");
                        println!("bestmove (none)");
                        let _ = stdout.lock().flush();
                    }
                }
            } else {
                // Recherche encore en cours : remettre le handle
                self.search_handle = Some(handle);
            }
        }
    }

    /// Émet "bestmove <coup> [ponder <coup_attendu>]" sur stdout.
    fn emit_bestmove(&self, result: &SearchResult, stdout: &io::Stdout) {
        if result.best_move.is_null() {
            // Aucun coup légal (mat ou pat).
            // On émet "(none)" plutôt que "0000" : la notation "0000" n'est pas
            // standard UCI et certaines GUIs (Cutechess, Fritz) la rejettent ou
            // se déconnectent. "(none)" est la convention acceptée universellement.
            println!("bestmove (none)");
        } else if !result.ponder_move.is_null() {
            // Inclure le coup de réponse prédit pour que la GUI puisse lancer un ponder
            println!("bestmove {} ponder {}",
                result.best_move.to_uci(),
                result.ponder_move.to_uci());
        } else {
            println!("bestmove {}", result.best_move.to_uci());
        }
        let _ = stdout.lock().flush();
    }

    /// Lance la recherche dans un thread dédié et renvoie son JoinHandle.
    ///
    /// Renvoie `Err` si l'OS refuse de créer le thread (ressources épuisées,
    /// cas catastrophique) AU LIEU de paniquer : l'appelant (commande Go)
    /// bascule alors sur un repli synchrone. Comportement normal strictement
    /// inchangé — le chemin `Ok` est exactement l'ancien.
    fn spawn_search(
        &mut self,
        config: SearchConfig,
    ) -> std::io::Result<std::thread::JoinHandle<SearchResult>> {
        let board       = self.game.board.clone();
        let tt          = Arc::clone(&self.search_engine.tt);
        let stop        = Arc::clone(&self.search_engine.stop_flag);
        let num_threads = self.search_engine.num_threads;

        // Réinitialiser le signal d'arrêt avant le lancement.
        stop.store(false, Ordering::SeqCst);

        // Pile de 8 Mio (au lieu du défaut ~2 Mio) : ce thread mène la recherche
        // PRINCIPALE à pleine profondeur, le plus exposé à la récursion profonde
        // + aux listes de coups allouées sur la pile (MoveList, captures scorées).
        // Le corps est délégué à run_search(), partagé avec le repli synchrone.
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || run_search(tt, stop, num_threads, board, config))
    }

    // =========================================================================
    // Gestionnaires des commandes UCI
    // =========================================================================

    /// Commande "uci" : identifier le moteur et lister les options.
    fn cmd_uci(&self) {
        println!("id name {} {}", ENGINE_NAME, ENGINE_VERSION);
        println!("id author {}", ENGINE_AUTHOR);
        println!();

        println!("option name Hash type spin default 32 min 1 max 32768");
        println!("option name Skill Level type spin default 64 min 1 max 64");
        println!("option name Ponder type check default true");
        println!("option name Debug type check default false");

        let default_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        println!("option name Threads type spin default {} min 1 max 768", default_threads);

        // --- Options UCI standards (interopérabilité avec GUIs/plateformes) ---
        // UCI_LimitStrength + UCI_Elo : mécanisme standard de bridage de force,
        // alternative à "Skill Level" pour les outils qui ne connaissent pas
        // cette option maison (voir search::elo_to_skill_level).
        println!("option name UCI_LimitStrength type check default false");
        println!("option name UCI_Elo type spin default {} min {} max {}", ELO_MAX, ELO_MIN, ELO_MAX);
        // MultiPV : nombre de variantes principales affichées (1 = défaut standard).
        println!("option name MultiPV type spin default 1 min 1 max 218");

        // Move Overhead : marge de sécurité (ms) contre les pertes au temps
        // dues à la latence GUI/réseau — remplace l'ancienne marge fixe de
        // 50 ms codée en dur dans compute_time_limit() (même valeur par défaut).
        println!("option name Move Overhead type spin default 50 min 0 max 5000");

        // Clear Hash : bouton qui vide la table de transposition manuellement,
        // sans avoir à envoyer "ucinewgame" (qui réinitialise aussi killers/history).
        println!("option name Clear Hash type button");

        // UCI_AnalyseMode : force toujours le meilleur coup (skill_level=64),
        // prioritaire sur "Skill Level" et "UCI_LimitStrength" — voir la
        // commande Go. Utile pour les outils d'analyse qui ne veulent jamais
        // d'erreur volontaire de la part du moteur.
        println!("option name UCI_AnalyseMode type check default false");

        // Contempt : pénalise légèrement les positions nulles (toutes causes)
        // du point de vue du camp que le moteur joue actuellement — utile
        // contre un adversaire plus faible (continuer à chercher la victoire
        // plutôt que se contenter d'un partage des points). 0 = comportement
        // inchangé (par défaut). Convention standard : centipions, plage
        // -100 à 100. Voir alphabeta.rs::draw_score() pour le mécanisme.
        println!("option name Contempt type spin default 0 min -100 max 100");

        // UCI_EngineAbout : information cosmétique sur le moteur, sans effet
        // fonctionnel — convention UCI pour afficher auteur/licence/lien.
        // Auteur initial = ENGINE_AUTHOR (Fabrice Garcia) ; le moteur a été
        // développé en coworking avec Claude (Anthropic), mentionné ici en
        // tant que contributeur par souci d'honnêteté.
        println!(
            "option name UCI_EngineAbout type string default {} {} - Auteur initial: {} - Contributeur: Claude (Anthropic) - GPL-3.0",
            ENGINE_NAME, ENGINE_VERSION, ENGINE_AUTHOR
        );
        println!();

        println!("uciok");
    }

    /// Commande "ucinewgame" : réinitialiser pour une nouvelle partie.
    fn cmd_new_game(&mut self) {
        // Stopper tout ponder ou recherche en cours avant le reset
        self.abort_ponder();
        self.stop_current_search();
        self.game.reset();
        self.search_engine.new_game();
    }

    /// Commande "position" : définir la position courante.
    fn cmd_position(&mut self, fen: Option<String>, moves: Vec<String>) {
        if let Some(fen_str) = fen {
            match Game::from_fen(&fen_str) {
                Ok(game) => { self.game = game; }
                Err(e)   => {
                    eprintln!("info string Erreur FEN : {}", e);
                    return;
                }
            }
        } else {
            self.game = Game::new();
        }

        for mv_str in &moves {
            if let Some(mv) = parse_move_uci(mv_str, &mut self.game.board) {
                self.game.make_move(mv);
            } else {
                eprintln!("info string Coup invalide ignoré : {}", mv_str);
                break;
            }
        }
    }

    /// Commande "setoption" : configurer une option du moteur.
    fn cmd_setoption(&mut self, name: &str, value: &str) {
        match name {
            "Hash" => {
                if let Ok(size) = value.parse::<usize>() {
                    // Plafond à 32 Go (32768 Mo) : large pour toute machine
                    // moderne, sans changer le défaut (32 Mo) qui protège les
                    // configs modestes.
                    let requested = size.clamp(1, 32768);

                    // REPLI GRACIEUX (priorité robustesse de Vendetta Chess Motor) :
                    // on tente la taille demandée, puis on RÉDUIT DE MOITIÉ tant
                    // que l'allocation échoue — au lieu de planter. La TT est
                    // allouée d'un bloc : un réglage Hash plus grand que la RAM
                    // disponible ne doit JAMAIS tuer le moteur. En dernier
                    // recours (même la taille minimale échoue), on conserve la
                    // table actuelle. Un message "info string" informe la GUI.
                    let mut try_size = requested;
                    loop {
                        if let Some(tt) =
                            crate::search::transposition::TranspositionTable::try_new(try_size)
                        {
                            if try_size != requested {
                                eprintln!(
                                    "info string Hash {} Mo impossible (mémoire insuffisante) \
                                     — repli sur {} Mo",
                                    requested, try_size
                                );
                            }
                            self.hash_size_mb = try_size;
                            self.search_engine.tt = Arc::new(tt);
                            break;
                        }
                        if try_size <= 1 {
                            eprintln!(
                                "info string Hash {} Mo impossible — table de transposition \
                                 actuelle conservée",
                                requested
                            );
                            break;
                        }
                        try_size /= 2;
                    }
                }
            }
            "Skill Level" => {
                if let Ok(level) = value.parse::<u8>() {
                    self.skill_level = level.clamp(1, 64);
                }
            }
            "Threads" => {
                if let Ok(n) = value.parse::<usize>() {
                    self.search_engine.num_threads = n.clamp(1, 768);
                }
            }
            "Ponder" => {
                // Option informative — le pondering est toujours supporté.
                // On accepte l'option pour la conformité UCI sans action nécessaire.
            }
            "UCI_LimitStrength" => {
                self.limit_strength = value.eq_ignore_ascii_case("true");
            }
            "UCI_Elo" => {
                if let Ok(elo) = value.parse::<u16>() {
                    self.elo = elo.clamp(ELO_MIN, ELO_MAX);
                }
            }
            "MultiPV" => {
                if let Ok(n) = value.parse::<usize>() {
                    self.multipv = n.clamp(1, 218);
                }
            }
            "Move Overhead" => {
                if let Ok(ms) = value.parse::<u64>() {
                    self.move_overhead_ms = ms.min(5000);
                }
            }
            "Clear Hash" => {
                // Option de type "button" : aucune valeur, l'arrivée même de
                // la commande déclenche l'action. Vide la TT immédiatement —
                // équivalent partiel à "ucinewgame", mais sans toucher aux
                // killer moves / history (qui ne sont réinitialisés qu'entre
                // deux parties, pas en cours de réflexion).
                self.search_engine.tt.clear();
            }
            "UCI_AnalyseMode" => {
                self.analyse_mode = value.eq_ignore_ascii_case("true");
            }
            "Contempt" => {
                if let Ok(cp) = value.parse::<i32>() {
                    self.contempt = cp.clamp(-100, 100);
                }
            }
            "UCI_EngineAbout" => {
                // Option informative en lecture — déclarée pour la conformité
                // UCI (toute option annoncée doit pouvoir être "set" sans
                // erreur), mais sans effet : c'est le moteur qui informe la
                // GUI via cette option, pas l'inverse.
            }
            _ => {
                eprintln!("info string Option inconnue : {}", name);
            }
        }
    }
}

impl Default for UciEngine {
    fn default() -> Self {
        UciEngine::new()
    }
}

/// Exécute une recherche complète (MultiPV compris) et renvoie le MEILLEUR
/// résultat (results[0]).
///
/// Fonction partagée par deux appelants :
///   - le thread de recherche dédié (cas NORMAL, via spawn_search) ;
///   - le repli SYNCHRONE déclenché si la création de ce thread échoue
///     (voir la commande Go) — appelée alors avec `num_threads = 1`.
///
/// L'extraction évite toute duplication : une seule définition du déroulé de
/// recherche + de l'affichage MultiPV. Le comportement du cas normal est
/// strictement identique à l'ancienne closure inline.
fn run_search(
    tt:          Arc<crate::search::transposition::TranspositionTable>,
    stop:        Arc<std::sync::atomic::AtomicBool>,
    num_threads: usize,
    mut board:   crate::board::state::Board,
    config:      SearchConfig,
) -> SearchResult {
    let mut engine = SearchEngine {
        tt,
        killers:      KillerMoves::new(),
        history:      HistoryTable::new(),
        countermoves: CountermoveTable::new(),
        cont_history: ContinuationHistoryTable::new(),
        num_threads,
        stop_flag:    stop,
    };

    // search_multipv() est strictement équivalente à search() quand
    // config.multipv <= 1 (cas par défaut) — zéro changement de comportement
    // pour une partie normale.
    let results = engine.search_multipv(&mut board, &config);

    // En MultiPV (>1 ligne), un récapitulatif par variante, de la meilleure (1)
    // à la moins bonne. (Limitation connue : les "info depth" intermédiaires ne
    // portent pas le champ "multipv" ; sans incidence sur le résultat final.)
    if results.len() > 1 {
        for (i, r) in results.iter().enumerate() {
            let nps = crate::search::compute_nps(r.nodes, r.time_ms);
            println!(
                "info multipv {} depth {} score {} nodes {} nps {} time {} pv {}",
                i + 1, r.depth, crate::search::format_score(r.score),
                r.nodes, nps, r.time_ms, r.best_move.to_uci(),
            );
        }
        let _ = io::stdout().lock().flush();
    }

    // bestmove/ponder reposent toujours sur la MEILLEURE ligne (results[0]).
    results.into_iter().next().unwrap_or(SearchResult {
        best_move: Move::NULL, ponder_move: Move::NULL,
        score: 0, depth: 0, nodes: 0, time_ms: 0,
    })
}
