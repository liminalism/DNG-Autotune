## Assessment

Yes—scene understanding is the next logical layer for this project. But I would **not** implement it as a single Samsung-style enum such as:

```text
Nature
Indoor
Sky
Food
Macro
Portrait
```

That abstraction is too coarse for an autotuner. A photograph can simultaneously be:

```text
outdoor + landscape + sky + water + sunset + backlit + person
```

The useful architecture is:

1. A lightweight perception system produces **overlapping probabilities and soft spatial masks**.
2. The existing physically grounded pipeline measures RAW clipping, noise, luminance, color and focus within those regions.
3. A deterministic controller makes **small, bounded adjustments**.
4. Low confidence means the existing pipeline runs unchanged.

Samsung has publicly described Scene Optimizer as a deep-learning system that examined objects, background scenery and time-related characteristics, categorized scenes into 20 types, and adjusted saturation, white balance, brightness and contrast. Apple has similarly described scene detection as applying a tailored look. Neither company publicly documents its complete current camera pipeline or exact model architecture, so the part worth copying is the behavior—not an assumed Samsung network. ([Samsung Global Newsroom][1])

Your project is already well beyond a primitive RAW converter. It has:

* Continuous RAW clip confidence and reconstruction uncertainty.
* Harmonic pre-demosaic highlight reconstruction.
* A proper DNG matrix color path.
* Noise measurement.
* Preview-guided exposure.
* Automatic night detection and local tone mapping.
* Noise-aware denoising and sharpening.
* Highlight desaturation and gamut compression.

The present controller’s main limitation is exactly what `docs/KNOWN_LIMITATIONS.md` says: it cannot identify people, faces, sky, foreground, snow, documents or intended subjects. In `src/analyze.rs:326-468`, it samples global luminance statistics plus a central rectangle. The “subject” estimate is currently:

```rust
0.60 * center_median + 0.40 * global_median
```

That is a sensible fallback, but it is the point where semantic evidence should now enter.

---

# 1. Build a perception layer, not a scene-preset system

I recommend dividing perception into three classes of evidence.

## A. Capture-condition scores

These are global or nearly global, and should be independent probabilities rather than mutually exclusive labels:

```text
indoor
outdoor
backlit
high-key
low-key
sunset/dawn
artificial-light/neon
mixed-illuminant
low-light
haze/fog
close-up
shallow-depth-of-field
document-like
```

Some of these can be partly inferred without a model. For example, low light is already inferred from luminance, ISO and sensor noise. The model should supplement that physical evidence, not replace it.

## B. Spatial semantic regions

These are more useful than broad classifications because they tell the processor **where** an adjustment is safe:

```text
face/person
sky/cloud
foreground or salient subject
vegetation
water
snow/sand
building/interior
window
text/document
artificial light source
```

A sky label without a sky mask is of limited value. You cannot safely apply sky-ground HDR, gradient protection or sky-specific anomaly detection unless you know where the sky is.

## C. Physical risk maps

These should remain independent from the CNN:

```text
RAW clip confidence
highlight-reconstruction uncertainty
noise/SNR
local sharpness and blur
high-contrast edge strength
purple-fringe probability
gamut excursion
local luminance percentiles
```

The semantic model says, “this is probably sky.” The physical system says, “this portion is partially clipped, smooth, high-luminance and exhibiting an anomalous magenta displacement.” A correction should generally require both.

That separation will prevent a scene classifier from turning every purple sunset neutral or every bright surface into blue sky.

---

# 2. Recommended lightweight model stack

I would start with **separate models**, because they are easier to validate and replace. Once the labels and policies stabilize, they can be merged into a shared multi-task backbone.

