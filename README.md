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
| Game mode | arrows = move, Z = shot, X = hold/release first charge, B = debug EX on own field, Shift = focus, PgUp/PgDn = stage (`--game N` starts here) |
| ← / → | previous / next `.anm` file, or stage |
| ↑ / ↓ | step through individual scripts |
| A | toggle "all scripts at once" |
| C | toggle spawning at screen centre vs. (0,0) |
| 0–9 | send interrupt N to every VM |
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
| Enemies + TH19 wave system (movement, directional sprites, wave pool/sequence, level tables) | 🟡 waves, enemy bullets and death chains run on both fields; earned EX/boss dispatch remains |
| Player: movement (fixed point, decoded clamps/speeds), options, shot timers + SHT records, shots vs enemies | 🟡 Reimu on both sides; hearts, barrier and first charge tier work; homing remains approximate; bombs/higher charge tiers pending |
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

The first charge attack is playable: hold **X** until the bottom charge bar turns gold
(50 normal-speed frames, about 0.83 seconds), then release. Reimu fires the four orbs from
`pl00.sht` slot 10. It costs no meter. The blue bar is available meter; the lower bar is
held charge. These bars are a provisional HUD. B retains the debug EX injection into the
left field; it is not an earned attack sent to the opponent.

Starting meter and thresholds come from `FUN_005478f0` and the original SHT. Hold/release
and cancellation use `FUN_004e79e0`; the local attack uses `FUN_0053b110` / `FUN_0053ac80`,
and the charge aura uses `aura.anm` script 29. The orbs keep their scripted rotation,
pierce enemies, slow to 0.2 then accelerate after 120 frames. Their callbacks at
`0054b370`, `0054b380`, and `0054b3e0` are present in the executable's function tables
but absent as standalone functions from the current Ghidra export; they were checked
using disassembly. Thus export/index parity does not establish complete function discovery.

Higher charge tiers are deliberately gated: meter gains, Reimu's skill callback,
earned EX/boss dispatch and EX-tier barrier recovery are still unimplemented. The first
tier does not restore a broken barrier. Charge speed currently uses the unmodified
100-percent setting; ability-card modifiers and aura gameplay interactions remain.
