# Evidence and lookup workflow

The reusable part of the supplied Game-Reconstruction-Workflow-Discord outline is its
chain: behavior -> evidence -> implementation -> check -> retained result. The starter
CSV values are a synthetic teaching exercise. They are not TH19 settings and are not
runtime inputs here. This project already reads SHT/ECL/ANM/STD parameters from the game;
research/catalog.json records understanding and status rather than duplicating those
parameters as an alternate source of truth.

The Skate reference's README describes a Rust/Bevy rewrite backed by years of prior
research and working tools, with gameplay parity still in progress:
https://github.com/SK8-ENGINE/skate-3-rust-engine
This supports building reusable knowledge alongside the runtime. It does not establish
that moving this project's renderer to Bevy would make reverse engineering faster.

## Short commands (from the repository root)

```powershell
python tools/research.py topic
python tools/research.py topic charge
python tools/research.py check
python tools/research.py show charge_controller reimu_charge_start reimu_update --limit 70
python tools/research.py show charge_controller --start 220 --limit 90
python tools/research.py callers meter_spend --limit 12
python tools/research.py search '0x2078' --limit 12
python tools/research.py asm charge_shot_speed --count 24
python tools/research.py table 005a8a18 --count 12
```

Defaults resolve under the current user's home directory. Override TH19_DECOMPILE
(normalized functions directory) and TH19_EXE, or use --source / --exe before the command.
Only `asm` requires capstone; it is already installed on the current workstation. All
other commands use Python's standard library. No downloaded tutorial code is executed.

`show` accepts several aliases, FUN names or hexadecimal addresses and reports omitted
lines. A missing export is an error, not an empty success. `callers` reports literal
references in exported C, so virtual/function-pointer calls can be absent. It is not a
complete call graph. `asm` and `table` check the original executable SHA256 first.
Disassembling an arbitrary address does not establish that it is a function boundary.

## Keep the catalog small and honest

- Symbols retain original addresses and file hashes; semantic aliases are descriptive
  research labels, not recovered original names.
- Findings distinguish prior code checks, carried-over research, user observations and
  assumptions. Recheck carried-over research only where the next implementation needs it.
- Features track implementation separately from original-game comparison. Passing tests
  describe the implemented scope, not complete parity. Keep manual cases unrun until run.
- Update a feature's next steps after implementation. Record exact test command, revision,
  outcome and private capture path for a new result; do not overwrite a prior failure.
- Ask about input, timing and visible output when behavior remains ambiguous. Do not ask
  the owner to rediscover constants or repeat observations already saved here.

## Reference inventory and limits

The existing Ghidra export and canonical gamedb remain the full source reference:
`~/ghidra-export`, `~/ghidra-export-normalized-v4/.gamedb`.
The lightweight helper reads those files and never rebuilds or overwrites the database.
`fn_xrefs.tsv` currently contains address/name/reference-count rows, not caller edges.
Use gamedb or targeted source/disassembly work when actual call relationships are needed.

The previous 7,769/7,769 normalization/index parity report covers exported artifacts.
It does not prove all executable functions were discovered. Three charge callbacks
(0054b370, 0054b380, 0054b3e0) demonstrate the distinction. Resolve similar omissions
on demand; a full re-decompile is not a prerequisite for every feature.

Raw game assets, C exports, screenshots and generated local evidence stay under ignored
`data/` or the external research directories. Catalog hashes identify the checked build.
No original content is added to Git by these tools.

## Next playable increment

Complete Reimu's skill, natural meter gain, then opponent EX/boss dispatch and barrier
recovery. Read `topic charge` and `topic combat` first. The current executable only
exposes the free first charge tier. Exact costs already have source leads; meter gain
and skill collider arguments are the main unresolved questions. Keep each accepted
behavior tied to original assets/code and a targeted runtime check.