| Purpose                         | Recommended first choice            | Alternative             | Reason                                                                  |
| ------------------------------- | ----------------------------------- | ----------------------- | ----------------------------------------------------------------------- |
| Face detection                  | **YuNet**                           | BlazeFace full-range    | Ready ONNX model, permissive implementation, detects quite small faces  |
| Global multi-label scene scores | **MobileOne-S0 or S1**              | MobileNetV4 Conv-Small  | Simple fast inference graph; plenty of capacity for 10–20 custom scores |
| Coarse semantic segmentation    | **MobileNetV3 + LR-ASPP**           | Fast-SCNN or BiSeNetV2  | Produces useful masks without a large model                             |
| Runtime                         | **ONNX Runtime through Rust `ort`** | Abstract behind a trait | Good operator coverage, static INT8 and multiple hardware backends      |

YuNet is specifically intended as a lightweight face detector, supports small faces, and the OpenCV Zoo provides ONNX and quantized variants. BlazeFace remains a reasonable alternative, particularly its full-range back-camera model, but YuNet is likely the shortest route for this Rust application. ([GitHub][2])

MobileOne-S0 and S1 are straightforward convolutional backbones whose deployment graph is reparameterized into a simple inference form. MobileNetV4 is a newer family designed across mobile CPU, GPU, DSP and accelerator targets, but I would initially favor a convolution-only configuration rather than introducing attention operators for a classifier this small. ([GitHub][3])

For segmentation, MobileNetV3-LR-ASPP is probably the best starting balance. Fast-SCNN is a leaner option, while BiSeNetV2 is appropriate when boundary quality begins to matter more than absolute model simplicity. Their published benchmarks are not directly comparable to your custom 384–512-pixel workload, but all three were explicitly designed for efficient dense prediction. ([arXiv][4])

Places365 can provide useful scene-pretraining or teacher signals—it contains roughly 1.8 million images over 365 scene categories and also exposes indoor/outdoor and scene-attribute predictions—but I would not ship a 365-way Places classifier as your controller. Remap or distill it into your smaller actionable taxonomy. Dataset and weight licenses should be audited independently before distribution. ([GitHub][5])

ONNX Runtime supports static and dynamic INT8 quantization and multiple execution providers. For this project, establish a deterministic CPU path first, then optionally enable CUDA, DirectML or another provider. The Rust `ort` crate provides the integration layer. ([ONNX Runtime][6])

## Do not start with a large model

At 384 or 512 pixels, one small classifier, one small segmenter and a face detector should be cheap relative to demosaicing and full-resolution image operations. A much larger vision model would not necessarily produce better rendering because the difficult part is not recognizing “forest.” It is correctly translating uncertain perception into bounded processing.

Spend the additional desktop compute on:

* Better spatial masks.
* More reliable region statistics.
* Edge-aware local processing.
* Camera-specific color transforms.
* Better CA/fringe correction.
* Optional multi-pass analysis.

Do not spend it primarily on increasing the classifier parameter count.

---

# 3. Feed models a fixed analysis render, not the RAW mosaic

Ordinary pretrained CNNs expect something resembling display-referred RGB. Feeding them Bayer data or unbounded scene-linear RGB without retraining will produce poor and unstable results.

Create a dedicated **semantic analysis proxy**:

```text
RAW decode
→ lens correction
→ demosaic
→ global as-shot white balance
→ camera-to-working-space conversion
→ orientation
→ chroma denoise
→ fixed neutral analysis tone map
→ sRGB transfer
→ resize to 384 or 512 pixels
```

The model proxy must use a **fixed rendering policy**. It must not depend on the semantic decisions it is about to make, or you create a feedback loop:

```text
classifier says sunset
→ rendering becomes warmer
→ classifier becomes more confident it is sunset
→ rendering becomes still warmer
```

A fixed shoulder that preserves bright sky, moderate shadow lift and no creative saturation is sufficient. It does not need to be attractive; it needs to be stable and recognizable.

## Exact insertion point in this project

The strongest insertion point is in `src/pipeline.rs` after chroma denoising and before local white balance:

