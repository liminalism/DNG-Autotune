# 2026-08-27 (later) — the residual patch fix, and the lavender cast's acquittal

Two parallel investigations into what remained after the v7 neutral-fallback fix
(`c09b7f9`, `docs/evidence/harmonic-neutral-fallback-20260827/`).

## 1. _DSC1282 residual pink patch → fixed (`archive-auto-v8`)

v7 neutralized the heterogeneity guard's *target* but the recomposition in
`joint_log_chromaticity` still started from the raw clipped floor:

```text
output = floor + support · (target − floor)
```

With partial colour-line support (~0.55 on _DSC1282's canopy sky) the cell kept
`1 − support` of the floor's magenta even though the target was neutral. A second
mechanism compounded it: `neutral_luminance` (mean of the harmonic channels) can sit
*below* a channel's own white-balanced clip floor, and the per-channel `.max(floor)`
clamp then re-injects clip chroma asymmetrically (probed at cell (2020,40): support
1.0 yet a 0.26 green deficit from this alone).

Fix (`fix(highlight): neutralize the support blend's base, not only its target`):
the base gets the same coherence blend as the target, anchored to the
**least-commitment neutral** `clip_neutral = max_c(floor[c] · wb[c])` — the dimmest
neutral consistent with all three clip lower-bounds — and `neutral_luminance` is
clamped to it. Coherent components unchanged in the coherence→1 limit; output ≥ floor
holds.

Numbers: patch magenta index 17.2 → 2.5 (camera 2.4). Corpus (58 files): 33
byte-identical; the 29 changed paired frames improve mean |magenta − camera|
8.34 → 6.17; every visually checked mover better (`ab__DSC1251.png`: magenta
twilight sky 31.9 → 9.2, camera 2.6; `ab__DSC1287.png`: dirty pink-grey blown sky →
clean white). Regression test pins the partial-support incoherent case.

- `ab_1282_patch.png`, `ab_1282_full.png` — before / after / camera
- `ab__DSC1251.png`, `ab__DSC1287.png` — biggest corpus movers

## 2. _DSC1288/1289 broad faint lavender → not a defect; a look decision

Bisection: the cast survives `--highlight-method current` and
`--highlight-reconstruction 0` unchanged — it is not in the reconstruction path.
Direct CFA probe of an **unclipped** sky window: R:G:B = 6201:6881:8853 under the
as-shot WB (B/R 1.43, nothing clipped), i.e. the sky genuinely is blue-violet and
the colour matrix renders it faithfully; the camera JPEG instead converges the same
pixels to white with a much stronger highlight-to-white shoulder
(ours: `highlight_desaturation` 0.16). Below L = 0.35 our ratios match the camera to
within 0.03; the divergence is entirely the shoulder policy, aggravated by our sky
rendering 0.25–0.57 EV darker (ground matches to 0.05 EV — connects to the standing
midtone/shadow-contrast work item).

Options quantified on the 56-frame paired corpus:

- **A. Raise `highlight_desaturation` to camera levels** — fixes the targets but
  bleaches legitimate blue skies (`ab_1262.png`); the populations overlap in
  `normalized_highlight`. Rejected.
- **B. Keep faithful rendering** (current default) — `docs/STATUS.md` already
  records the shoulder divergence as deliberate.
- **C. Clip-evidence-gated path to white** — candidate on branch
  `fix/lavender-cast` (`7f4fc29`, not merged): discriminates perfectly on real
  frames (57–92% clipped green sites on the targets, zero on the blue-sky
  controls; zero regressions across 56 frames) but fails the BlueRamp synthetic
  gate (final OKLab RMSE 0.0073 → 0.0799) because it discards a *correctly
  reconstructed* blue clipped highlight, and it whitens only the blown core,
  leaving the unclipped lavender surround (`ab_1289.png`).

Population note: _DSC1249/1250/1253/1258 share the same mechanism (0.7–2.2 EV
darker skies, +22–35 pt green-deficit delta) and belong here, not with the patch
defect. The full closure path is a DCP-style hue/sat table — Candidate D in
`docs/PURPLE_SKY_PROBLEM_SCOPE.md`.
