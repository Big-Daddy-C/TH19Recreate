# Workflow tool checks - 2026-10-02

Runtime baseline: 92b07ae. This change adds research infrastructure, not game behavior.

- `python -m unittest discover -s tools -p test_research.py -v`: 5 passed.
  Covers aliases, bounded excerpts, source/executable drift detection, duplicate
  address rejection and PE virtual-address mapping with file-backed bounds.
- `python tools/research.py check`: passed against 7,769 current function files,
  30 named references and five feature topics. This is catalog integrity only.
- `show reimu_charge_start --limit 8`: returned correct file and continuation line.
- `show charge_shot_speed --limit 8`: exit 1 and explicit missing-export report.
- `asm charge_shot_speed --count 8`: decoded original callback after SHA256 check.
- `table 005a8a18 --count 4`: fourth pointer is 0053b110, matching recorded basic callback.
- `callers meter_spend --limit 2`: two textual references in charge controller.
- `topic`: returned five concise feature summaries.

No gameplay or original-game comparison was run for this infrastructure-only change.
No measured token or development-time speedup is claimed. The tool reduces repeated
address/path discovery and bounds returned source text; usage will determine the savings.
