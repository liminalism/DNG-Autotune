# 2026-08-27 — midtone/shadow contrast vs reference renderers: measured, decided

Corpus resolution of `work.midtone-shadow-contrast-versus-reference-renderers`.
111 RAW+camera-JPEG pairs across six sets (raw_3rd_batch paired with raw/jpeg,
arw_better, indoor_tungsten, raw_backlit, raw_extremely_bright, and the 21
normal-resolution Samsung raw_better DNGs — the 200 MP ones exceed the memory
guard on this machine). Both renderings measured with one code path
(`compare_percentiles.py`): Rec.709 display-luminance percentiles in EV
relative to middle grey.

## Finding 1 — the entry hypothesis is refuted against the gate reference

The work item's entry observation ("every percentile below the highlight end
sits about twice as high") came from Windows Photo Viewer. Against the paired
**camera JPEGs** — the reference the project's own gate names — the midtone
lift does not exist: median dEV(p50) is **−0.01 EV** over all 111 pairs, and
on the entry frame _DSC1289 itself our p50 is 92 against the camera's 89
(WPV renders the same file at 49). WPV is one vendor's darker aesthetic, and
the ~2× claim was an artifact of that choice of reference.

## Finding 2 — what actually deviates, systematically

| percentile | median (ours − camera) |
|---|---|
| p05 | **+0.44 EV** (Sony sets +0.56…+1.40) |
| p25 | +0.27 EV |
| p50 | −0.01 EV |
| p75 | −0.15 EV |
| p95 | −0.22 EV |

The shadow **toe** renders about half a stop brighter than the camera on Sony
frames (the camera crushes deep shadows harder), and the highlight end sits a
quarter stop lower (the camera's stronger white shoulder — the same divergence
already sealed as `decision.faithful-blue-violet-sky-over-camera-white-shoulder`).
The Samsung set inverts the toe sign (−0.36 EV at p05, flat ≈−0.2 EV
everywhere): the phone's computational JPEG brightens globally. Deltas
correlate with `center_median_ev` (r ≈ −0.7 at p50), driven by very dark
scenes: the preview oracle's ±3.5 EV cap keeps _DSC1275–78 about 3 EV brighter
than the camera's near-black rendering, and the genuine night frames
(_DSC1253/1257, `low_light_score` 1.0) diverge in both directions.

## Finding 3 — the candidate fix works as designed and is still wrong

`black_output_linear` (auto: 0.0012, set by eye in the initial commit, never
corpus-measured) is the rendered floor, and because `shadow_power` is derived
from it, halving it to 0.0006 moves **only** the toe: measured shift p05
−0.17 EV median, p50/p75/p95 unchanged (`toe-ab-eval.txt`). The signed toe
delta improves (+0.44 → +0.26) — and 21 frames get **further** from the
camera at the toe, because the population is bimodal: night frames, low-key
frames and most Samsung pairs are already at or below the camera's toe.
_DSC1253 (night, already darker than the camera at every percentile) worsens
across p05–p75 — exactly the crush the work item warned against. Median
|toe error| improves only 0.74 → 0.69 EV.

## Decision: keep the current placement

A global black constant cannot close a scene-adaptive difference: Sony's own
toe treatment varies per scene (DRO), and per-scene emulation of a vendor
curve is out of scope — the camera JPEG is a reference, not ground truth, and
the retained shadow detail is deliberate archival behaviour (visible in
`toe-ab-sheet.jpg`: the tungsten night cottage keeps its surroundings where
the camera crushes them to black). No default change ships; the experimental
binary and renders were discarded.

- `toe-ab-sheet.jpg` — _DSC1289 ours/camera (matched), _DSC1324 ours/camera
  (deliberate shadow retention), _DSC1253 ours/toe/camera (the harm case)
- `baseline-aggregate.txt` — full per-set/per-class tables and correlations
- `toe-ab-eval.txt` — the experiment's isolated toe shift and regression list
- `compare_percentiles.py`, `aggregate.py`, `ab_eval.py` — measurement code