* Chroma denoise: `src/pipeline.rs:555-574`
* Current local white balance: `src/pipeline.rs:586-604`
* Current global analysis: `src/pipeline.rs:620-633`

That order would become:

```text
chroma denoise
→ build neutral semantic proxy
→ run perception
→ use semantic evidence to gate local WB
→ compute region-aware physical statistics
→ solve exposure/tone/local processing
```

Semantic inference needs to precede local white balance because some of its most important jobs are:

* Excluding sunsets, neon, screens and colored lamps from illuminant estimation.
* Using faces or plausible neutral surfaces as anchors.
* Distinguishing an actual mixed-light cast from an intentionally colored scene.

After local WB, recompute the physical region statistics from scene-linear pixels. The masks need not be regenerated.

## Preserve low-resolution masks

Do not allocate full-resolution floating-point semantic masks for a 50-megapixel image.

Store masks as low-resolution `u8` probabilities, for example:

```text
384 × 256 × 10 classes ≈ 1 MB
```

Sample or edge-aware-upsample them inside the local processing stage. Preserve them as soft probabilities rather than hard binary regions. Hard masks will produce visible exposure, sharpening and color boundaries.

---

# 4. Rework the embedded-preview path

`src/preview.rs` currently combines two separate concerns:

1. Can the embedded preview serve as a reliable exposure oracle?
2. Can the embedded preview be decoded at all?

The minimum area threshold of 250,000 pixels is appropriate for rejecting a tiny thumbnail as an exposure reference, but a 256×191 thumbnail can still distinguish:

* Indoor versus outdoor.
* A large face.
* Sky-dominated versus room-dominated.
* Sunset versus daylight.
* Document versus ordinary photograph.

At present, `preview::read()` decodes the RGB image, reduces it to `PreviewOracle`, and discards the pixels. Split this into:

```rust
read_preview_rgb(...)
measure_preview_oracle(...)
build_semantic_preview(...)
```

Then maintain two eligibility decisions:

```text
semantic_eligible = decoded and plausible
exposure_oracle_eligible = sufficiently large and trustworthy
```

The embedded JPEG can be particularly valuable for global classification because it already has a recognizable camera rendering. The RAW-generated proxy should remain the source for aligned segmentation masks because the preview may have different cropping, distortion correction or orientation.

For difficult HDR scenes, a later custom model could receive two neutral RAW proxies:

* A midtone-oriented render.
* A highlight-preserving render.

That would let it see both a backlit subject and sky without depending on the final tone map. I would not start there; use a single nonclipping proxy first.

---

# 5. Make scene output explicitly multi-label

Do not use a softmax classifier where one label wins. Use sigmoid outputs such as:

```rust
struct SceneScores {
    indoor: f32,
    outdoor: f32,
    backlit: f32,
    high_key: f32,
    sunset: f32,
    artificial_light: f32,
    mixed_light: f32,
    closeup: f32,
    shallow_dof: f32,
    document: f32,
    portrait: f32,
    landscape: f32,
}
```

A separate segmentation head can emit:

```rust
struct RegionMasks {
    face: Mask,
    person: Mask,
    sky: Mask,
    vegetation: Mask,
    water: Mask,
    snow_or_sand: Mask,
    building_or_interior: Mask,
    text_or_document: Mask,
    salient_foreground: Mask,
}
```

Then calculate physical measurements per region:

```rust
struct RegionStats {
    area_fraction: f32,
    p10_ev: f32,
    p50_ev: f32,
    p90_ev: f32,
    clipped_fraction: f32,
    reconstruction_uncertainty: f32,
    mean_chroma: f32,
    noise_level: f32,
    sharpness: f32,
}
```

The resulting high-level object should contain both model provenance and evidence:

```rust
struct SceneEvidence {
    scores: SceneScores,
    masks: RegionMasks,
    regions: Vec<RegionStats>,
    focus: FocusEvidence,
    model_hashes: ModelHashes,
}
```

