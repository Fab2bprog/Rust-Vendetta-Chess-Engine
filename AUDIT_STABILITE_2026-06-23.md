# Audit de stabilité — Vendetta Chess Motor v1.0.0

> Date : 2026-06-23 · Périmètre : recherche de bugs, coups illégaux, crashs
> Méthode : **audit statique du code source** (`src/`, ~11 600 lignes)

---

## Limite importante de cet audit

Je **n'ai pas pu compiler ni exécuter** le moteur dans l'environnement d'audit :
pas de toolchain Rust disponible, installation `rustup` bloquée par le proxy
réseau, et les binaires pré-compilés du dépôt sont des Mach-O arm64 (macOS),
non exécutables sous Linux. Les conclusions sur **perft / coups illégaux** et
sur les **crashs sous charge** reposent donc sur la **lecture du code**, pas sur
une exécution. À rejouer sur votre Mac pour confirmation :

```
cargo build --release --target aarch64-apple-darwin
cargo test
cargo run --release --bin perft          # doit afficher 6/6 PASS
```

---

## Verdict global

Le code est **solide et défensif**. La gestion d'erreur récupérable par
`Option`/`Result` est appliquée systématiquement, les bornes de récursion et
d'indexation de tableaux sont correctement posées, et l'entrée non fiable
(FEN, commandes UCI) est validée avant usage. **Un seul vrai vecteur de crash**
a été trouvé, de probabilité faible en usage réel (entrée non-ASCII).
Aucun vecteur de coup illégal trouvé par revue statique.

