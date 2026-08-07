## Diagnosis

Treat this as **two separate problems, in this order**:

1. **The autotuner is manufacturing false sky chroma during highlight reconstruction.**
2. **The scene may still benefit from HDR-like local tone mapping to balance sky and ground.**

The cyan-to-violet gradient is mainly the first problem. Adding more “HDR” before fixing it will preserve, reveal, or exaggerate the false color.

This is no longer principally a final clamping failure. For `_DSC1289`, the rendered-output `clipped_fraction` is only `0.000004`, or **0.0004%**, even though 6.29% of samples are near white. `_DSC1290` similarly reports only **0.0008%** clipped. The output is therefore not being visibly ruined by large areas reaching final white; it is being ruined while those highlights are reconstructed and color-mapped.  

The older clamping diagnosis was valid for the Rawler path: it discarded negative and above-one scene-linear values before your own tone mapping could use them. The owned path corrected that. Do not undo it and do not add an earlier clamp.

## Why the current code creates the gradient

The current reconstruction has three properties that are structurally dangerous for a smooth sky:

* A channel becomes “clipped” at one hard demosaiced threshold: `0.98`.
* The algorithm changes to a different rule when the number of clipped channels changes from one to two.
* Two or three clipped channels are treated as evidence that the pixel is “near white,” and that branch always runs at full strength, regardless of the configured `0.75` strength.

That makes an exposure gradient through a sky become a **processing-regime gradient**:

```text
no clipped channel
        ↓
one clipped channel: survivor-based hue estimate, strength 0.75
        ↓
two clipped channels: presumed achromatic, different anchor, strength 1.0
        ↓
three clipped channels: no measured chromaticity remains
```

Those boundaries naturally follow broad luminance contours in the sky. Even if the original sky varies smoothly, the reconstruction rule does not. A synthetic constant-chromaticity ramp through the present algorithm develops abrupt hue changes as it crosses the one-to-two-channel boundary.

The reports strongly support this explanation:

* `_DSC1289` has **958,928 pixels** with at least one clipped camera channel.
* Of those, **344,861** have two or more clipped channels and **614,067** have exactly one.
* It has **zero fully clipped pixels**, so this is not a large region where every channel is simply gone.
* `_DSC1290` has **801,061 clipped pixels**, including **682,786 multi-channel-clipped pixels**, but only **5,828 fully clipped pixels**.  

So most of the troublesome sky is divided between two differently processed populations, not irretrievably all-channel white. The exact color of a two-channel-clipped pixel is not stored in that pixel, but there is enough neighboring and surviving-channel information to do substantially better than a binary “colored versus white” decision.

The latest changelog effectively acknowledges the remaining split: the new rule corrected the two-or-more-channel cohort, while the one-channel cohort remained unchanged. It also notes that remaining purple in `_DSC1289` comes partly from one-channel-clipped pixels.

There is another amplifier. The Auto preset uses:

* `saturation = 1.22`
* `highlight_desaturation = 0.16`

At the very top of the shoulder, the approximate net chroma factor is:

[
1.22 \times (1 - 0.16) = 1.0248
]

So even at maximum highlight desaturation, Auto approximately preserves the original chroma instead of materially reducing it. Slightly below the top, it still boosts chroma. Any cyan or violet error produced by reconstruction therefore survives almost intact.

## First step: prove the stage responsible

Your existing `--dump-stages` output begins after `develop()`, which is already after highlight reconstruction. It cannot currently show whether the bad hue originated in reconstruction or in final gamut mapping.

Add these diagnostic outputs inside the owned color path:

1. **Camera RGB immediately before highlight reconstruction.**
2. **A clip-state map**, with separate values for zero, one, two, and three clipped channels.
3. **Camera RGB immediately after reconstruction.**
4. **Reconstruction delta**, preferably both luminance delta and OKLab chroma/hue delta.
5. **Display-linear RGB immediately before `compress_gamut`.**
6. **Display-linear RGB immediately after `compress_gamut`.**

For the clip-state map, a simple diagnostic encoding is sufficient:

```text
black   = no channel clipped
red     = one channel clipped
magenta = two channels clipped
white   = three channels clipped
```

The decisive test is whether the cyan-to-violet boundary in `_DSC1289_auto.jpg` follows the red-to-magenta boundary in that map. I expect that it will.

Also run this small ablation sweep on the same RAW and write lossless PNGs:

| Variant                                  | What it establishes                                                              |
| ---------------------------------------- | -------------------------------------------------------------------------------- |
| `--highlight-reconstruction 0`           | Whether the entire gradient is introduced by reconstruction                      |
| reconstruction `0.25`, `0.75`, `1.0`     | Whether only the one-channel area changes while the two-channel area stays fixed |
| default + `--working-space rec2020`      | How much narrow-gamut intermediate processing contributes                        |
| default + `--highlight-contrast 1.2`     | Whether brightening merely masks the color by pushing it toward white            |
| reconstruction off + `--local-tone 0.35` | Whether sky/ground balance can improve without the reconstruction artifact       |