The sidecar should record:

* Model SHA-256.
* Model/input-preprocessing version.
* Raw scores, not merely winning labels.
* Region area fractions.
* Every policy adjustment made.
* Confidence or veto responsible for the adjustment.
* Whether inference failed and the legacy controller was used.

This matters for reproducibility. It also makes scene behavior debuggable rather than mysterious.

---

# 6. Replace the center prior with a subject hierarchy

The current center weighting is a reasonable fallback but should no longer be the main subject estimator.

Use this priority order:

```text
reliable face
→ reliable person
→ salient foreground
→ document/text region
→ composition-weighted foreground
→ existing center prior
```

A face should not automatically determine global exposure. Instead calculate relationships:

```text
face median EV
background median EV
sky median/highlight EV
face noise
face clipping
available highlight headroom
```

A backlit portrait is not “make the entire image brighter.” It is:

```text
face is 2.2 EV below background
sky has limited headroom
therefore:
    local face lift = +0.30 EV
    mild global adjustment = +0.05 EV
    daylight local tone = moderate
    protect highlights
```

That is much closer to computational photography than applying a portrait preset.

---

# 7. Initial policy rules

Every semantic adjustment should be smooth, confidence-weighted and bounded:

```text
weight =
    calibrated_model_confidence
    × region_area_gate
    × physical_evidence_gate
    × veto_gate

adjustment = clamp(weight × proposed_adjustment, lower_limit, upper_limit)
```

The confidence should be calibrated on a validation set. Raw neural-network probabilities are frequently overconfident and should not be treated as literal probabilities.

## Face and backlit subject

Use face detection to improve:

* Subject exposure.
* Local denoising.
* Sharpening protection.
* Skin gamut handling.
* Mixed-light WB anchoring.

Safe initial limits:

```text
global semantic exposure change: no more than about ±0.25 EV
local face lift: no more than about +0.35 EV
face sharpening: no stronger than current clean-frame setting
face denoise: preserve eyes, brows, lips and hair boundaries
```

The exact values should come from corpus testing, but the first release should be deliberately conservative.

## Sky and daylight HDR

Sky detection is probably the highest-value addition after faces.

Measure:

```text
sky p50/p90 EV
foreground p50/p90 EV
sky clipped-channel fractions
sky reconstruction uncertainty
sky area
sky texture level
sky-to-foreground EV gap
```

Then activate daylight local tone smoothly, for example:

```text
gap below roughly 2.0 EV: no semantic HDR
gap around 2.5–3.0 EV: begin ramp
larger gap: increase strength subject to clipping/noise
```

The model should not instruct the renderer to “make the sky blue.” Its useful jobs are:

* Permit sky-ground exposure balancing.
* Protect smooth gradients from local contrast and sharpening.
* Prevent a bright sky from controlling subject exposure.
* Distinguish sky highlights from lamps, paper or reflective objects.
* Provide a guard for high-luminance anomaly detection.

The final local tone strength should remain constrained by RAW headroom and reconstruction confidence.

## High-key snow, beach and pale interiors

Your current percentile-based controller can interpret a genuinely high-key scene as overexposed and pull it toward gray. Scene evidence should relax that behavior.

For confident snow/beach/high-key scenes:

* Keep the target median intentionally bright.
* Use clipping evidence, not global brightness alone, to decide whether to darken.
* Avoid over-neutralizing blue shadows in snow.
* Keep specular snow highlights distinct from broad clipped areas.
* Allow a higher oracle/controller target ceiling than an ordinary scene.

This is a better use of scene classification than changing saturation.

## Sunset, stage light and neon

These should function primarily as **vetoes**:

* Veto aggressive local WB neutralization.
* Veto broad “pink highlight correction.”
* Reduce automatic highlight desaturation where color is physically supported.
* Preserve warm sky-to-ground relationships.
* Treat colored light sources as subjects, not casts.

