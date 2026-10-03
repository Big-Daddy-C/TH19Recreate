# th19-rs

A from-scratch Rust reimplementation of **Touhou 19 ~ Unfinished Dream of All Living Ghost**
(v1.10c), in the spirit of engine reimplementations like OpenMW or the Skate 3 Rust engine.

**No game assets are included or redistributed.** The engine reads everything — sprites,
scripts, stages, music — at runtime from *your own* copy of `th19.dat` / `thbgm.dat`.
Anything extracted for debugging goes into `data/`, which is git-ignored.

## Running

```sh
cargo run --release -p th19 -- --dat "C:/path/to/th19/th19.dat"
# or set TH19_DAT / TH19_DIR
```

Right now the `th19` binary is an **ANM + stage viewer** used to validate the pipeline end to end:

| Key | Action |
| --- | --- |
| Tab | cycle ANM viewer → stage (STD) viewer → game (stage + waves) |
| Game mode | arrows = move, Z = shot, X = hold/release charge (all four tiers), B = debug EX on own field, Shift = focus, PgUp/PgDn = stage (`--game N` starts here) |
| G (game mode) | development godmode: neither player can be hit |
| 1–4 (game mode) | development: release player 1's charge instantly at tier 1 shot / 2 skill / 3 EX / 4 boss (falls back to EX while the opponent has a boss) |
| C (game mode) | development: clear bullets, enemies, bosses and attack scripts on both fields |
| ← / → | previous / next `.anm` file, or stage |
| ↑ / ↓ | step through individual scripts |
| A | toggle "all scripts at once" |
| C | toggle spawning at screen centre vs. (0,0) (viewer modes) |
| 0–9 | send interrupt N to every VM (except 1–4 in game mode) |
| Space / `.` | pause / single-step a frame |
| F | fast-forward ×8 |
| R / Esc | restart / quit |

Other tools:

```sh
cargo run --release -p th-extract -- th19.dat list               # list archive contents
cargo run --release -p th-extract -- th19.dat extract data/raw   # dump everything (local only!)
cargo run --release -p th-extract -- th19.dat anm-info           # parse every .anm, summary
cargo run --release -p th-extract -- th19.dat anm-dump title.anm # entries / sprites / bytecode
cargo run --release -p th-extract -- th19.dat ecl-info           # also: std-info, msg-info, bgm-info
cargo run --release -p th-extract -- th19.dat ecl-dump wave01.ecl  # ECL bytecode ([sub] [opcode] filters)
cargo run --release -p th-game --example side_trace -- th19.dat  # run the wave system headless
cargo run --release -p th-anm --example anm_smoke -- th19.dat    # run every ANM script headless
cargo run --release -p th-stage --example stage_smoke -- th19.dat  # run every stage headless
cargo test   # includes a real-data test that parses every file in your th19.dat (set TH19_DAT)
```

## Layout

| Crate | Purpose |
| --- | --- |
| `th-formats` | Pure parsers: THA1 archive, ANM, ECL, STD (`stage`), MSG, SHT, `thbgm.fmt` (`bgm`). |
| `th-anm` | ANM virtual machine + interpolators + 3D camera; emits renderer-agnostic triangle batches. |
| `th-stage` | STD player: camera/fog script, spawns stage quads as 3D ANM VMs. |
| `th-ecl` | ECL VM core: typed stack, variables, sub calls, threads, interpolators (opcodes 0–99). |
| `th-game` | Gameplay: enemies (movement, sprites, ECL host for opcodes 300+), the TH19 wave system, the two sides. |
| `th-extract` | CLI for inspecting / extracting your game data. |
| `th19` | The game binary: winit window, wgpu renderer, fixed 60 Hz loop. |

The simulation crates never touch the GPU, so they can be tested headless and kept
deterministic (needed for replays and netplay).

## Status

| Area | State |
| --- | --- |
| `th19.dat` (THA1: decrypt + LZSS) | ✅ all 353 files |
| ANM v8 + TH19 extensions (cropped PNG/JPEG THTX, 40-byte sprites) | ✅ all 124 files parse |
| ANM VM (vars, math, jumps, interrupts, interpolation modes 0–31, children, primitives) | 🟡 all 3,009 scripts run; 600-series TH19 draw ops, `effectNew`, 3D modes approximate |
| wgpu renderer (10 ANM blend modes, letterboxed 640×480 canvas) | 🟡 works; layers/render stages not yet modelled |
| ECL container (`SCPT`/`ECLH`, 43 files, 169 distinct opcodes) | ✅ parsed |
| STD (17 stages: objects, quads, instances, camera script) | ✅ parsed |
| MSG (58 files incl. endings; text XOR decoding) | ✅ parsed |
| SHT (19 characters; header + 120-byte shot records decoded from the player code) | ✅ parsed |
| `thbgm.fmt` (24 tracks, intro/loop points) | ✅ verified against `thbgm.dat` size |
| STD playback (camera/fog script, 3D quads, perspective + fog) | 🟡 all 17 stages run; camera shake, skybox sprites, distortion not yet |
| ECL VM core (opcodes 0–97) | ✅ checked against the decompiled interpreter |
| Enemies + TH19 wave system (movement, directional sprites, wave pool/sequence, level tables) | 🟡 waves, enemy bullets and death chains run on both fields; Reimu EX/boss dispatch works; full boss opcode coverage remains |
| Player: movement (fixed point, decoded clamps/speeds), options, shot timers + SHT records, shots vs enemies | 🟡 Reimu on both sides; hearts, barrier and all four charge tiers work in free-charge practice; homing and boss movement remain approximate |
| Bullet manager, lasers, items, collision | ⬜ |
| MSG dialogue, menus, story/VS flow, CPU opponent | ⬜ |
| Audio: `thbgm.dat` looping BGM (via `thbgm.fmt`), SFX `.wav` | ⬜ |
| Replays / netplay determinism (exact RNG) | ⬜ |

