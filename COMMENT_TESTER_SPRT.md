# Tester une modif avec le SPRT self-play (binaire maison)

Mesure objectivement si une modification ajoute de l'Elo, en faisant jouer le
moteur contre lui-même (deux variantes A et B). **Zéro dépendance externe.**

- **A** = référence, **B** = candidat.
- **PASS** = B est plus fort → on garde la modif. **FAIL** = pas de gain → on retire.

---

## 1. Lancer (premier run rapide)

Depuis le dossier `vendetta_chess_motor/` :

```bash
cargo run --release --bin selfplay
```

Il lit `selfplay_config.txt` (déjà fourni, réglé rapide). Tu verras une ligne de
progression rafraîchie régulièrement :

```
[ 12.0%]  120/1000 parties  |  B 41-44-35 (G-N-P)  |  Elo +14.2 ±31.0  |  LLR 0.31/2.94 → PASS (10%)
```

- `12.0%` = avancement vers le **plafond** de parties.
- `LLR 0.31/2.94 → PASS (10%)` = avancement vers le **verdict** (ici 10 % du chemin vers PASS).

Il s'arrête seul dès qu'il conclut (PASS/FAIL) ou au plafond, et écrit
`rapport_selfplay.txt`.

> Tu peux passer un autre fichier de config :
> `cargo run --release --bin selfplay -- mon_test.txt`

---

## 2. Arrêter proprement

Dans un **autre** terminal (dans le même dossier) :

```bash
touch STOP
```

Le programme finit la partie en cours, écrit un rapport final marqué
`statut = INTERROMPU`, supprime `STOP`, et se termine. (Et de toute façon le
rapport est **autosauvegardé** régulièrement : même un Ctrl-C brutal ne perd
quasiment rien.)

---

## 3. Lire le verdict

Ouvre `rapport_selfplay.txt`. Champs clés :

```
statut    = SPRT_CONCLU_PASS   (ou _FAIL, ou INTERROMPU, ou PLAFOND_ATTEINT)
verdict   = garder la modif (B est plus fort)
elo_estime = +12.4
llr        = 2.97
```

- `SPRT_CONCLU_PASS` → **garde la modif**.
- `SPRT_CONCLU_FAIL` → **retire-la**.
- `INTERROMPU` / `PLAFOND_ATTEINT` → résultat **partiel** : regarde `elo_estime`
  pour la tendance, mais relance (plus de parties / plus de nœuds) pour trancher.

---

## 4. AVANT de croire un vrai test : valider l'arbitre (2 contrôles)

C'est l'étape pro à ne pas sauter. Dans `selfplay_config.txt` :

1. **A contre A** — mets `improving_a = true` (identique à B). Relance.
   → doit donner **Elo ~0** et **FAIL**. Sinon il y a un biais à corriger.
2. **A contre version affaiblie** — remets `improving_a = false`, mais mets
   `nodes_b = 3000`. Relance.
   → doit donner **Elo nettement négatif** et **FAIL**.

Si ces deux contrôles passent, l'arbitre est fiable.

---

## 5. Affiner (après le premier run rapide)

Pour un verdict solide, augmente le réalisme dans le config :
- `nodes_a` / `nodes_b` plus grands (ex. 50000) → recherche plus profonde, plus
  représentative (mais parties plus longues) ;
- `games_max` plus grand (ex. 20000) → laisse le SPRT conclure sur les modifs
  marginales.

## 5 bis. Parallélisme (gros runs plus rapides)

Le binaire joue plusieurs parties **en même temps**. Réglé par `concurrency`
dans le config :

```
concurrency = 4
```

Chaque partie lance 2 recherches mono-thread, donc `concurrency = 4` occupe
~8 cœurs (bon défaut sur le M2 Pro, 10 cœurs). Met `1` pour du séquentiel
strict. Ça ne change RIEN au verdict (les parties restent indépendantes), juste
la vitesse : ~4× plus rapide à `concurrency = 4`. C'est ce qui rend un run de
20–30 k parties faisable en un temps raisonnable.

## Tester d'AUTRES features plus tard

Pour l'instant le binaire ne sait basculer que `improving` (runtime). Pour tester
futility / LMR enrichie / correction history, il faudra rendre chacune réglable
à l'exécution (un champ comme `disable_improving`), au cas par cas — dis-le-moi
le moment venu.