This is especially important because a sky mask alone cannot tell the difference between a defective lavender sky and a legitimate purple sunset.

## Indoor mixed lighting

Semantic masks can substantially improve the existing local-WB algorithm.

Exclude or strongly downweight:

```text
lamps and light emitters
windows and sky
screens
highly saturated objects
vegetation
faces with strong makeup or theatrical light
```

Prefer:

```text
low-chroma walls
paper
known neutral objects
well-exposed skin as a weak anchor
large consistent surfaces
```

The local-WB stage should still make the final decision from measured chromatic clusters. The model tells it which samples are plausible illuminant evidence.

## Documents and text

A document mode can safely influence:

* Neutral WB.
* Exposure uniformity.
* Reduced vibrance.
* Stronger local text contrast.
* Noise-aware sharpening.
* Suppression of broad photographic local-tone effects.

This is more actionable than a generic “indoor” class.

## Nature and foliage

I would not allow `nature` to produce a large green or blue saturation adjustment. “Nature” includes forests, deserts, snow, cloudy mountains, waterfalls and sunsets, which need different processing.

Use vegetation and water masks mainly for:

* Region statistics.
* Texture preservation.
* Gamut monitoring.
* Small camera-look adjustments once the profile layer is implemented.

Broad scene-dependent saturation should initially be limited to roughly a few percent. Larger color differences belong in the camera-profile or look-transform layer.

---

# 8. Macro and close-up detection

You can detect **close-up appearance** from image data. You usually cannot prove true optical macro magnification from pixels alone.

Call the output:

```text
closeup_score
shallow_dof_score
```

rather than:

```text
is_macro
```

A tightly cropped distant object, a telephoto portrait and a true 1:1 macro image can have similar image structure.

## Metadata evidence

`src/shotinfo.rs:19-74` currently exposes only:

```text
ISO
exposure time
f-number
```

However, `src/metadata.rs` already handles lens model and focal length for other purposes. Surface those values into the analysis layer, along with these where available:

```text
lens model/ID
focal length
35 mm equivalent
f-number
focus or subject distance
sensor/crop dimensions
camera make/model
```

With a lens database containing minimum focus distance and approximate magnification behavior, metadata can provide a useful macro prior. MakerNote focus distances will be camera-specific and frequently unavailable, so treat them as optional evidence.

## RAW-domain focus evidence

This is one place where direct RAW analysis is worthwhile.

Build a half-resolution focus-energy map from the Bayer green samples before demosaic. Green contains most of the luminance resolution and is less affected by color interpolation. Use multiscale gradient or structure-tensor energy, not only a single Laplacian variance.

Measure:

* Size of the largest sharp connected region.
* Distance of the sharp region from the frame boundary.
* Foreground-to-background sharpness ratio.
* How quickly sharpness falls away around the subject.
* Whether the sharp region occupies a large fraction of the frame.
* Whether the background has smooth bokeh-like low-frequency structure.

Then combine it with semantic and metadata evidence:

```text
closeup_score =
    metadata prior
    + subject fill
    + foreground/background sharpness separation
    + shallow-DOF evidence
    + flower/food/insect/product evidence
```

A later optional relative-depth model could help, but it is not necessary for the first version.

## What macro detection should change

Macro should initially influence spatial processing, not color:

* Stronger denoise in the out-of-focus background.
* Lower or zero sharpening in bokeh.
* Normal or slightly stronger sharpening on the in-focus subject.
* Reduced local-tone strength across narrow subject/bokeh boundaries.
* Protection of small specular highlights.
* More conservative hot-pixel removal around genuine tiny highlights.
* Avoidance of halos around hairs, stems and insect edges.

A generic “macro saturation look” would likely cause more harm than benefit.

---

# 9. The white/pink blowout is not fundamentally a scene-detection problem

Your own `docs/PURPLE_SKY_PROBLEM_SCOPE.md` provides strong evidence for this.

