# Audit — robustesse, crash, propreté, bugs (Vendetta Chess Motor)

**Date :** 2026-06-26
**Périmètre :** code récent en priorité (Correction History reworkée, King safety par
attaque, parallélisme du binaire `selfplay`, regroupement des interrupteurs runtime,
renommage du projet), puis balayage des sources de crash classiques sur tout le crate.
**Verdict global :** code sain. **Aucun bug ni crash critique** trouvé dans le cœur du
moteur. Trois points mineurs relevés — **tous corrigés** (voir § Corrections appliquées).

---

## 1. Crash / panics

Le cœur (`board` / `search` / `eval` / `uci` / `moves`) ne contient **aucun
`unwrap`/`expect` en chemin chaud**. Les seuls panics possibles :

- **Gardes d'initialisation volontaires** (au démarrage uniquement) :
  `bitboard.rs:192/202`, `magic.rs:360/374` (« init non appelée »), et le `panic!`
  diagnostique de `magic.rs:231`. Conforme à la règle du projet « `debug_assert!`
  uniquement, jamais de panic en production ».
- **`uci/mod.rs:469`** : `.expect()` au spawn du thread de recherche → panic si l'OS
  refuse de créer le thread (épuisement de ressources, très rare). C'est le **seul**
  panic possible en runtime côté moteur. Durcissement possible (non bloquant) :
  repli sur une recherche mono-thread au lieu d'`expect`.
- **`selfplay.rs`** : `.expect()` au spawn des ouvriers (binaire de test, acceptable).

Les autres `unwrap`/`expect` du dépôt sont dans des tests `#[cfg(test)]` ou des outils
hors-ligne (`tuner`, `extract_positions`, `benchmark`) — hors moteur.

---

## 2. Robustesse

- **`selfplay` — `concurrency` non borné** *(corrigé)* : la clé était lue du config
  sans borne haute ; une valeur absurde (ex. 5000) aurait spawné des milliers de
  threads (pile 8 Mio + 2 TT chacun) → OOM/panic.
- **Invariant « roi toujours présent »** : `king_zone` (mobility.rs) et `king_square`
  font `1u64 << ksq` / `lsb(bb)`. Si `bb == 0`, `lsb` renvoie 64 → `1 << 64` = UB en
  release. **Impossible en échecs légaux** (le roi n'est jamais capturé), et c'est le
  **même pattern que le king_safety pré-existant** — donc pas une régression, purement
  théorique, couvert par le `debug_assert` de `king_square` en debug. *Laissé tel quel.*

---

## 3. Bugs (logique) — aucun trouvé

Vérifications explicites menées sur le code récent :

- **Correction History** : toutes les indexations bornées dans `[0, 2·SIZE−1]` (pion /
  non-pion) et `[0, 2·CONT_SIZE−1]` (continuation) ; pas de division par zéro
  (`wtot ≥ 4`) ; pas d'overflow `i32` (cibles et poids bornés).
- **King attack** : orientation correcte du différentiel (perspective camp au trait,
  négation si Noirs au trait) ; `units²` borné (pas d'overflow) ; gating finale cohérent
  avec le terme de bouclier (les deux valent 0 en finale) ; pas de double comptage
  (bouclier vs attaque mesurent des choses différentes).
- **Parallélisme `selfplay`** : pas de course de données — compteurs en `AtomicU64`,
  état global du moteur en `OnceLock` (lecture seule après init) ou `thread_local`
  (cache de pions, un par thread), TT par camp (non partagée entre parties).
- **`unsafe` du TT / prefetch** : correctement justifiés (préchargement matériel non
  fautif, pointeur de slot toujours dans les bornes).

---

## 4. Propreté

- **Commentaire obsolète** *(corrigé)* : l'en-tête de `selfplay.rs` référençait un doc
  inexistant ; pointe désormais sur `COMMENT_TESTER_SPRT.md`.
- **Spam d'affichage du `selfplay`** *(corrigé)* : la boucle de scrutation réimprimait
  une ligne identique quand aucune partie ne se terminait entre deux relevés.
- Reste : commentaires à jour, pas de code mort détecté.

---

## 5. Corrections appliquées (2026-06-26)

Toutes dans `src/bin/selfplay.rs`, **sans impact sur la logique de jeu ni les résultats** :

1. `concurrency` borné via `MAX_CONCURRENCY = 64` (`clamp(1, 64)`).
2. Commentaire d'en-tête corrigé (`COMMENT_TESTER_SPRT.md`).
3. Anti-spam : affichage/autosave uniquement quand le compteur de parties avance
   (`last_reported`).

---

## 6. Limite de l'audit — vérification restante

Cet audit a été mené **par revue de code statique uniquement** : l'environnement ne
permettait pas de compiler (toolchain Rust absente). Une **erreur de compilation ne peut
donc pas être exclue**. Vérification à exécuter sur la machine cible (Mac M2 Pro) :

```bash
cargo build --release --target aarch64-apple-darwin
cargo test  --target aarch64-apple-darwin -- --include-ignored
# + perft 6 sur 2-3 positions de référence (validation de la génération de coups)
```
