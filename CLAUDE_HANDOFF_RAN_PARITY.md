# Claude handoff — Ran corrections and source-game parity audit

Prepared 2026-10-03. This is the user's new task instruction for the next Claude session.

## Replace the previous task context

Ignore your previous conversation context, assumptions, and task instructions. Use this handoff as the new user work order. Continue to obey applicable system instructions and this repository's rules. The older `CLAUDE_HANDOFF.md` is historical and is not the current backlog; inspect actual files before treating any of its statements as current.

Work in `C:\Users\wante\th19-rs`. Reconstruct Touhou 19 v1.10c in Rust with parity to the owner's original game. **No guessing. Verify behavior in the original assets, executable, and decompile before implementing a fix.** User observations identify symptoms; they do not establish exact constants or a diagnosis.

## Current state to preserve

HEAD is `44f033c` (`Complete Marisa line lasers and source-backed laser cancellation`). Important implementation changes are **uncommitted**. Read `git status` and the diff first; do not reset, clean, overwrite, or revert this working tree.

The runtime uses wgpu/winit. Four selectable characters exist: Reimu=1, Marisa=2, **Sanae=3, Ran=4**. Sanae/Ran load their original SHT/ANM/ECL data. Their shots, scopes, charge skills, bombs, specials, EX attacks, and boss programs are implemented, but complete parity has NOT been established.

Latest verification: `cargo test --workspace` passed **105 tests**, release viewer build passed, and `python tools/research.py check` passed. Tests exercised Sanae/Ran EX and boss programs at all eight ranks and checked boss HP. These checks prove execution and selected values, not full trajectories or original-game parity.

Other recent working-tree fixes include combo expiry/flash, persistent Marisa player lasers, full-height Marisa scope/bomb, scoped-bullet tint, and Shift+K player AI. Preserve them. Meter gain/resource economy remains deferred; keep free charging practice and two default hearts.

## Priority 1 — Ran's options during slow movement

User observation: **Ran's side orbs linger in place during slow mode and return to her when slow mode ends.** Implement this from the native code, including the transitions.

Distinguish Shift slow movement from X charge/scope. Do not freeze options whenever a scope exists unless the original condition says so.

Current suspect: `Player::update_options` in `crates/th-game/src/player.rs` always moves every option toward a player-relative SHT offset, closing 30% of the distance per tick. It does this for every character. Ran's character-specific option update/position latch may be missing. This is a lead, not a confirmed source rule.

Trace the character update dispatch, option callbacks, SHT option layout, and native movement/focus flags. The player module references native option function `00546d10`; use it and its callers as starting points. Verify:

- When positions are captured, and whether the orbs remain at absolute field positions or follow another anchor.
- Whether pressing Shift before moving differs from pressing it while moving.
- Return timing and interpolation on release, without invented speeds.
- Shot origins while options are held, not just the visuals.
- Ran's C2 power-400 four-option boost: creation, latching, expiry after 600 ticks, and restoration to power 200.
- Both fields, screen boundaries, respawn and character changes; preserve other characters' option behavior.

Add a meaningful state-transition/shot-origin regression and compare the visible behavior against the original where possible.

## Priority 2 — Ran's special/EX attack positions

User observation: **Ran's special bullets, including her EX attack, should appear at the edges of the opponent's screen; currently many appear in the middle.** Verify exact targeting and routing in the original. Do not simply clamp every spawn to an edge or reuse an unrelated edge table.

Separate these paths before editing:

1. Automatic character-specific special sends: shared dispatch in `crates/th-game/src/side.rs`, arrivals/flight in `reflect.rs`, Ran native update `00419c00`.
2. Charge-tier EX requests and execution of Ran's original `pl03.ecl` programs.
3. ECL **1037**, currently handled in `enemy.rs`, which creates cross-field talismans via `Pending::new_character`.

**Concrete suspect to re-audit:** the current 1037 handler discards arg0 and uses `target: Some([0.0, arg1])`. Previous research read native case 0xbe at `00505116` as writing both float argument results into request+0x10, leaving constructor target X at zero. This apparent interpretation conflicts with the user's observed center-spawn problem. Recheck instruction boundaries, calling conventions, stack layout, constructor defaults, request consumers, coordinate conversion, and delayed arrival. Do NOT regard the previous interpretation as settled, and do NOT replace it with presumed XY semantics without executable evidence.

Native enemy opcode table: byte map **0x505560**, indexed by opcode minus 300; case address table **0x50524c**, four bytes per entry. ECL1037 maps to case0xbe. The older 0x505434 table claim is wrong.

Existing retained automatic-special finding: `00419c00` cycles 11 edge targets; steps 0/3/7 top, 1/6/9 left, 2/5/10 right, 4/8 bottom. Inspect the function/table for exact coordinates and verify whether it applies to the particular attack the user is reporting. Ran's `ExtraAttack` program sends talisman enemies; do not add a speculative call to an unused subroutine just to make bullets appear.

Check sender/receiver ownership, coordinate mirroring, rank/difficulty, warning/flight visuals, arrival positions, and all eight EX/boss attack ranks. Keep script-defined randomness and distinguish its current RNG approximation from deterministic targeting bugs.

## Priority 3 — Add the original graze effect

User request: **being near a bullet grazes it; add the graze effect using the game files.** Audit existing detection before adding anything: graze is partially implemented already.