For `_DSC1289`, about 99.2% of the selected lavender sky population was below the RAW-domain reconstruction clip threshold. Along `_DSC1283`’s purple ridge, about 95% was also below it. An exact lens profile reduced the edge-localized purple population from 0.59% to 0.34%, while the broad sky green deficit barely changed, from 13.8% to 13.6%.

That suggests at least two different defects:

1. **Edge-localized purple/fringing**, partly optical or spatial.
2. **Broad bright-sky color error**, likely involving camera color rendering, highlight/tone behavior, flare or gamut behavior rather than only clipped-channel reconstruction.

Scene detection will help distinguish sky from other bright objects, but it should not be treated as the correction itself.

## Split the defect into explicit origins

Add a diagnostic classification to the sidecar:

```text
raw_fully_clipped
raw_partially_clipped
reconstruction_uncertain
edge_fringe
profile_or_matrix_gamut
broad_highlight_color_anomaly
unknown
```

For each suspicious region, save or optionally dump:

* RAW clip confidence.
* Reconstruction uncertainty.
* Sky probability.
* Edge strength.
* Radial position.
* Local chroma and hue.
* Pre-tone and post-tone RGB.
* Gamut excursion.
* Whether the anomaly existed before tone mapping.

This will stop unrelated failure modes being treated as one “pink sky” problem.

## Edge-localized purple

This needs a dedicated correction path:

* Image-driven RAW CA estimation before demosaic.
* Residual post-demosaic purple-fringe correction.
* Lens-profile correction where available.
* Strong edge, radial and local-color safeguards.

The semantic sky mask can raise confidence that a magenta band along a dark ridge is aberration, but the actual correction should be driven by channel displacement and edge evidence.

## Broad bright-sky cast

The project currently implements the colorimetric DNG matrix path but not DCP HueSatMap, LookTable, vendor picture styles or a trained camera-look transform, as noted in `docs/KNOWN_LIMITATIONS.md:13-53`.

That missing layer is likely more important for achieving a Samsung/Apple-like result than adding dozens of scene classes.

I would implement one of these:

### Preferred standards-oriented path

Support:

```text
DCP HueSatMap
DCP LookTable
DCP ProfileToneCurve
```

This gives you camera-specific, hue- and luminance-dependent rendering without confusing it with scene classification.

### Data-driven path

Fit a smooth camera-specific 3D look transform from your RAW/camera-JPEG pairs:

1. Produce a neutral base render.
2. Align it with the camera JPEG.
3. Exclude clipped pixels, strong edges and regions with major local-tone differences.
4. Fit a smooth regularized LUT with a strong identity prior.
5. Separate color mapping from tone mapping.
6. Validate on held-out shooting sessions and cameras.

Do not fit one unconstrained transform directly from base RAW to final JPEG; it will absorb local tone, sharpening, noise reduction and scene decisions into a color LUT.

## Semantic anomaly guard

Once the physical corrections and profile layer exist, a final bounded guard can use:

```text
high sky probability
+ high luminance
+ low texture
+ anomalous magenta/green relationship
+ low sunset/neon probability
+ low RAW reconstruction uncertainty
```

to apply a small chroma/hue correction.

It should not run merely because a pixel is bright and in the sky. That would erase legitimate dawn, dusk, storm and pollution colors.

For fully three-channel-clipped pixels, the true color and texture are absent. The correct behavior is a smooth, uncertainty-marked neutral or boundary-informed fallback—not pretending the original sky color can be recovered.

---

# 10. Other high-value improvements to the autotuner

## Refactor the duplicated pipeline first

`src/pipeline.rs` and `src/api.rs` currently contain substantially duplicated development and rendering sequences. Adding semantic inference, masks and policy decisions to both will create drift quickly.

Extract something similar to:

```text
decode_and_develop()
build_analysis_context()
solve_controller()
render_frame()
```

Both CLI and library API should call the same core. This is a prerequisite, not cleanup for later.

