# 2026-08-27 — spatial highlight floor census and memory-planner costing

Resolves `question.spatial-highlight-floor-not-swept` and
`question.memory-planner-cannot-use-the-spatial-floor`.

Command (production path, floor forced to 1 so every frame *counts* CFA
sites and then declines the solver; chroma/luma/night/sharpen off so the
survey is the clip census, not a render):

```
raw-autotune <381 CFA files> --dry-run --summary cfa-corpus.json \
  --spatial-highlight-floor 1 --jobs 8 \
  --chroma-denoise 0 --luma-denoise 0 --night-tone 0 --sharpen 0
```

381/381 completed, 0 failed. Wall 3:25 at three concurrent images (memory
safety capped `--jobs 8` to 3 because the planner still reserved Harmonic
on the 24 MP CFA ceiling). `analyze.py` joins the summary against
`raw-dimensions.json` (TIFF photometric/size probe, no decode).

The 16 LinearRaw Expert RAW files were held out of this census: they have
no Bayer mosaic, so `reconstruct_cfa` never runs and the floor does not
apply. They *are* the memory-planner half (below).

## Floor: the empty-band hypothesis is false

`DEFAULT_SPATIAL_CLIPPED_FLOOR` is 1e-5 (~240 sites on a 24 MP mosaic).
The 29-frame sample in `docs/evidence/archive-speed-20260826/` looked like
a wide empty band between 0 and 7.3e-6, so "any floor inside that band
behaves identically". That band was a reporting artifact: a declined
frame's Current-path sidecar hard-coded `clipped_cfa_sites` to 0 even
though `reconstruct_cfa` had counted them. `_DSC0883` is 6 sites
(5.71e-7), not 0; the always-solve column of that sample already said so.

Full CFA corpus (n=381):

| clipped CFA fraction | frames | % |
|---|---|---|
| 0 | 99 | 26.0 |
| (0, 1e-5) | 71 | 18.6 |
| ≥ 1e-5 (solves at the default) | 211 | 55.4 |

Positive-fraction min is 4.11e-8 (one site on `_DSC1255.ARW`). There is
no empty band. 71 frames sit inside (0, 1e-5) with 1–224 sites (max
`_DSC1313.ARW`, 9.20e-6). The constant is a real gate, not a no-op.

Decline rate at nearby floors:

| floor | declined | solved |
|---|---|---|
| 1e-6 | 131 (34.4%) | 250 |
| **1e-5 (shipped)** | **170 (44.6%)** | **211** |
| 1e-4 | 213 (55.9%) | 168 |
| 1e-2 | 319 (83.7%) | 62 |

The original rationale was cost, not quality: "a few hundred isolated
clipped sites" are not worth ~20 s and ~1.3 GB. 224 sites is still that
population. The HDR-evaluation defect frames sit at ≥1% two-channel clip,
three orders above. Keep 1e-5. A quality A/B of the 71 against camera
JPEGs was not run; raising or lowering the constant would move those 71
and needs that measurement first.

`below-floor.csv` lists the 71. `census.json` is every frame.

## Statistic: the floor should keep gating on CFA sites

HDR_EVALUATION.md counted demosaiced pixels with two or more channels at
`CLIP_THRESHOLD` (0.98). The floor counts CFA sites at
`VALID_CONFIDENCE_MAX` (0.5, mid-ramp, ~0.9525 linear). Of 285 frames
with any clip on either statistic, 254 have a larger CFA-site fraction
than two-plus pixel fraction. 62 frames are ≥1% CFA-site; 50 are ≥1%
two-plus. Expected: the floor's population is strictly larger. Do not
retarget the floor at two-plus — it gates a CFA solver, and that is the
statistic it should compare.

## Early return vs the floor

98 frames report method `harmonic` with zero clipped CFA sites: those
are `has_reconstruction_authority == false` (every site's clip
confidence is 0), which returns the *requested* method without solving
and routes `color::develop` to `spatial_report_and_uncertainty` instead
of the Current estimator. One further zero-site frame has sub-0.5
confidence somewhere, hits the floor, and reports `current`.

Making those 98 report `Current` would take the Current reconstruction
path. Every CFA site on them is below `CLIP_RAMP_LOW` (0.92), so Current
is a documented no-op on the mosaic; the change was left alone because
it was believed to move pixels. This census does not A/B it. Leave the
early return as it is.

## Sidecar fold (code)

`HighlightReport::into_report` still hard-codes the mosaic counters to 0
— the Current tally never saw a mosaic. `fold_raw_counters` now copies
`reconstruct_cfa`'s counts onto that report when a spatial pass ran,
including when the floor declined it. Without the fold this census
cannot see the 71. Pixels are untouched. Regression:
`a_declined_spatial_pass_still_reports_clipped_cfa_sites`.

## Memory planner: LinearRaw is knowable; the floor is not

Three options, now costed.

**A — reserve Current, grow the pool when a frame needs the solver.**
Breaks the guarantee that a plan cannot be exceeded. Three 24 MP Current
workers (1.33 GiB) plus one 24 MP Harmonic (2.78 GiB) arriving later is
5.44 GiB against an 8.67 GiB budget on this machine and still looks
fine; three Harmonic in flight is 8.35 GiB, inside by millimetres, and a
50 MP CFA (none in the corpus) would not be. Rejected.

**B — probe clipped-site fraction before planning.** The census that
produces that fraction *is* a decode: 3:25 for 381 CFA files on this
machine. A header-only probe cannot see clip confidence. Rejected as a
pre-pass.

**C — keep the worst-case CFA reserve.** Still right for Bayer frames.
The planner cannot see the floor without a decode, and 211/381 CFA
frames still clear 1e-5, so a mixed Sony batch is sized on 24 MP
Harmonic either way.

**LinearRaw, separate and shipped.** Expert RAW is photometric
LinearRaw: already demosaiced, `reconstruct_cfa` is never called, yet
the planner used `max(pixels) × Harmonic extra`. Ten 50 MP files in this
corpus (the handoff's "200 MP" DNGs — 8160×6120 LinearRaw from the 200
MP sensor) budgeted 5.644 GiB and either refused to start or forced
`--jobs 1` on the whole batch. They now budget as Current (2.667 GiB).
A mixed batch takes the *max per-file cost*: 24 MP Bayer Harmonic
(2.782 GiB) binds over 50 MP LinearRaw Current, so `--jobs auto` stays
3, and the 50 MP files can join. Header-only: the same TIFF directory
the planner already reads.

The 16 LinearRaw files: 10× 8160×6120, 1× 5712×4284, 3× 4080×3060, 2×
3648×2736.