The CLI ablation is only preliminary because turning reconstruction off also changes the image that analysis sees and may slightly change the selected tone parameters. For the definitive comparison, render the pre- and post-reconstruction buffers using the **same frozen `ToneParams`**.

## Correct reconstruction architecture

The central correction is to separate three concepts that are currently conflated:

* **Clip state:** objective sensor evidence.
* **Achromatic probability:** an inference from spatial context.
* **Reconstruction policy:** what values to synthesize.

Two clipped channels mean “the original ratio is unavailable.” They do **not** mean “the original pixel was white.”

### 1. Detect clipping before demosaic

Do not infer clipping solely from a demosaiced channel being above `0.98`.

Demosaicing can:

* pull a saturated photosite below the threshold by averaging it with neighbors;
* spread a saturated sample into nearby reconstructed channels;
* make the exact region depend on which demosaic algorithm Auto selected.

Instead:

* build per-CFA-site clipping confidence from the normalized raw sample and its actual channel white level;
* preserve that mask through hot-pixel correction;
* demosaic or propagate the confidence alongside camera RGB.

A continuous confidence is preferable to a Boolean threshold:

[
c_i = \operatorname{smoothstep}(t_0,t_1,x_i)
]

where the thresholds are selected in raw code space near the real white level. The exact values should be based on the camera’s white-level behavior, not permanently fixed at `0.98`.

### 2. Estimate chromaticity from trusted neighboring pixels

For pixels with clipped channels, estimate a low-frequency local camera-space chromaticity from nearby unclipped pixels. Log ratios work well because they separate color from intensity:

[
u=\log\frac{R+\epsilon}{G+\epsilon},
\qquad
v=\log\frac{B+\epsilon}{G+\epsilon}
]

Filter `u` and `v` with confidence weighting, excluding clipped channels. Use an edge-aware or guided filter so foliage edges do not bleed green into the sky.

Given the resulting local chromaticity vector (q=[q_R,q_G,q_B]), solve an intensity (s) from the surviving channels:

[
s^* =
\arg\min_s
\sum_{i\in U} w_i(sq_i-x_i)^2
]

where (U) is the set of trusted channels. Then reconstruct only the clipped channels toward (s^*q), constrained so reconstruction never contradicts the observed lower bound at saturation.

This naturally handles the cases:

* **One channel clipped:** two survivors strongly constrain intensity and validate the local color prior.
* **Two channels clipped:** one survivor supplies intensity; spatial chromaticity supplies the missing ratios.
* **Three channels clipped:** neither chromaticity nor exact intensity exists at that pixel. Extend low-frequency color and luminance from the component boundary, but do not invent detail.

### 3. Blend continuously rather than switching by clip count

The reconstructed result should be blended according to per-channel clip confidence and prior reliability, not one discrete branch:

[
x'_i =
(1-c_i)x_i + c_i,\hat{x}_i
]

A second confidence should describe how trustworthy the local chromaticity estimate is. Near a valid boundary it can preserve the estimated sky blue. Deep inside a large fully clipped component it should gradually reduce chroma toward neutral.

This eliminates the visible one-channel/two-channel processing boundary.

### 4. Carry reconstruction uncertainty into the tone renderer

Propagate one compact per-pixel uncertainty value into rendering. Use it to limit chroma only where color was synthesized:

[
C_{\text{gain}} =
C_{\text{normal}}
\left(1-u,d_{\text{uncertain}}\right)
]

This is preferable to globally increasing `highlight_desaturation`, which would flatten legitimate saturated highlights throughout the corpus.

A useful first implementation would be:

* reliable local color prior: retain most reconstructed chroma;
* one clipped channel with weak prior: mild chroma cap;
* two clipped channels: stronger cap;
* all channels clipped: strong smooth neutralization.

Do not make the cap itself dependent on a hard clip count; interpolate through the uncertainty value.

## Fix gamut mapping after reconstruction

The current renderer scales RGB around mapped luminance and then clamps each channel into `[0,1]`. This is continuous, but it is not perceptually hue-constant. A cyan or violet reconstruction error can shift further as a bright color approaches the irregular boundary of sRGB.

After reconstruction is continuous, replace or supplement `compress_gamut` with a perceptual chroma compressor:

1. Convert display-linear RGB to OKLab.
2. Preserve lightness and hue.
3. Reduce chroma until conversion back to sRGB is in gamut.
4. Use a smooth onset before the boundary rather than waiting for an out-of-range channel.

A binary search over OKLab chroma is adequate initially. Optimize it with a cusp approximation only after correctness is established.

This will not recover a wrong hue created upstream, but it should stop final gamut fitting from turning a small upstream difference into a visible sky band.

## Where HDR belongs

The current local-tone system is off by default, so it did not cause these images. The automatic profile has `local_tone: 0.0`, while highlight reconstruction is enabled at `0.75`.

