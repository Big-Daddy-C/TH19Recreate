# Touhou 19 Rust reconstruction — Claude handoff

Prepared 2026-10-03. Continue implementation in this repository; the user considers the latest visible result good and has requested the next fixes below. Preserve the working baseline.

## Start here

- Repository: `C:\Users\wante\th19-rs`.
- Baseline commit: `e40cbd0` — `Add source-driven spirit attacks, scope slowdown and shot impacts`. Working tree was clean before this handoff was added.
- Read `AGENTS.md`, `research/WORKFLOW.md`, and the relevant topics in `research/catalog.json` before researching from scratch.
- Goal: reconstruct **Touhou 19 v1.10c in Rust**, using the owner's original executable and assets as the authority. Current renderer is **wgpu/winit**, not Bevy. An engine migration is a separate decision.
- The user repeatedly instructs: **do not guess; consult the game files and decompile**. Observations describe problems, not permission to invent numeric fixes. Trace behavior, implement it, and retain the evidence.
- Routine local edits, builds, tests, and continued implementation are authorized. Do not modify the original installation, commit original assets/decompiled C, or push/publish. Ask before irreversibly replacing an existing research database.
- Meter gain and normal resource accounting remain deferred. Preserve free practice charging. Default life is two hearts.

## Requested next work

### 1. Correct spirit orientation

User observation: spirit textures appear rotated **clockwise by 90 degrees**. Orient them to match the original. Earlier screenshots in `C:\Users\wante\OneDrive\Pictures\Screenshots` should contain examples; inspect them rather than assuming every image is relevant.

Check all four types (pure, wolf, otter, eagle), their active and quelled scripts, and movement-direction updates. Compare `ghost.anm`, `default.ecl`, fixed/directional enemy animation setup, and the native owner-to-ANM rotation path. Do not blindly subtract 90 degrees from every sprite or undo the source-backed child rotation inheritance added for shot trails.

Relevant code: `crates/th-game/src/spirit.rs`, `enemy.rs`, `side.rs`, and `crates/th-anm/src/vm.rs`. ECL 303 slot 0 currently creates a fixed main animation; 306 uses directional animation. Investigate the actual failing script before changing either rule.

### 2. Restore correct charge damage and hit cadence

User observation: Reimu's charge attack should destroy a spirit almost instantly after focusing/scoping it, but currently takes too long. Verify what happens after the scope quells the spirit; do not just multiply damage until it looks strong.

Research SHT damage, native collider repeat-hit/reset behavior, charge hit callback 7, piercing, target eligibility, and the spirit's quelled HP. There is a concrete suspect in `player.rs`: charge shots skip a target when `b.last_enemy == Some(d.id)`. Determine the original lifetime/reset semantics of that exclusion. This is a lead, not a confirmed diagnosis.

- Reimu charge slot 10 uses script 10, initial speed 14, and callbacks `[11, 8, 0, 7]`; the currently read damage is 7 per orb.
- Native collider: `00520450`. Charge hit callback: `0054b370`; charge speed callback: `0054b380`. These callbacks can be absent from the normalized export: use verified executable disassembly.
- Quelled spirit HP is set by original ECL: pure `50 + 10*I0`; wolf `200 + 30*I0`; otter/eagle `150 + 30*I0`.
- Distinguish Shift focus from the X-held scope. The user's phrase “focus moding a spirit” likely concerns quelling, but verify the runtime state rather than treating Shift as an automatic scope.

Add a meaningful hit-cadence regression check and compare a fixed encounter to the original. Preserve ordinary shot impacts, piercing, boss damage scaling, and invulnerability rules.

### 3. Complete chain-explosion bullet cancellation

User request: death explosions/chains must destroy eligible enemy bullets, with original immunity rules. Some bullets, such as certain EX attacks, survive. Verify the exact categories and flags; do not make every bullet disappear.

This mechanic is **partially implemented**, not entirely absent:

- `Side::update_explosions` currently deletes bullets only when `state == 1`, `kind` is in `0..4`, and the center lies within `radius + 1`. This branch does not explicitly check bullet invulnerability.
- ECL-triggered `st.cancel_bullets` currently drains the whole bullet list. Audit it separately; do not conflate script cancellation, chain cancellation, skill clearing, and bombs.
- `Bullet::intersects_cancel` has a separate eligibility/intersection path. Trace why these paths differ before consolidating them.

Start with `0051cc70`, `0051c8c0`, `0051d4e0`, `004ee770`, `00520450`, and bullet type setup `004f66a0`. Confirm overlap geometry, state/flag eligibility, cancel animation, combo contribution, and any opponent-side sends caused by cancellation. Check immunity independently for each clearing mechanism.

Test eligible small bullets, immune attacks, overlap boundaries, chained kills, and both fields. Update the evidence catalog with a concrete eligibility table derived from the executable/data.

