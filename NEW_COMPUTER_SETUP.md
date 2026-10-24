# Setting up th19-rs on a new computer

For the AI assistant helping the owner on their new machine. Read this first, then `AGENTS.md`
(how to work on the project) and `CLAUDE_HANDOFF.md` (where work left off).

**What this is:** th19-rs is a from-scratch Rust reimplementation of Touhou 19 ~ Unfinished Dream of
All Living Ghost (v1.10c). It reads all assets from the owner's own game files at runtime. Game
files and anything extracted from them are never committed (`/data`, `*.dat` are git-ignored), and
must never be pushed or published.

**Your job:** install the software below, put the owner's copied files where the project expects
them, then build and run to confirm everything works. Ask the owner before pushing to GitHub or
changing anything outside this setup.

## 1. Software to install

| Software | Needed for | Where to get it |
|---|---|---|
| Git | getting and updating the code | https://git-scm.com/download/win |
| Rust (stable, via rustup) | building and running the engine; the old machine had rustc/cargo 1.99.0 | https://rustup.rs (accept the "Visual Studio C++ Build Tools / Desktop development with C++" install it offers; Rust needs it on Windows) |
| Python 3 (3.14 on the old machine) | `tools/research.py`, the research catalog tool described in `AGENTS.md` | https://www.python.org/downloads/ (tick "Add python.exe to PATH") |
| A GPU driver with DirectX 12 or Vulkan | the engine renders with wgpu | normally already installed |

Research tools (only needed for reverse-engineering work, not for building or playing):

| Software | Where to get it |
|---|---|
| Ghidra 12.1.4 (was at `~/tools/ghidra_12.1.4_PUBLIC`) | https://github.com/NationalSecurityAgency/ghidra/releases |
| JDK 21 for Ghidra (was at `~/tools/jdk21`) | https://adoptium.net (Temurin 21) |
| thtk, Touhou toolkit (was at `~/tools/thtk`) | https://github.com/thpatch/thtk/releases |
| gamedb, decompiled-source search tool (was at `~/tools/gamedb`) | `git clone https://github.com/smileybaal/gamedb.git` into `~/tools/gamedb`, then `cargo build --release` there; the binary is `~/tools/gamedb/target/release/gamedb.exe` |

## 2. Get the code

```powershell
cd $HOME
git clone https://github.com/Big-Daddy-C/TH19Recreate th19-rs
```

The repo is private; the first clone may open a GitHub login window.

## 3. Files the owner copies over by hand

These are not in git. The owner brings them on a drive or through OneDrive. Put each back at the
same place under the user folder (`~` = `C:\Users\<username>`):

| Item | Place it at | Required? |
|---|---|---|
| The game: `th19.dat`, `thbgm.dat`, `th19.exe`, etc. (~675 MB) | `~\Downloads\th19\th19\` | **Yes**: nothing runs without `th19.dat` / `thbgm.dat` |
| Audio settings `th19-rs-settings.txt` | `~\th19-rs\` | No (defaults are used) |
| Ghidra project `th19.gpr` + `th19.rep` (keep together) | `~\ghidra-projects\` | Research only |
| Ghidra exports `ghidra-export` and `ghidra-export-normalized-v4` (includes its `.gamedb` index, a hidden folder) | `~\` | Research only; `tools/research.py` reads `~/ghidra-export-normalized-v4/functions` by default |
| `ghidra-cli` (`ghidra.exe`) | `~\tools\ghidra-cli\` | Research only |
| Extracted assets `th19-rs\data` (~470 MB) and `th19-extracted` | `~\th19-rs\data`, `~\th19-extracted` | No: can be regenerated with `th-extract` (see README) |

Never modify the original game files.

## 4. Paths and environment variables

The project expects the layout above. If the new Windows username isn't `wante`, or files are
elsewhere, set these (User environment variables, then reopen the terminal):

| Variable | Points to | Used by |
|---|---|---|
| `TH19_DAT` | `...\th19\th19.dat` | the engine and the real-data tests |
| `TH19_DIR` | the game folder | the engine |
| `TH19_EXE` | `...\th19\th19.exe` | `tools/research.py` (default `~/Downloads/th19/th19/th19.exe`) |
| `TH19_DECOMPILE` | `...\ghidra-export-normalized-v4\functions` | `tools/research.py` |

Some code and tests fall back to the old machine's absolute path `C:\Users\wante\Downloads\th19\th19\th19.dat`
when `TH19_DAT` isn't set: `crates/th-anm/src/vm.rs`, `crates/th-game/src/beam.rs`,
`crates/th-game/src/player.rs`, `crates/th19/src/main.rs`, and tests in `crates/th-anm/tests`,
`crates/th-formats/tests`, `crates/th-game/tests`. Setting `TH19_DAT` avoids it; if the owner wants,
replace those fallbacks with a single shared default.

## 5. Check that it works

From `~\th19-rs`:

```powershell
cargo build --release
cargo test
cargo run --release -p th19 -- --dat "$HOME/Downloads/th19/th19/th19.dat"
python tools/research.py check
```

The first build downloads dependencies and takes several minutes. In the running engine, Tab cycles
the ANM viewer → stage viewer → game mode; see the README for all keys. `research.py check` should
report the executable and decompile fingerprints as matching; if they don't, the files above are
missing or in the wrong place.

Then tell the owner what works, what didn't, and anything still missing.