What you want for sky versus ground is **single-frame local tone mapping**, not necessarily an HDR output format. It can redistribute the luminance range already present in the RAW, but it cannot restore channel ratios that were clipped at capture.

After the reconstruction and gamut fixes, test the existing operator around `--local-tone 0.35`. Do not turn it on by default immediately. Its Gaussian multiscale design can improve broad sky/ground separation, but the foliage/sky boundary in these samples is exactly where halo behavior must be checked.

A more controlled production design would use an edge-aware base/detail decomposition in log luminance:

[
L = \log_2(Y+\epsilon)
]

[
B = \operatorname{guided_filter}(L)
]

[
D = L-B
]

Compress the dynamic range of `B`, retain a bounded fraction of `D`, and convert the difference back into a per-pixel EV correction:

[
\Delta EV = B_{\text{compressed}} + kD - L
]

Then apply one scalar gain to RGB before the global curve. Important constraints:

* keep median correction near zero so exposure does not drift;
* begin with roughly `+0.4–0.6 EV` maximum shadow lift and `−0.6–0.9 EV` maximum broad-highlight compression;
* suppress detail enhancement where reconstruction uncertainty is high;
* never apply independent local curves to R, G, and B;
* test haloing around leaves, branches, and mountain ridges.

This gives the HDR-like balance you are seeking while preserving reconstructed chromaticity.

## How to validate without a “true” reference image

You do not need a perfect real-world control image to fix this defect. You need different references for different questions.

### Correctness reference: synthetic RAW-like ramps

Create synthetic camera-RGB tests with known chromaticity and increasing intensity:

* neutral gray ramp;
* constant blue-sky ramp;
* blue-to-neutral cloud transition;
* bright sky next to dark foliage;
* broad smooth gradient crossing one-, two-, and three-channel saturation.

These have known expected behavior:

* luminance remains monotonic;
* hue remains continuous;
* no discontinuity appears at a clip threshold;
* a constant-hue source remains constant-hue until evidence is genuinely gone;
* fully clipped areas remain smooth and do not acquire fabricated texture.

This is a more rigorous control for the current bug than any vendor JPEG.

### Rendering-intent reference: the camera JPEG

Use `_DSC1289.JPG` for:

* global white balance;
* ground and midtone placement;
* broad scene brightness;
* avoiding an obviously artificial overall look.

Do not use its white sky as the target for recovered sky hue or detail. The camera itself discarded that part of the rendering.

### Future capture reference: a bracket

For several difficult scenes, capture a fixed-camera bracket such as `−2/0/+2 EV`. Merge it to a scene-linear radiance reference. That supplies the control currently missing:

* lower exposure gives real sky chromaticity;
* higher exposure gives real ground/shadow information;
* the middle exposure is what the autotuner must develop automatically.

You need only a small set of bracketed scenes, not a new reference for every photograph.

## Metrics worth adding

Replace the overloaded “near white” interpretation with measurements that describe the actual failure:

* fraction with exactly 1, 2, and 3 clipped channels;
* reconstruction uncertainty distribution;
* hue or OKLab discontinuity across clip-state boundaries;
* low-frequency hue total variation in bright, low-texture regions;
* reconstruction chroma delta and luminance delta;
* pre-gamut versus post-gamut hue delta;
* fully clipped connected-component size;
* sky-band versus ground-band EV separation for the manually graded test frames.

`near_white_fraction` cannot distinguish:

* a legitimate bright white cloud,
* a saturated blue sky,
* a false cyan reconstruction,
* or an almost-white neutralized region.

It should remain an output statistic, but not drive the reconstruction policy.

## Recommended implementation order

1. **Add pre/post reconstruction, clip-map, and pre/post gamut diagnostics.**
2. **Add the synthetic constant-chromaticity saturation-ramp test.**
3. **Rename “near white” internally to “multi-channel clipped” and remove the semantic assumption.**
4. **Carry raw-domain clip confidence through demosaic.**
5. **Implement neighborhood-guided log-chromaticity reconstruction with continuous blending.**
6. **Add reconstruction-uncertainty chroma limiting.**
7. **Move final gamut compression to a perceptual hue-preserving method.**
8. **Only then tune or replace local tone mapping for the sky/ground HDR effect.**
9. **Leave `--highlight-contrast` as a diagnostic or user preference, not the correction. It can brighten and wash out the artifact without recovering accurate color.**

One repository issue also needs correction: the latest changelog refers to `docs/HDR_EVALUATION.md` and `tools/grade_sky.py`, but neither appears to be present on `master`. The conclusion that HDR would not help is therefore not currently reproducible from the pushed repository. The actual reports show that `_DSC1289` has no fully clipped pixels and `_DSC1290` has relatively few, so dismissing these skies as globally unrecoverable is too strong.

The first production fix should be **continuous, neighborhood-guided highlight chromaticity reconstruction**, not another clamp adjustment and not a stronger global shoulder. The HDR-style luminance work is the next layer after that passes the synthetic ramp and clip-boundary tests.