## Add automatic daylight HDR

Night HDR is already being activated from `low_light_score`. Daylight HDR should be activated from region relationships rather than the current global `HighDynamicRange` classification.

Use:

```text
sky-to-foreground EV difference
face-to-background EV difference
highlight clipping and uncertainty
foreground noise and lift headroom
sky and subject area
```

This directly addresses the sky-versus-ground problem without producing the gradient artifact caused by broad or poorly localized tone manipulation.

## Make denoise and sharpening semantic

The present noise model is already useful. The missing part is texture intent.

Examples:

```text
face:
    denoise skin smoothly
    preserve eyes/hair
    moderate sharpening

sky:
    suppress chroma blotching
    suppress sharpening and local contrast
    protect gradients

foliage:
    preserve high-frequency structure
    avoid turning noise into leaves

bokeh:
    stronger denoise
    no sharpening

document:
    stronger edge-preserving luma cleanup
    stronger text sharpening
```

Semantic decisions should scale the existing operators rather than introduce entirely separate filters.

## Convert the camera preview from authority into a prior

The preview oracle currently gives excellent average subject exposure on the paired corpus, but `docs/KNOWN_LIMITATIONS.md` correctly notes that this largely inherits the camera’s judgement.

That was appropriate for the goal “at least as good as the camera JPEG.” It becomes limiting when the goal changes to “better than the camera JPEG.”

Eventually use:

```text
independent controller result
camera-preview prior
semantic subject estimate
physical clipping/noise limits
```

as separate signals. The preview should be allowed to influence the result, but not automatically dominate it.

## Avoid naive candidate-render scoring

Do not generate several renders and choose whichever maximizes entropy, local contrast, sharpness or another no-reference metric. Such metrics commonly prefer:

* Excessive microcontrast.
* Visible noise.
* Oversharpening.
* Day-like night rendering.
* Saturation and clipping.

Candidate rendering is useful only when the selector is a reliable human-trained preference model or a one-sided physical guard. For example, a second render can be accepted when it reduces verified clipping without materially altering subject exposure—not because its gradient score is higher.

## Preserve sequence consistency

For bursts or adjacent captures, abrupt semantic thresholds will make the autotuner appear unstable.

Record capture timestamp, camera, lens and visual similarity. For likely related images:

* Keep scene-score interpretation consistent.
* Avoid a one-frame switch between indoor and outdoor.
* Stabilize WB and color-look selection.
* Permit exposure differences when the actual content changed.

This does not require video-style temporal processing; it can be a small consistency prior over related files.

---

# 11. Practical implementation sequence

## First slice: perception infrastructure

1. Refactor CLI and API onto one processing core.
2. Add `SemanticProxy`, `SceneEvidence`, `RegionStats` and model provenance.
3. Produce a fixed 384- or 512-pixel neutral sRGB proxy after chroma denoise.
4. Preserve embedded-preview RGB separately from exposure-oracle eligibility.
5. Add an optional ONNX feature with a deterministic CPU fallback.
6. Add mask and score overlays to stage dumps.

At this point, inference should make **no rendering changes**. Run it over the corpus and inspect scores and masks.

## Second slice: three high-confidence interventions

Wire only these:

1. **Face/backlight-aware subject placement**
2. **Sky/foreground-driven daylight HDR**
3. **Sunset/neon/semantic vetoes for local white balance**

These have clear intended behavior and limited interaction with the rest of the pipeline.

The output must remain byte-identical to the old controller when:

* Models are unavailable.
* Confidence is below threshold.
* Masks are implausible.
* The semantic feature is disabled.

## Third slice: macro and local detail

1. Add Bayer-green multiscale focus analysis.
2. Surface focal length, lens model and optional focus distance.
3. Produce `closeup_score` and `shallow_dof_score`.
4. Apply subject/background denoise and sharpening maps.
5. Add boundary halo tests.

## Fourth slice: color and highlight correctness

