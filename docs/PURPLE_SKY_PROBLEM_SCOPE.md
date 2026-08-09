# Residual lavender/purple skies: problem scope and external-review brief

Status: **unresolved**
Prepared: 2026-08-08
Project: `raw-autotune` 0.1.19 working tree, based on commit `ff74d3b248f65ea98bde93578a3b95c80a647181`

## Purpose

This document is intended to be shared with RAW-pipeline, color-science,
computational-photography, and Darktable/RawTherapee experts. It describes the
remaining lavender/purple-sky defect, distinguishes observations from
hypotheses, records failed approaches, and asks for advice on the next
experiments and architecture.

The defect is not considered solved. A previous dark-taupe/posterization bug in
clipped skies was fixed, but a residual broad lavender cast and purple edge
fringing remain. An exact Lensfun profile improved the edge-localized population
but did little to the broad cast. A compression-aware color-ratio experiment
improved the target crop but failed the full-batch clipping gate.

## Short version of the question

Given a Bayer RAW in which most purple-looking sky pixels are below the measured
sensor clip threshold, how should an unattended RAW converter distinguish and
correct:

1. transverse chromatic aberration (spatial channel displacement),
2. longitudinal chromatic aberration, purple fringing, blooming, or flare,
3. demosaic artifacts at high-contrast boundaries,
4. camera-profile or white-balance errors that become visible in bright blue
   colors,
5. highlight color distortion introduced or amplified by tone/gamut mapping,
6. a legitimate scene color that should not be neutralized?

In particular, should this project adopt a Darktable-style data-driven RAW CA
stage, its post-demosaic adaptive-manifold CA stage, a dedicated high-luminance
color-restoration stage similar in purpose to Sony Imaging Edge's highlight
color distortion reduction, improved camera profiles, or some combination?

## Scope and target sources

The project must work unattended on:

- Sony A7C (`ILCE-7C`) ARW files, especially the Sony FE 24mm F2.8 G lens;
- Samsung Galaxy/Expert RAW linear DNGs;
- Samsung/ProShot Bayer DNGs.

The main visual targets are `_DSC1282`, `_DSC1283`, `_DSC1288`, `_DSC1289`,
and `_DSC1290` in `raw/raw_3rd_batch`. `_DSC1289` is the most useful current
case because it contains both a broad faint lavender sky and edge-localized
purple near a dark ridge/foliage boundary.

## What is established

### The original severe defect was real and separate

The earlier renderer made the most-clipped sky pixels darker than their less
clipped neighbors, producing dark taupe areas surrounded by near-white contours.
That inversion came from highlight reconstruction plus an uncertainty-dependent
chroma contraction. The current near-white reconstruction and peak-preserving
path to white remove that inversion. Fully clipped sky now renders approximately
neutral and bright.

The discussion below is about what remains after that fix.

### The remaining pixels are mostly not classified as clipped

On `_DSC1289`, approximately 1.15% of the sky remains visibly lavender and
99.2% of that selected population is below the raw-domain reconstruction clip
threshold. Along `_DSC1283`'s purple ridge band, approximately 95% is similarly
below that threshold. Reconstruction-off and PPG, RCD, AMaZE, and Rawler
demosaic controls retain the broad color.

This establishes only that the residual is not primarily created by the current
highlight-reconstruction branch. It does **not** establish that the color is a
faithful measurement of the scene. The signal may already include optical
aberration, sensor behavior, interpolation, white balance, or an inadequate
camera transform before reconstruction runs.

### A fixed lens profile addresses only part of it

An exactly matched Lensfun 0.7.0 profile for `Sony ILCE-7C` plus
`Sony FE 24mm f/2.8 G` was tested with distortion and transverse chromatic
aberration correction, without vignetting or fuzzy profile selection.

On `_DSC1289`:

| Measurement | Existing path | Exact profile |
|---|---:|---:|
| Edge-localized purple mask | 0.59% | 0.34% |
| Broad sky green deficit | 13.8% | 13.6% |