- `side.rs` updates ordinary bullets, sets `Bullet::grazed`, queues graze positions, and awards graze/orbs. Existing comments cite `00549e00` and `00549fb0`.
- `bullet.rs` contains the one-graze-per-bullet state.
- `laser.rs` and `Side::update_lasers` contain separate laser contact/graze cadence; comments cite `00549570`, `0052b550`, `0052b250`, and an eight-tick award rule.

These are starting points to verify, not universal confirmed rules. Trace the native graze callback and its callers into ANM spawning and sound playback. Find the exact effect file/script, position, orientation, scale, color, timing, lifetime, and any rate limit. Determine whether the missing part is a visual effect, sound, collision logic, reward, or several of these.

Verify player-state eligibility, shield/invulnerability behavior, hit-vs-graze precedence, bullets already cancelled/impacting, ordinary bullets versus specials/white dots, and straight/curved lasers. Avoid double awards or a fake effect every frame. Preserve the deferred meter economy; do not make meter implementation a prerequisite for graze presentation.

## Priority 4 — Check for other mistakes

Audit the new Sanae/Ran implementation and the shared systems it exercises. Fix source-proven mistakes and record remaining gaps. Focus on shot origins and hit cadence, focus slowdown/tint, options, scope geometry, skills, bomb shapes/damage/cancellation, special routing, EX patterns and boss health/transitions. Regression-check Reimu and Marisa when changing shared code.

Known gaps to investigate or report honestly:

- Shared boss movement and replay RNG remain approximate.
- ECL516 sound, 524 spell metadata, 527 spell score and 534 presentation are not implemented.
- Curved lasers lack some endpoint effects, type-specific ANM setup, non-angle-acceleration EX handlers and exact split presentation.
- ECL1041 suppresses kill credit; its full explosion/chain consequences still need audit.
- Bot avoidance does not yet cover lasers.

Do not turn this into a speculative wholesale rewrite. Complete the requested fixes in coherent increments and clearly distinguish corrected behavior, measured parity, unresolved discrepancies, and unrun comparisons.

## Source references and efficient workflow

Read `AGENTS.md`, `research/WORKFLOW.md`, and relevant `research/catalog.json` topics, especially `sanae_ran`. Record new evidence and corrections there, including any superseded findings.

| Reference | Location |
| --- | --- |
| Original game | `C:\Users\wante\Downloads\th19\th19` |
| Binary/assets | `th19.exe`, `th19.dat`, `thbgm.dat` in that folder |
| Full Ghidra export | `C:\Users\wante\ghidra-export\decompiled.c` |
| Canonical normalized functions/index | `C:\Users\wante\ghidra-export-normalized-v4` |
| gamedb CLI | `C:\Users\wante\tools\gamedb\target\release\gamedb.exe` |
| Private extracted files | Repo `data\shot-audit-archive` |
| User screenshots | `C:\Users\wante\OneDrive\Pictures\Screenshots` (some unrelated) |
| Latest private captures | Repo `data\reference\sanae-extra-next.png`, `ran-boss-next.png`, `sanae-scope-next.png`, `ran-scope-next.png` |

From the repo root:

```powershell
python tools/research.py topic
python tools/research.py topic sanae_ran
python tools/research.py check
python tools/research.py show ran_ex_tick ran_skill_start --limit 90
python tools/research.py show 00549fb0 00546d10 --limit 90
python tools/research.py asm 00505116 --count 60
```

Batch bounded source lookups. Follow specific unresolved branches. Use disassembly for missing functions, hidden arguments, and suspicious decompiler types; raw C output is not infallible. Verify address mappings against the executable fingerprint. A missing Ghidra export does not prove a function is absent. The 7,769 exported-artifact parity count does not prove complete executable discovery.

**Shared ANM fixes to preserve:** original Ran scope script11 spawns four script12 arms, copying different parent variable10004 angles at each spawn through opcode509. Native child construction executes immediately (`004d0770`). ANM318 scales child position by root-parent scale (`00404050`), and primitive603/606 rotate independently of sprite render mode (`004c4d20`). Current regression checks scope extents +/-105 on both axes at scale30. Do not undo these source-backed fixes to compensate for a targeting bug.

## Validation and delivery

Run appropriate regressions, then `cargo test --workspace`, `cargo build --release -p th19`, `python tools/research.py check`, and `git diff --check`. Capture fixed encounters with reproducible inputs/ticks. When comparing to the original, match version, character, difficulty, rank, timing, position and relevant settings. Current tests and screenshots alone are not proof of parity.

Preserve original assets and private decompiled output outside Git. Do not modify the original game installation, overwrite a research database irreversibly, discard local changes, or push/publish without authorization. Routine local implementation/build/test work is authorized. Keep the historical handoff intact.

Report each requested issue with source addresses/files, the actual corrected rule, tests/comparison outcome, and any unresolved limits. Give a runnable PowerShell command. Do not claim everything matches simply because the build succeeds.

Example current launch, Ran versus Sanae:

```powershell
cd C:\Users\wante\th19-rs; .\target\release\th19.exe --game 1 --p1 4 --p2 3 --full-charge
```

Controls: arrows move, Z shoot, Shift slow/focus, X hold/release charge, C bomb, B developer clear, 1–4 instant charge tiers, Shift+K player AI, K opponent AI. Shift+[ / Shift+] changes player character; [ / ] changes opponent. Do not confuse Shift movement focus with X-held scope.