1. Implement DCP creative tables or a regularized camera-look LUT.
2. Add data-driven RAW CA.
3. Add residual post-demosaic fringe correction.
4. Add the bounded high-luminance sky anomaly guard.
5. Keep highlight reconstruction uncertainty as the primary safety signal.

## Fifth slice: custom multi-task model

Once the evidence taxonomy is stable:

* Train one MobileOne or MobileNetV4-derived backbone.
* Add a global multi-label head.
* Add a coarse segmentation head.
* Add close-up/backlit/mixed-light auxiliary heads.
* Distill from larger offline teachers.
* Quantize using calibration images generated by the actual semantic-proxy pipeline.

Training and validation should be split by camera and shooting session, not random individual frames. Otherwise nearly identical images will leak between training and validation and make the model look much more reliable than it is.

---

# 12. Evaluation gates

The acceptance corpus should explicitly contain:

| Category                   | Required behavior                                          |
| -------------------------- | ---------------------------------------------------------- |
| Backlit face               | Face visible without globally blowing the background       |
| Bright sky and dark ground | Natural relationship, no broad gradient or halo            |
| Snow/beach/high-key        | Remains bright rather than gray                            |
| Sunset/neon/stage          | Color preserved; no neutralization                         |
| Indoor mixed light         | Cast correction where appropriate, colored lights retained |
| Macro with bokeh           | Subject sharp, background clean and unsharpened            |
| Smooth blue sky            | No lavender cast, banding or enhanced noise                |
| Dark ridge against sky     | Purple fringe reduced without edge desaturation            |
| Document                   | Neutral, clear text, restrained photographic processing    |
| No semantic confidence     | Exact legacy fallback                                      |

Track region-level measurements rather than only whole-image metrics:

```text
face exposure error
sky/foreground EV gap
sky hue and chroma stability
edge-localized purple population
output clipping by semantic region
face skin gamut excursions
mask-boundary halo score
background noise after macro processing
adjacent-shot parameter consistency
runtime and peak memory
```

The model should be judged by whether it improves these rendering outcomes, not merely classification accuracy.

## Bottom line

The right next step is **coarse segmentation plus face detection**, not a large 20-class scene classifier. Sky, face, foreground, document, artificial-light and focus-region masks immediately improve exposure, HDR, WB, denoise and sharpening. A global MobileOne/MobileNetV4 classifier can then add backlit, high-key, sunset, mixed-light and close-up scores.

For the remaining white/pink defects, scene detection should serve as a safety gate. The root work remains camera-look profiling, RAW/residual CA correction and explicit diagnosis of where the color error enters the pipeline. The fastest route toward a Samsung/Apple-quality result is therefore:

```text
semantic masks
+ region-aware controller
+ automatic daylight HDR
+ camera-specific look transform
+ residual CA/fringe correction
```

—not a generic `nature → greener` scene preset.

[1]: https://news.samsung.com/us/stellar-shots-every-time-galaxy-note9s-intelligent-camera "Stellar Shots Every Time: The Galaxy Note9’s Intelligent Camera"
[2]: https://github.com/opencv/opencv_zoo/blob/main/models/face_detection_yunet/README.md "opencv_zoo/models/face_detection_yunet/README.md at main · opencv/opencv_zoo · GitHub"
[3]: https://github.com/apple/ml-mobileone "GitHub - apple/ml-mobileone: This repository contains the official implementation of the research paper, \"An Improved One millisecond Mobile Backbone\" CVPR 2023. · GitHub"
[4]: https://arxiv.org/abs/1905.02244 "[1905.02244] Searching for MobileNetV3"
[5]: https://github.com/csailvision/places365 "GitHub - CSAILVision/places365: The Places365-CNNs for Scene Classification · GitHub"
[6]: https://onnxruntime.ai/docs/performance/model-optimizations/quantization.html "Quantize ONNX models | onnxruntime"