The profile therefore finds a genuine optical/spatial component but does not
explain the broad cast. It also changes geometry and framing. A 58-frame run
with the profile plus the color-ratio experiment increased maximum decoded-JPEG
hard clipping from 0.204% to 3.178% and maximum near-white area from 2.0% to
28.0%. Neither experiment is enabled by default.

### A tone-compression color-ratio experiment is insufficient

The Mantiuk-inspired experiment raises color/luminance ratios to an exponent
below one only where the tone curve is compressing and then renormalizes to
preserve mapped luminance. At exponent 0.6, `_DSC1289`'s profiled sky green
deficit falls to 7.6% with essentially unchanged ground exposure. However, the
full-batch clipping regression above rejects it as a production setting.

The experiment supports the hypothesis that tone/color handling contributes to
the broad cast, but it is not a complete or adequately bounded solution.

## Why Darktable is relevant

Darktable has three CA mechanisms, not one. This project currently implements
only the profile/embedded-geometry category.

Darktable is not installed in the current evaluation environment, so no claim
below is based on a Darktable render of these files yet. The descriptions come
from Darktable's manual and source pinned to commit
`da5a3c743dfacf46aee87432f081db163568acd0`; running the ablation matrix is a
next experiment.

### 1. Profile or embedded lens correction

Darktable's lens correction can use either embedded RAW metadata or Lensfun and
can separately enable distortion, TCA, and vignetting. It also provides manual
red/blue TCA overrides and an automatic scale/crop control. Darktable warns that
combining TCA here with RAW CA can over-correct. See the
[Darktable lens-correction manual](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/lens-correction/).

Our exact-profile experiment corresponds only to this layer. It lacks
Darktable's manual fine-tuning, auto-scale policy, and broader support for
vendor-embedded correction metadata.

### 2. Image-driven RAW CA before demosaic

Darktable's `cacorrect.c`, derived from RawTherapee work, operates on the Bayer
mosaic. It estimates red and blue sample displacement relative to interpolated
green from image evidence in overlapping tiles. It finds per-tile vertical and
horizontal shifts that minimize color-difference variance, rejects or
downweights unreliable shifts, fits a smooth two-dimensional polynomial field,
then resamples the non-green photosites. Its optional color-shift avoidance pass
uses blurred pre/post correction ratios.

Relevant pinned source:

- [`cacorrect.c`, data-driven tile fit](https://github.com/darktable-org/darktable/blob/da5a3c743dfacf46aee87432f081db163568acd0/src/iop/cacorrect.c#L430-L675)
- [`cacorrect.c`, color-shift avoidance](https://github.com/darktable-org/darktable/blob/da5a3c743dfacf46aee87432f081db163568acd0/src/iop/cacorrect.c#L1120-L1193)
- [RAW chromatic-aberrations manual](https://docs.darktable.org/usermanual/3.6/en/module-reference/processing-modules/raw-chromatic-aberrations/)

This is materially different from Lensfun: it estimates what the particular
capture needs and runs before demosaic can turn a channel displacement into a
colored fringe. It is a strong candidate for the Sony edge defect.

### 3. Post-demosaic adaptive-manifold CA correction

Darktable also has a scene-linear RGB module for residual CA and blur mismatch.
It preserves a selected guide channel and models the other two as functions of
that guide. It constructs high and low local manifolds using log channel ratios,
interpolates between their ratios according to the guide pixel, and includes a
safeguard that blends back toward the input when local averages move too far.
It can operate in normal, brighten-only, or darken-only mode.

Relevant pinned source:

- [`cacorrectrgb.c`, algorithm rationale](https://github.com/darktable-org/darktable/blob/da5a3c743dfacf46aee87432f081db163568acd0/src/iop/cacorrectrgb.c#L50-L128)
- [`cacorrectrgb.c`, manifold construction](https://github.com/darktable-org/darktable/blob/da5a3c743dfacf46aee87432f081db163568acd0/src/iop/cacorrectrgb.c#L250-L494)
- [`cacorrectrgb.c`, ratio correction and artifact safeguard](https://github.com/darktable-org/darktable/blob/da5a3c743dfacf46aee87432f081db163568acd0/src/iop/cacorrectrgb.c#L497-L620)
- [Darktable chromatic-aberrations manual](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/chromatic-aberrations/)

This mechanism is relevant because our residual includes both spatially shifted
edges and possibly a channel whose edge is blurrier than the others. It is also
risky: an unrestricted guide-channel model can wash out a legitimate blue sky
or propagate foliage color. Any prototype must expose its correction mask and
be evaluated independently on flat sky and high-contrast edges.

### Darktable's highlight reconstruction is also more plural

Darktable offers opposed inpainting, segmentation-based reconstruction, guided
Laplacians, clipping, and LCh reconstruction. Its segmentation mode reasons per
clipped connected region and rejects dark/edge boundary candidates; guided
Laplacians propagate gradients at multiple scales. The manual explicitly warns
that large scales can import unrelated sky or foliage color and increase both
memory use and runtime. See the
[Darktable highlight-reconstruction manual](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/highlight-reconstruction/).

Those methods may inform genuinely clipped areas, but they should not be applied
to the mostly-unclipped broad `_DSC1289` cast merely because it looks like a
highlight problem.

## What Sony appears to do that this project does not

Sony documents multiple distinct controls in its production RAW/JPEG workflow:

- compatible lenses can receive automatic in-camera chromatic-aberration
  compensation ([Sony A7C help guide, “Lens Comp.”](https://helpguide.sony.net/ilc/2020/v1/en/print.pdf));
- Imaging Edge performs lens correction for distortion and chromatic
  aberration;
- Imaging Edge has a dedicated **highlight color distortion reduction** mode,
  whose advanced setting is described as making high-luminance regions such as
  bright skies reproduce with a more natural tone;
- it separately exposes white balance/tint, Creative Style/Creative Look,
  per-channel tone curves, and D-Range Optimizer, which analyzes the scene and
  treats highlights and shadows separately.

The relevant Sony documentation is
[Imaging Edge: Adjusting images](https://support.d-imaging.sony.co.jp/app/imagingedge/en/instruction/3_3_retouching.php).

The algorithms and ordering are proprietary, so it would be wrong to claim that
Sony uses any particular Darktable or published method. The documented feature
separation is still important: Sony does not treat bright-sky color distortion
as synonymous with geometric TCA. Our current pipeline largely does.

Sony's camera JPEG is therefore an informative reference for preferred output,
but not a colorimetric ground truth. It includes camera-specific calibration,
highlight-color handling, tone decisions, and a creative look that the ARW
alone does not fully specify in a portable form.

## What Samsung appears to do that this project does not

Samsung Expert RAW is not a minimally processed single capture. Samsung calls
it a multi-frame-based 16-bit computational RAW system, and Samsung Research
describes a Multi-Frame Processor that collects and combines multiple frames.
Samsung also documents ISP stages for automatic white balance, demosaic, and
per-pixel adaptive dynamic-range compression; newer Galaxy processing can use
content recognition and segmentation to apply processing by region.

Primary descriptions:

- [Samsung Research: Multi-Frame Processor](https://research.samsung.com/srTalks/-SR-Talks-Interview-with-a-Camera-Expert-at-Samsung-Research-America)
- [Samsung Expert RAW: computational RAW and multi-frame processing](https://news.samsung.com/global/user-guide-no-need-for-heavy-cameras-a-day-in-san-francisco-with-the-galaxy-s23-ultra)
- [Samsung ISP: AWB, demosaic, and adaptive local dynamic-range compression](https://semiconductor.samsung.com/technologies/image-processing/)
- [Samsung Tetrapixel/HDR color capture](https://semiconductor.samsung.com/news-events/tech-blog/how-tetrapixel-delivers-crystal-clear-photos-day-and-night/)

This means the Samsung comparisons need three separate labels:

1. ProShot Bayer DNG: closest to a conventional single-frame mosaic;
2. Expert RAW DNG: already computationally fused and possibly locally processed;
3. Samsung JPEG: a fully rendered, content-aware output.

Treating all three as interchangeable evidence for one RAW algorithm will lead
to false conclusions. It is currently unknown which Samsung processing stages
are baked into each DNG in this corpus and which exist only in the JPEG.

## Relevant supplied research

### Rouf, Lau, and Heidrich: gradient-domain clipped-highlight color restoration

`research/Rouf2012GDC.pdf` is directly relevant to pixels with one or two
clipped channels. It propagates a smooth hue field from the boundary of clipped
regions, estimates missing-channel gradients from unclipped channels, and solves
Poisson problems in the gradient domain. Its principal benefit over a per-pixel
rule is explicit continuity across regions with different clipped-channel
counts. Its assumptions weaken in fully clipped areas and wherever the boundary
color belongs to a different object.

This paper should be evaluated as a bounded clipped-region reconstruction
candidate, not as a broad correction for the 99.2% unclipped lavender
population. A tiled or multigrid solver would be required to respect this
project's memory constraints.

### Mantiuk et al.: display-adaptive tone mapping

`research/mantiuk08datm.pdf` motivates preserving color/luminance relationships
while adapting contrast to the display. The current exponent experiment used
only a small color-ratio idea from that larger optimization framework. Its batch
failure should not be read as a rejection of display-aware tone mapping; it
shows that one isolated exponent, without the paper's contrast/visibility model
and without a sufficiently bounded gamut policy, is not an adequate substitute.

### Marnerides et al.: deep HDR hallucination

`research/marnerides2021.pdf` targets inverse tone mapping from LDR and generates
plausible missing content with a GAN. It is not a faithful recovery method for
this RAW defect and is unsuitable as the first production response: it requires
a model and training distribution, can invent sky structure, complicates
determinism and provenance, and adds substantial memory pressure. It may serve
as a distant comparison for fully saturated regions, not as the baseline fix.

## Current pipeline gaps

| Capability | Sony/Imaging Edge | Samsung production path | Darktable | raw-autotune now |
|---|---|---|---|---|
| Camera/lens-specific TCA | Documented auto correction | Likely ISP/lens-specific; details proprietary | Embedded metadata or Lensfun | Exact Lensfun opt-in; DNG opcodes default |
| Data-driven RAW CA before demosaic | Undocumented | Undocumented | Yes, Bayer tile/shift fit | No |
| Residual RGB CA/fringe correction | Highlight/lens tools are separate | Content-aware ISP is documented generally | Yes, adaptive manifolds | No |
| Dedicated bright-highlight color correction | Yes, explicitly documented | HDR/color processing documented generally | Multiple reconstruction/color modules | Only global shoulder and reconstruction confidence |
| Camera-specific look/profile tables | Creative Look/Style | Proprietary ISP/look | ICC/DCP-like workflow and calibration tools | Matrices; no DCP hue/sat/look tables |
| Local/content-aware tone | D-Range Optimizer | Per-pixel DRC and content recognition | Tone equalizer/filmic/local tools | Optional non-semantic local tone |
| Multi-frame fusion | Some modern Sony modes, not assumed for these ARWs | Core Expert RAW behavior | No for ordinary RAW development | No |

## Measurement weaknesses that must be fixed before another algorithm lands

1. The current “purple fraction” is a loose RGB threshold, not a perceptual or
   causal classification. It can count legitimate blue-violet sky.
2. “Green deficit” is useful for regression on this cohort but is not a general
   sky-correctness metric.
3. The top 15% sky band is corpus-specific and includes foliage or ridges in
   some frames.
4. Lens correction changes geometry. Comparisons must register/crop both images
   to a common valid field before pixel or population measurements.
5. Camera JPEGs include aesthetic rendering and cannot be the only ground truth.
6. “Unclipped” currently means below one chosen confidence threshold after
   normalization. White-level calibration error, channel headroom mismatch, and
   near-saturation nonlinearity still need examination.
7. Existing paired captures are not controlled calibration data. There is no
   neutral/color target photographed through an exposure bracket in the same
   daylight and lens configuration.

## Proposed investigation, in order

### Phase 0: preserve evidence and establish controlled references

- Freeze lossless stage dumps and RAW-coordinate ROIs for the five target Sony
  frames. Record hashes and exact parameters.
- Render the same ARWs through Sony Imaging Edge, Darktable, and at least one
  independent production converter with every automatic look disabled where
  possible.
- For Sony, export ablations for lens correction, highlight color distortion
  reduction, DRO, Creative Look, and white-balance/tint separately.
- Capture a new Sony exposure bracket containing blue sky, neutral cloud, dark
  foliage edges, a gray card, and a ColorChecker. Keep aperture, focus distance,
  focal length, and white balance fixed.
- Capture matched Samsung native JPEG, Expert RAW DNG, and ProShot DNG scenes.
  Do not infer one path's stages from another path's output.

### Phase 1: classify the defect spatially

- Work in the original CFA coordinates before demosaic and white balance.
- Measure red/green and blue/green edge displacement versus radius and azimuth.
  A smooth radial displacement supports TCA; a non-spatial broad ratio error
  does not.
- Compare edge spread functions by channel. Blur asymmetry without a simple
  displacement suggests longitudinal CA, flare, or sensor/demosaic behavior.
- Map correction need against raw clip confidence, distance from frame center,
  local gradient, edge orientation, luminance, and selected demosaic method.
- Inspect Sony MakerNotes and any embedded lens-correction metadata before
  assuming the Lensfun profile is the best available calibration.

### Phase 2: run Darktable as an oracle, not immediately as code to copy

For each target, using registered valid crops:

1. no CA correction;
2. Lensfun distortion only;
3. Lensfun TCA only;
4. raw CA only, with and without color-shift avoidance;
5. RGB adaptive-manifold CA only with R, G, and B guide choices;
6. Lensfun TCA followed by weak RGB CA;
7. highlight reconstruction methods only on genuinely clipped masks.

Record correction fields/masks, not just final JPEGs. If Darktable fixes the
edge but not the broad sky, the two-problem model is strengthened. If the RGB CA
module fixes the broad sky, verify that it is not merely desaturating all blue
regions.

### Phase 3: calibrate color and high-luminance behavior

- Verify Sony A7C white levels and channel-specific saturation points against
  exposure-bracket data.
- Compare the current camera-to-working matrix against Sony Imaging Edge,
  Darktable, Adobe/other DCP profiles, and a chart-derived profile under the
  target illuminant.
- Plot hue/chroma versus exposure for blue, cyan, magenta, and neutral patches.
  The point where converters diverge identifies whether the missing behavior is
  profile calibration, near-saturation handling, tone mapping, or gamut mapping.
- Treat Sony's highlight color distortion reduction as a black-box operator:
  difference its off/advanced exports in scene regions, luminance bands, and
  hues, then formulate the narrowest non-semantic approximation supported by
  those measurements.

### Phase 4: prototype only the mechanism supported by the evidence

Candidate A: port or independently reimplement a tiled RAW-domain CA estimator.
Candidate B: implement a bounded post-demosaic guide/manifold residual-CA stage.
Candidate C: implement a high-luminance color-distortion stage driven by measured
near-saturation behavior, not by “sky” labels.
Candidate D: add calibrated camera profiles/hue-saturation tables where matrix
color is demonstrably insufficient.

These are not mutually exclusive, but they must be evaluated independently
before composition. Direct reuse of Darktable/RawTherapee GPL code also requires
an explicit licensing and attribution review even though this project is
AGPL-3.0-or-later.

## Acceptance gates for any candidate

- Improves both registered target crops and a held-out Sony set; no tuning only
  to `_DSC1289`.
- Separately reports edge-fringe reduction and broad low-frequency cast change.
- Does not neutralize legitimate saturated blue/cyan objects or sunset color.
- Does not grow halos across sky/foliage or ridge boundaries.
- Does not increase hard clipping, near-white area, or hue discontinuity beyond
  agreed tolerances.
- Default-disabled value is exactly inert; unattended enablement requires a
  full-corpus gate.
- Deterministic across worker counts.
- Peak RSS is measured on one maximum-resolution/noisy frame before a batch is
  attempted.
- New full-frame buffers are included in the memory planner. Processing must
  fail before decode if one frame does not fit the reserved memory budget.
- Prefer tiled/streaming implementations. No unbounded neighborhood scan and no
  full-batch experiment while the machine has no swap and low `MemAvailable`.

## Questions for external reviewers

1. Which measurements best separate TCA, longitudinal CA/purple fringing,
   sensor blooming, demosaic artifacts, and a camera-profile error on these
   crops?
2. Is Darktable's RAW CA estimator appropriate for a modern Sony Bayer ARW with
   this lens, and which assumptions or failure modes should be tested first?
3. Is the adaptive-manifold RGB correction safe for smooth skies, or is there a
   better residual-fringe method that preserves broad chromaticity?
4. Where should raw CA, demosaic, geometric lens correction, highlight
   reconstruction, camera color conversion, and highlight-color correction be
   ordered, especially when TCA uses per-channel coordinates?
5. How should clip confidence and CFA white levels be transformed through these
   stages without creating false highlight evidence?
6. What known open method most closely resembles Sony Imaging Edge's documented
   highlight color distortion reduction?
7. Would a dual-illuminant DCP plus hue/saturation map plausibly address the
   broad cast, and what controlled chart/bracket capture is required to decide?
8. How should Expert RAW DNGs be interpreted when the source is already
   multi-frame computational RAW? Which corrections are likely baked in versus
   deferred to Samsung's JPEG ISP?
9. What objective metrics and visual crops would you require before accepting an
   unattended correction?
10. How can the recommended algorithm be tiled so a 50 MP frame stays safely
    below roughly 2 GiB peak RSS?

## Reproduction and materials to share

Subject to privacy/licensing approval, provide reviewers with:

- the five target ARWs and paired Sony JPEGs;
- lossless stage dumps from `--dump-stages`;
- `summary.json` sidecars containing raw clip-state and reconstruction data;
- registered full-resolution crops for broad sky, ridge edge, foliage edge, and
  a clean blue-sky control;
- outputs from the Darktable and Sony Imaging Edge ablation matrices above;
- `src/highlight.rs`, `src/lens.rs`, `src/color.rs`, `src/tone.rs`,
  `src/oklab.rs`, and `tools/grade_sky.py`;
- `research/Rouf2012GDC.pdf`, `research/mantiuk08datm.pdf`, and
  `research/marnerides2021.pdf` as citations, subject to their redistribution
  terms.

Current project commands:

```text
# Default renderer
raw-autotune raw/raw_3rd_batch/_DSC1289.ARW --output OUT --format png --jobs 1

# Exact Lensfun profile experiment
raw-autotune raw/raw_3rd_batch/_DSC1289.ARW --output OUT \
  --format png --jobs 1 --lens-correction profile-exact

# Compression-aware ratio experiment; rejected as a default
raw-autotune raw/raw_3rd_batch/_DSC1289.ARW --output OUT \
  --format png --jobs 1 --lens-correction profile-exact \
  --highlight-color-ratio-exponent 0.6

# Stage attribution
raw-autotune raw/raw_3rd_batch/_DSC1289.ARW --output OUT \
  --format png --jobs 1 --dump-stages OUT/stages
```

## Desired form of external advice

Please distinguish:

- diagnosis supported by the supplied pixels/metadata;
- assumptions that require a new capture or calibration target;
- a minimal experiment that can falsify the diagnosis;
- recommended pipeline ordering;
- algorithm and parameter suggestions;
- memory complexity and a tiling strategy;
- licensing implications if suggesting existing source code;
- failure cases and an explicit promotion gate.

The most valuable answer is not a visually pleasing one-off edit. It is a
falsifiable explanation and a bounded unattended correction that preserves
legitimate color across other cameras and scenes.