## Roadmap

1. ~~**Formats**~~ — done; all parsers are checked against every file in `th19.dat` by `cargo test`.
2. ~~**Stage backgrounds**~~ — STD player + 3D camera done; shake/skybox/distortion remain.
3. **ECL VM** — ~~core VM~~, ~~waves~~; bullet instructions (600s), bosses/charge attacks (`plXX.ecl`, `default.ecl`).
4. **Gameplay core** — playfields (two of them — TH19 is a VS shooter), player movement/focus,
   shots from `.sht`, bullet manager, hitboxes, graze, charge attacks.
5. **Game flow** — title → character select → story/VS, MSG dialogue, results.
6. **Audio** — streaming loop points from `thbgm.fmt`, SFX mixing.
7. **Parity work** — compare against the original frame by frame; exact RNG for replays.

### Reverse-engineering workflow

Start with [research/WORKFLOW.md](research/WORKFLOW.md) and
`python tools/research.py topic`. The saved feature catalog links source addresses,
implementation, existing checks and unresolved questions. `python tools/research.py check`
detects changed reference files before we reuse their findings.

Public format docs cover the file layouts and most of the opcodes, but exact behaviour
(e.g. render modes, TH19-only opcodes, ECL instruction semantics, CPU AI) has to come from
the executable. Suggested pipeline:

1. Decompile `th19.exe` with Ghidra (export C).
2. Index it with [gamedb](https://github.com/smileybaal/gamedb) to search functions / call graphs from the shell.
3. Anything implemented from approximation is tagged `APPROX` in the source so it can be audited.

## Credits

- [thtk](https://github.com/thpatch/thtk) (BSD-2-Clause) — archive crypto/LZSS and ANM layouts were ported from it.
- [ExpHP's thpages](https://exphp.github.io/thpages/) — instruction and interpolation reference (used as documentation only).
- Touhou Project © ZUN / Team Shanghai Alice. This project is unofficial and requires a legitimately owned copy of the game.

### Gameplay progress

The player starts with two hearts and a barrier. Breaking the barrier grants 100 frames
of invulnerability; losing a heart restores it. Final defeat finishes the round and awards
one point to the opponent. The title displays round points and the outcome. R resets the
viewer, including the score; full multi-round match progression remains to be implemented.

All four Reimu charge tiers are playable. Hold **X** and release at the desired bottom-bar
marker. The window title shows the ready tier and last released attack. Charging is free
and all tiers are available; natural meter gain and spending are deferred for practice.

| Tier | Hold time at 60 Hz | Release |
| --- | --- | --- |
| 1: Shot | 50 frames / 0.83 s | Existing four local charge orbs |
| 2: Skill | 100 frames / 1.67 s | Three rotating skill effects, local damage and bullet cancellation |
| 3: EX | 170 frames / 2.83 s | Restore shield and send Reimu's scripted EX attack to the opponent |
| 4: Boss | 250 frames / 4.17 s | Restore shield and summon Reimu's scripted boss on the opponent's field |

Every higher release also fires the basic local charge shot. A full release becomes EX
if the opponent already has a boss. The blue bar shows the available practice capacity;
the lower bar shows held charge. These bars and the boss HP bar are provisional. B still
injects a debug EX on your own field. It is separate from X's opponent attack.

For a reproducible local capture, use `--game 1 --preview-charge 2 --frames 183
--screenshot data/reference/charge-skill.png` (join into one command). Preview tiers 1–4
simulate holding X from frame 60; this option only applies with `--screenshot`.

Starting meter and thresholds come from `FUN_005478f0` and the original SHT. Hold/release
and cancellation use `FUN_004e79e0`; the local attack uses `FUN_0053b110` / `FUN_0053ac80`,
and the charge aura uses `aura.anm` script 29. The orbs keep their scripted rotation,
pierce enemies, slow to 0.2 then accelerate after 120 frames. Their callbacks at
`0054b370`, `0054b380`, and `0054b3e0` are present in the executable's function tables
but absent as standalone functions from the current Ghidra export; they were checked
using disassembly. Thus export/index parity does not establish complete function discovery.

The skill follows `FUN_0053b920` / `FUN_0053b3a0` and `pl00.anm` script 20. Opponent
attacks run the original `ExtraAttack` / `BossAttack` ECL entries. Asset-backed tests cover
skill lifetime/cancellation, shield recovery, dispatch in both directions, boss firing,
defeat, timeout and the active-boss fallback.

This is a playable increment, not verified full parity: exact replay RNG, boss random
movement, ghost cancellation, some boss ECL globals/opcodes and presentation effects
remain incomplete. Charge speed uses the unmodified 100-percent setting; ability-card
modifiers and aura gameplay interactions remain. See `research/catalog.json` for the
recorded gaps and comparison status.
