# Captures and licence calls still needed

Written 2026-08-27. This is the photographer-facing brief for the three
planning heads that cannot move without you. Nothing here changes a
default pixel. The same points are in the AKR ledger as
`@raw-autotune.observation.remaining-work-blocked-on-user-captures` so a
later session’s `akr start` / `akr context` will surface them.

Every shot: **RAW+JPEG in one press**, camera on auto WB/exposure as
usual. Sony A7C unless noted. 3–5 keepers per scene with small
angle/framing variation is plenty.

---

## 1. Phase 4 — backlit / indoor policy (measured, no render change)

**Done 2026-08-28.** `raw/raw_backlit2` (14 RAW+JPEG pairs) is the
contre-jour set that `raw/raw_backlit` was not: hand against sun (fill)
and the same hand at dusk (silhouette), plus architecture-against-sky
and window-looking-out.

- Classifier sees Backlight on the hand series. Geometric
  centre-vs-surround never reaches the 1.25 EV Actionable gate
  (off-centre silhouettes go negative). Fusion stays `observed`.
- Default preview-guided exposure already matches the camera’s
  silhouette-vs-fill (median key +0.06 EV). `--no-preview` on the dusk
  silhouettes is +3.5 EV and ruins them.
- Indoor `--local-white-balance` was already rejected
  (`docs/evidence/phase4-indoor-wb-20260827/`).

**Do not.** Do not consume `backlit == Actionable` for a fill or crush.
Do not lower the 1.25 EV margin to manufacture Actionable. Do not gate
`--local-white-balance` on indoor-actionable.

Ledger: `@raw-autotune.work.phase-4-backlit-scene-lighting-wiring`,
`@raw-autotune.decision.backlit-tone-policy-does-not-ship`,
`@raw-autotune.decision.indoor-actionable-does-not-enable-local-wb`.
Evidence: `docs/evidence/phase4-backlit2-20260828/`.

Optional, still useful later, **not** required to close phase 4: mixed
window+tungsten interiors (room lights on, daylight through a window)
before anyone revisits local-WB as a mixed-light operator.

---

## 2. Slice 8 — reconstruction ground-truth brackets

**Why.** Stage attribution is done. The sky discontinuity is amplified
by the tone shoulder. `--local-tone` and `--hdr` change luminance and
leave the colour estimate alone, so they must not be promoted as a
colour fix. The open acceptance check
(`local-tone-hdr-remain-colour-neutral`) waits on a bench that can tell
**recovered detail from invented colour**.

Synthetic gates already exist. What does not is an end-to-end pair
where a clipped channel in frame A is *measured* (not reconstructed) in
frame B.

**Shoot** — tripod, same framing, both RAW+JPEG:

- **A** — the frame that actually clips. Needs a *coloured* clipped
  thing: blue sky through trees, red taillight, green canopy against
  sky. A white blowout teaches nothing.
- **B** — the same scene **1–2 stops darker** so the channel that
  clipped in A is recorded in B.

B is ground truth for “what colour was really there.” Three to five
such pairs is enough. `_DSC1289`-class sky through foliage is the
highest-value composition.

**Do not.** Do not promote `--local-tone` or `--hdr` into the automatic
profile. Do not use either operator to hide a colour-estimation
discontinuity. Do not treat reconstruction-disabled as the quality
target; it already lost the camera-grade comparison.

Ledger: `@raw-autotune.work.slice-8-local-tone-sky-ground-hdr-evaluation`
(check `local-tone-hdr-remain-colour-neutral` unsatisfied).

---

## 3. Candidate D — chart + sky, or a licence review

**Why.** You kept faithful blue-violet sky over the camera’s stronger
white shoulder
(`@raw-autotune.decision.faithful-blue-violet-sky-over-camera-white-shoulder`).
The leftover look gap is per-hue, not a global shoulder and not an
illuminant miss (see the indoor A/B). Candidate D is a DCP-style
hue/saturation table **after** the colour matrix. No code until table
data exists.

Two ways in. Pick one.

**2026-08-28.** CamSDD has no colour chart (30 scene-JPEG classes, not a
characterisation set). A ColorChecker is not available where the photographer
is. Path B is settled on the ART/RawTherapee `SONY ILCE-7C.dcp`
(`ProfileCopyright` = `public domain`; not Adobe). `--hue-sat-map` ships
default-off; a chart plus sky brackets remain the preferred *our* table if
one ever appears.

### A — shoot a table (preferred; no licence problem)

On the A7C, **same lens** you care about (the FE 24mm F2.8 G if that is
still the corpus lens), RAW+JPEG:

- A colour chart (ColorChecker or equivalent) in daylight.
- The same chart in open shade.
- Sky brackets of the `_DSC1288` / `_DSC1289` class of bright
  blue-violet sky, including a darker frame that is clearly unclipped.
- Hold out a few frames from any later fit.

That is *our* table. Nothing third-party to ship.

### B — reuse someone else’s table (the licence issue)

This project is **AGPL-3.0-or-later**. A hue/sat table is copyrighted
**data**, not an idea. “We are AGPL too” does not automatically clear a
source.

| Source | What it is | Can we ship it? |
|---|---|---|
| **Adobe DCP** (`HueSatMap` inside `.dcp`, Lightroom camera profiles) | Adobe’s proprietary camera profiles | **No.** You may *read* a DCP you legally have, on your machine, for a private experiment. We cannot commit Adobe’s numbers or redistribute them in a binary. The DNG spec being open does not license Adobe’s profile *contents*. |
| **Darktable / RawTherapee / ART** in-tree DCPs | Programs are GPL-3; each `.dcp` has its own `ProfileCopyright` | Combining GPL-3 *code* with our AGPL is fine. **File-by-file:** the ILCE-7C DCP in `profiles/` is tagged `public domain` (ART/RT user-submitted; identity tone curve, so it is not the Adobe default-curve trap). Other DCPs in those trees may still be Adobe-derived (`Adobe Systems` in the copyright tag) or GPL-2-only — do not copy those. Attribution in `profiles/README.md`. |
| **CamSDD classifier** (already in tree) | CC BY-NC-SA 4.0 inherited from the dataset | Not Candidate D and has no colour chart. Already limits commercial redistribution of `--scene-classify` weights. Do not treat that as a precedent for shipping Adobe tables. |

**Do not.** Do not implement `--hue-sat-table` (or equivalent) without
one of A or B settled. Do not copy Adobe DCP values into `src/`. Do not
assume Darktable’s `adobe_coeff` / white-balance presets are clean to
vend. Do not weaken the default highlight shoulder to mimic the camera
JPEG (already decided against).

Ledger: `@raw-autotune.work.calibrated-hue-sat-table-candidate-d`,
`docs/PURPLE_SKY_PROBLEM_SCOPE.md` Candidate D and its acceptance gates.

---

## What you can ignore until those exist

- Indoor tungsten WB policy (measured, will not ship).
- Spatial-floor constant (censused, 1e-5 stays;
  `docs/evidence/spatial-floor-sweep-20260827/`).
- Memory planner vs the floor (Bayer keeps worst-case Harmonic;
  LinearRaw Expert RAW no longer reserves Harmonic — 50 MP files can
  join).
- Slice 8 stage dumps and renderer-exact checkpoints (already built).

Drop new folders next to the existing ones (`raw/raw_backlit`,
`raw/indoor_tungsten`) so a later session can find them without a
scavenger hunt. Name them for the class (`raw/raw_contrejour`,
`raw/recon_brackets`, `raw/chart_daylight`).
