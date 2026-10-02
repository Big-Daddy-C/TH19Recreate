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

Right now the `th19` binary is an **ANM viewer** used to validate the pipeline end to end:

| Key | Action |
| --- | --- |
| ← / → | previous / next `.anm` file |
| ↑ / ↓ | step through individual scripts |
| A | toggle "all scripts at once" |
| C | toggle spawning at screen centre vs. (0,0) |
| 0–9 | send interrupt N to every VM |
| Space / `.` | pause / single-step a frame |
| R / Esc | restart / quit |

Other tools:

```sh
cargo run --release -p th-extract -- th19.dat list               # list archive contents
cargo run --release -p th-extract -- th19.dat extract data/raw   # dump everything (local only!)
cargo run --release -p th-extract -- th19.dat anm-info           # parse every .anm, summary
cargo run --release -p th-extract -- th19.dat anm-dump title.anm # entries / sprites / bytecode
cargo run --release -p th-anm --example anm_smoke -- th19.dat    # run every ANM script headless
cargo test
```

## Layout

| Crate | Purpose |
| --- | --- |
| `th-formats` | Pure parsers: THA1 archive (`dat`), ANM v8 (`anm`). ECL/STD/MSG/SHT/thbgm.fmt next. |
| `th-anm` | ANM virtual machine + interpolators; emits renderer-agnostic triangle batches. |
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
| STD (3D stage backgrounds) | ⬜ |
| ECL VM (enemies, bullets, spell cards) | ⬜ |
| Player / shot types (`plXX.sht`, `plXX.ecl`) | ⬜ |
| Bullet manager, lasers, items, collision | ⬜ |
| MSG dialogue, menus, story/VS flow, CPU opponent | ⬜ |
| Audio: `thbgm.dat` looping BGM (via `thbgm.fmt`), SFX `.wav` | ⬜ |
| Replays / netplay determinism (exact RNG) | ⬜ |

## Roadmap

1. **Formats** — ECL (`SCPT`), STD, MSG, SHT, `thbgm.fmt`, with round-trip/dump tests against real files.
2. **Stage backgrounds** — STD VM + 3D camera, so `world01..17` render behind the field.
3. **ECL VM** — sub/thread model, enemy/bullet instructions, `default.ecl` / `common.ecl` / `waveNN.ecl`.
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
