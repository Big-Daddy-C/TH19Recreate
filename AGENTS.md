# Working on th19-rs

Goal: reconstruct Touhou 19 v1.10c in Rust using the owner's original assets and
executable behavior. Current runtime uses wgpu/winit; engine migration is a separate
scope decision. Preserve the working implementation as research continues.

## Start with existing evidence

1. Read `research/WORKFLOW.md` and run `python tools/research.py topic`.
2. For the current feature, run `python tools/research.py topic charge` (or the other
   relevant topic). This includes already-known user observations and next questions.
3. Before relying on address-based findings, run `python tools/research.py check`.
   If the source changed, inspect the changed evidence and update its findings before
   refreshing fingerprints. Never blindly refresh a mismatching executable hash.
4. Batch related functions in one `show` call. Read bounded excerpts, then follow
   concrete unresolved branches. Use `asm` for missing exports/hidden arguments.
5. Save new findings, source addresses, confidence, implementation gaps and comparison
   status in the catalog during the same change. Keep raw game data/decompiled C private.

Implement complete, playable increments with checks appropriate to the change. A passing
build or synthetic test does not establish original-game parity. Preserve approximations
and open issues explicitly. Ask the user about visible behavior only when their existing
notes cannot resolve a material ambiguity; use source data for exact numeric details.

The user authorizes routine local edits, builds, tests and continued implementation.
Do not import another project's per-build permission rule from the tutorial attachment.
Ask before irreversible replacement of an existing research database. Do not modify the
original game installation. Do not push/publish without authorization.