### 4. Identify and implement missing white/special bullets

User reference: [Gameplay — Reflecting White Dot Bullets](https://en.touhouwiki.net/wiki/Unfinished_Dream_of_All_Living_Ghost/Gameplay#Reflecting_White_Dot_Bullets).

The user believes “special bullets” are character-specific attacks sent by the opponent, and says pure spirits were just implemented while white and special bullets remain missing. Treat this terminology as a question to resolve against the original code and reference, not an established mapping.

**Reimu EX attacks already exist**, including original `pl00.ecl` execution and routing to the opposing field through `dispatch_charges`. Determine whether the missing feature is a separate special-bullet category, automatic sending triggered by chains, reflecting white dots, or an incomplete aspect of existing EX attacks. Do not duplicate the current EX implementation.

Trace bullet creation, conversion/reflection, opponent transfers, ranking, speed, appearance, collision, scope interaction, and cancellation immunity. Pure spirits are enemies; do not implement them again as ordinary bullets.

The linked Gameplay page could not be fetched during handoff preparation (access blocked). It is retained as the user's reference, not as verified numeric evidence. A searchable secondary [translation of the game's instructions](https://en.touhouwiki.net/wiki/Unfinished_Dream_of_All_Living_Ghost/Translation) describes explosions erasing small bullets and sending attacks to the other field; verify exact rules in the original files. The executable and bundled scripts remain authoritative.

### 5. Add a bomb / spirit-strike feature if feasible

The user explicitly authorizes a simplified practice economy:

- Bind **C** to bomb.
- Move the current clear-fields command from **C** to **B**.
- Treat available meter as **maximum whenever a bomb is used**.
- Defer bomb consumption/limited uses and meter gain/accounting.

Derive the actual maximum-meter bomb behavior from the original: damage, coverage, timing, visuals, bullet immunity/cancellation, player protection, and effects on enemies/spirits. The economy simplification does not authorize invented attack behavior. Check whether the original calls this a spirit strike and how it differs from Reimu's skill/charge attacks.

**Key conflict:** B currently calls `debug_extra_attack()` in game mode. Remove or rebind that debug action when moving clear to B, and update help/README. Instant tier keys 1–4 remain available. Avoid triggering a bomb repeatedly through keyboard repeat. Preserve C's separate center-origin behavior in the ANM viewer if appropriate.

These are the next requested features; do not make meter gain a prerequisite. Implement and verify coherent increments rather than changing all systems at once.

## Working baseline to preserve

- Shared scrolling stage background with two gameplay fields over it; do not render independent stage instances for each player.
- Reimu movement, regular and homing shots, all four free-practice charge tiers, scope, shield/life handling, skill/EX/boss routing, enemy waves and bullets.
- Source-backed homing steering/acceleration, original shot textures, and ANM-driven bullet-hit effects. Impacting shots stop damaging enemies and persist until their impact script finishes.
- Boss HP checks across eight levels, rotating/tilted boss circle, and scene effects. Complete boss pattern parity has not been demonstrated.
- Four spirit types driven by `default.ecl`: `GhostPure`, `GhostWolf`, `GhostOtterR/L`, `GhostEagle`. Cross-field warning animations, delayed spawns, scope quelling, and death-triggered animal selection exist.
- Scope slows ordinary enemies and live enemy bullets using the traced 0.5 movement factor. Spirit contact dispatches its disable subroutine instead of applying generic enemy drag. Animation/script time is not indiscriminately halved.

Recent shot facts: script 6 uses rectangular hit sprites 26–28; script 7 uses sprite 29; homing script 8 uses impact sprites 35–37. The original homing sprite 34 is blue/white with red detail; its trail is pink/red. Do not recolor assets based only on recollection. ANM opcode 504 now inherits parent XYZ rotation before the child script runs, following `004d09b0`.

## Original sources and retained evidence

| Item | Location |
| --- | --- |
| Original installation | `C:\Users\wante\Downloads\th19\th19` |
| Executable/assets | `th19.exe`, `th19.dat`, `thbgm.dat` in that directory |
| Full Ghidra export | `C:\Users\wante\ghidra-export\decompiled.c` |
| Canonical normalized export/index | `C:\Users\wante\ghidra-export-normalized-v4` (`functions\...`, `.gamedb`) |
| gamedb executable | `C:\Users\wante\tools\gamedb\target\release\gamedb.exe` |
| Retained research | `research/catalog.json`, `research/WORKFLOW.md`, `tools/research.py` |
| Private extracted archive | `data/shot-audit-archive` |
| Private latest capture | `data/reference/spirits-and-impacts.png` |
| User screenshots | `C:\Users\wante\OneDrive\Pictures\Screenshots` |

Original executable SHA256: `8918fb39d511c9906ef769c6dd2761e7cb44ded5b7c4c1cd99c8992d12c078e9`.

Original footage: `C:\Users\wante\OneDrive\Desktop\Videos\'Iran War didn't happen'\OBS\2026-10-02 01-35-27.mp4`. The folder contains literal apostrophes; use `Get-ChildItem -LiteralPath` / proper PowerShell quoting. Earlier footage may cover backgrounds rather than the newer gameplay features.

Useful private files include `data/ghost-dump.txt`, `data/default-ecl-dump.txt`, `data/pl00-anm-dump.txt`, `data/spirit-textures/enemy_enemy2.png.png`, and `data/player-textures/player_pl00_pl00s.png.png`; confirm availability before relying on them. Keep raw evidence under ignored `data/`.

## Efficient research commands

Run from the repository root:

```powershell
python tools/research.py check
python tools/research.py topic
python tools/research.py topic spirits
python tools/research.py topic scope
python tools/research.py topic charge
python tools/research.py topic combat
python tools/research.py topic player_shots
python tools/research.py show 0054dc40 --start 1 --limit 70
python tools/research.py asm 0054b370 --count 40
python tools/research.py callers meter_spend --limit 12
python tools/research.py search '0x2078' --limit 12
```

Batch related bounded excerpts; avoid repeatedly dumping the entire decompile. The helper validates source fingerprints and executable hashes. Never blindly refresh a mismatching hash. A missing Ghidra export is not proof a function is empty; disassemble omitted callbacks. Decompiled C can obscure ECX/XMM arguments.

Enemy ECL opcode dispatcher: `004fecc0`. Verified mapping: byte case table `00505560 + (opcode - 300)`, then case address table `0050524c + case*4`. **An older summary's `00505434` table reference was wrong.** ANM interpreter: `004bd660`.

Spirit scope contact is `00509030`. **`0052b250` is laser scope contact**, despite an older mistaken alias. ECL 521 stores the spirit disable subroutine, independently of ECL 514 HP/time interrupts. Spirit type is ECL 1027's internal category, not interchangeable with public enemy flags.

Use `th-extract` to inspect original ANM/ECL:

```powershell
.\target\release\th-extract.exe C:\Users\wante\Downloads\th19\th19\th19.dat anm-dump ghost.anm
.\target\release\th-extract.exe C:\Users\wante\Downloads\th19\th19\th19.dat ecl-dump default.ecl GhostPure
```

Check CLI usage for other commands. `extract` takes an output directory and extracts the archive; it is not a single-file output filename.

## Known limitations — do not hide these

- Spirit pressure is currently held at practice value zero; match escalation/settings and ability modifiers are incomplete. RNG is an approximation.
- ECL 318 is not implemented. Native case `004ff549` deletes/recreates the main ANM and resets movement-animation state; this matters when examining disable transitions. Trace it before substituting a no-op.
- Some enemy/boss movement, globals, presentation opcodes, lasers, items, audio, menus, story/CPU behavior, and other characters remain unfinished. The user's satisfaction with visuals does not establish that the full game is nearly complete.
- Background anchors/distortion geometry and stage 9 blur retain known approximations.
- Normalization/index parity is 7,769/7,769 **exported artifacts**, not proof every executable function was discovered or every gameplay behavior was ported.

## Validation and running

At the baseline, the preceding implementation run passed **53 workspace tests**, `cargo build --release -p th19`, and `git diff --check`. This handoff did not rerun the full test suite. `python tools/research.py check` was rerun while preparing this file: **7,769 exported files, 87 named references, nine topics; integrity PASS**. These checks do not establish original-game parity.

The latest fixed-frame capture showed a naturally earned spirit and shot impacts, but there was no matched original-game gameplay capture. Compare orientation, hit cadence and cancellation to the original before claiming parity.

```powershell
cd C:\Users\wante\th19-rs
cargo test --workspace
cargo build --release -p th19
git diff --check
python tools/research.py check
.\target\release\th19.exe --game 1
```

For the user, a single pasteable PowerShell launch command is:

```powershell
cd C:\Users\wante\th19-rs; .\target\release\th19.exe --game 1
```

Optional `--godmode`. Screenshot example:

```powershell
.\target\release\th19.exe --game 1 --godmode --screenshot data/reference/next-check.png --frames 1600
```

Build the actual executable, not only `--examples`. PowerShell 5.1 does not support `&&`.

Current controls before the requested remap: arrows move, Z shoot, Shift focus, X hold/release charge and scope; 1–4 instant charge tiers, Shift+1–4 opposite field; B debug EX, C clear fields, G godmode, Space pause, period single-step while paused, R restart, F fast-forward, PgUp/PgDn stage, Tab viewer mode.

After each implementation increment, update the catalog with exact addresses, findings, unresolved gaps, tests and comparison status. Report what changed and provide a runnable command. Keep the user's requested max-meter practice path available.