| Sévérité | Sujet | Statut |
|---|---|---|
| 🟠 Moyenne-faible | Panic sur token de coup non-ASCII (`parse_move_uci`) | **Confirmé** |
| 🟡 Faible | Marge de pile des threads secondaires (récursion profonde) | À surveiller |
| 🟡 Faible | Mémoire à `Threads=768` (~0,5 Go d'allocations) | Par conception |
| ℹ️ Info | 2 features désactivées (improving, échecs en quiescence) | Connu, bug de FORCE pas de crash |

---

## 1. Bug confirmé — panic sur token de coup non-ASCII

**Fichier** : `src/uci/parser.rs`, `parse_move_uci()`, lignes 287-288

```rust
let from = square_from_str(&mv_str[0..2])?;   // découpage par OCTETS
let to   = square_from_str(&mv_str[2..4])?;
```

`mv_str.len()` est une longueur en **octets**, et `&mv_str[0..2]` découpe aussi
par octets. Si un token de coup contient un caractère multi-octets dont une
frontière tombe à l'intérieur de `[0..2]` ou `[2..4]`, Rust **panique**
(« byte index N is not a char boundary »).

**Chemin d'exécution réel** : `cmd_position()` (`uci/mod.rs:553`) passe chaque
token brut de `position ... moves <token>` directement à `parse_move_uci`. Le
seul garde-fou amont est `mv_str.len() < 4` (en octets), qui laisse passer un
token comme `🙂e4` (6 octets) → `&mv_str[0..2]` coupe au milieu de l'emoji →
**crash du moteur**.

**Reproduction** :
```
position startpos moves 🙂e4
```

**Probabilité réelle** : faible — toute GUI UCI standard (Arena, Cutechess,
lichess-bot…) n'envoie que de l'ASCII. Mais c'est un panic non rattrapé,
déclenché par une entrée externe : il viole la règle du projet « jamais de
panic en production ».

**Correctif suggéré** (au choix, en tête de `parse_move_uci`) :

```rust
// Option A : rejeter d'emblée tout token non-ASCII
if !mv_str.is_ascii() || mv_str.len() < 4 {
    return None;
}
```
```rust
// Option B : découpage non paniquant
let from = square_from_str(mv_str.get(0..2)?)?;
let to   = square_from_str(mv_str.get(2..4)?)?;
```
L'option A est la plus simple et la plus sûre (un coup UCI est toujours ASCII).

---

## 2. Génération de coups / coups illégaux

Aucun vecteur de coup illégal trouvé par revue statique :

- **Validation des coups GUI** : `parse_move_uci` ne retourne un `Move` que
  s'il figure dans `generate_legal_moves(board)` — le moteur ne peut donc ni
  accepter ni jouer un coup illégal venant de l'interface. Les `MoveFlags`
  proviennent toujours du générateur (jamais reconstruits depuis la chaîne).
- **Roque** : droits de roque validés dans `from_fen` contre la **présence
  réelle** du roi et de la tour sur leurs cases (`state.rs:419-477`), avec
  retrait silencieux du droit incohérent (comportement correct vis-à-vis des
  GUIs qui envoient des droits résiduels).
- **Prise en passant** : rang de la case EP validé (rang 3 ou 6 uniquement,
  `state.rs:494-501`) — empêche la corruption `to±8`.
- **Approche make/is_in_check/unmake** : standard et correcte pour la légalité
  (clouages gérés implicitement).

⚠️ **Non revérifié dynamiquement** : la documentation indique perft 6/6 PASS.
Je n'ai pas pu rejouer perft. Si un seul chiffre perft diverge, c'est le signal
d'un bug de génération — à relancer après toute modification de `moves/`.

---

## 3. Crashs / panics — revue exhaustive

**`unwrap()` / `expect()` / `panic!`** : tous hors moteur livré, sauf gardes
d'initialisation inoffensives —
- tests `#[cfg(test)]` et binaires de dev (`tuner`, `extract_positions`,
  `benchmark`) : sans incidence sur le moteur UCI ;
- `magic.rs` / `bitboard.rs` `expect("init… non appelée")` : ne se déclenchent
  que si les tables ne sont pas initialisées — elles le sont au démarrage ;
- `magic.rs:231` `panic!` : seul panic autorisé (bug de masque irrécupérable),
  documenté.

**Parsing FEN** (`from_fen`, `state.rs:314`) : **robuste**. Retourne `Err` sur
tout champ invalide, accès bornés (`parts.len() > 4` avant `parts[4]`),
nombre de rois validé, bornes case/rang vérifiées. Pas de panic possible sur
FEN malformée.

**Parsing UCI** (`parser.rs`) : robuste — toutes les valeurs numériques via
`.parse().ok()`, indices bornés, tokens inconnus ignorés (conforme UCI). Seule
exception : le découpage par octets du §1.

**Récursion** : bornée. Extensions (échec / singulière) gardées par
`ply + 1 < MAX_PLY` (`alphabeta.rs:1130,1132`) ; quiescence bornée par
`MAX_QUIESCENCE_PLY = 256`. Pas de récursion infinie / débordement de pile par
ce biais.

**Indexation de tableaux** : sûre.
- `killers` / `eval_history` (taille `MAX_PLY`) : gardés `if ply >= MAX_PLY`.
- `scores` / `lmp_pruned` : taille `MAX_MOVES = 218` = maximum légal théorique.
- `gains` de SEE : `[i32; 32]`, boucle `while depth < 32` → pas de hors-bornes
  (un échange réel dépasse rarement ~16 pièces ; le plafond 32 ne tronque rien
  en pratique).
- Table LMR indexée avec `.min(63)` ; `depth as usize` borné `.min(63)`.

**Arithmétique entière** : `depth` et `ply` sont des `i32` **signés** avec
`.max(0)` / `.max(1)` aux points sensibles → pas de sous-débordement non signé.
`movestogo` protégé `.max(1)`, `Hash` borné `[1, 512]`, `Threads` borné
`[1, 768]`. Décrément de `piece_count` (u8) protégé par invariant +
`debug_assert!`.

---

## 4. Points à surveiller (faible sévérité, pas des bugs)

- **Pile des threads secondaires** : `std::thread::spawn` utilise la pile par
  défaut (~2 Mo). À profondeur ~192 plies, chaque trame d'`alpha_beta` porte
  `scores`/`lmp_pruned` (~1,1 Ko) plus la quiescence. Estimation worst-case
  ~0,5-0,7 Mo : ça tient, mais sans grande marge. Par sécurité, envisager
  `thread::Builder::new().stack_size(8 * 1024 * 1024)` pour les threads SMP.
- **Mémoire à `Threads` élevé** : chaque thread alloue une
  `ContinuationHistoryTable` (~576 Kio) + un clone du plateau. À 768 threads,
  ~0,5 Go rien que pour ces tables. Contrôlé par l'utilisateur, pas un crash,
  mais à documenter comme limite pratique.

---

## 5. Features désactivées — RÉACTIVÉES le 2026-06-23 (mise à jour)

> Ces deux features étaient désactivées au moment de l'audit. Elles ont été
> **réimplémentées correctement le même jour** (voir l'encart "Réactivation
> correcte des deux features" dans `CLAUDE.md`). Aucune des deux n'était un
> problème de stabilité/légalité — ce sont des bugs de FORCE de jeu.

- **Drapeau « improving »** : désormais ré-activé. Le bug d'origine
  (`eval_history[ply]` écrit seulement hors échec → données périmées d'autres
  branches lues par un descendant) est corrigé en écrivant `eval_history[ply]`
  inconditionnellement à chaque visite réelle (sentinelle en échec), sauf en
  recherche Singular Extension. L'invariant rend `eval_history[ply-2]` fiable.
- **Échec en quiescence** : désormais géré correctement (évasions complètes,
  pas de stand-pat, détection du mat) — uniquement quand le camp au trait
  subit un échec, donc à coût maîtrisé. La génération de contre-échecs
  silencieux (la partie coûteuse de l'ancienne tentative) reste non faite.

**À valider sur Mac** (non compilable dans l'environnement d'audit) :
`cargo build --release && cargo test`, puis test de la position FEN
`rn2nrk1/2q3pp/b1pbpp1B/p2pN3/1P1P4/2NBP1Q1/1PPK1PPP/R6R w - - 0 14`.

---

## Recommandations, par priorité

1. **Corriger le §1** (panic non-ASCII) — petit patch, supprime le seul
   vecteur de crash confirmé. Ajouter un test :
   `assert!(parse_move_uci("🙂e4", &mut board).is_none())`.
2. **Rejouer perft 6/6 et `cargo test` sur Mac** — pour confirmer
   dynamiquement l'absence de bug de génération (non vérifiable ici).
3. *(optionnel)* Fixer une `stack_size` explicite sur les threads SMP.
4. *(optionnel)* Documenter la limite mémoire pratique de l'option `Threads`.
