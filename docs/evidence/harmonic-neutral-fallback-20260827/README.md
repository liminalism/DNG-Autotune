# Harmonic heterogeneity guard: neutral fallback — evidence, 2026-08-27

Root cause of the magenta/pink false colour on clipped regions
(raw/raw_backlit and, it turned out, across the corpus): in
`joint_log_chromaticity`, the region-global heterogeneity guard
(`component_coherence`) attenuated the reconstruction toward the **clipped
floor**, whose chroma records which channel clipped first (G first on this
sensor → magenta after WB), not the scene. On mixed-colour components
(door/window openings, sky through canopy) coherence collapses to ~0.13 and
the whole opening kept clipped chroma.

Diagnosis chain (numeric probes, `RAW_AUTOTUNE_PROBE_GRID`):
cell (1200,600) of _DSC1334 — grid value G at clip floor 1.069, harmonic
colour-line prediction G **1.748** (correct; true ≈ 1.71 from the measured
R channel and the neutral point), joint output G **1.146** = floor +
0.126·0.965·(1.75−1.069). The good reconstruction existed and the guard
threw it away.

Fix: the guard's fallback target is now **neutral chroma at the harmonic
luminance** instead of the clipped floor; the discount of the joint chroma
in incoherent components is unchanged, and coherent components are
unchanged in the coherence→1 limit.

Files:
- `stage-grid.png` — solver stages at grid level (all clean; defect not here)
- `develop-strip.png` — develop stages; display-clipping masked the cast
- `fix-compare.png` — _DSC1334: before / fixed / current / camera
- `ab-sheet.png` — 16-frame corpus A/B, top changed frames (every change an improvement; coherent skies/sunsets byte-stable)
- `ab2-sheet.png` — purple-sky question frames _DSC1282/1283/1290 (patch population fixed) and night _DSC1309 (blown screen neutral)

Unchanged (<0.1%): _DSC1288/1289 — the *broad faint* lavender population of
docs/PURPLE_SKY_PROBLEM_SCOPE.md is a separate cause and remains open.
