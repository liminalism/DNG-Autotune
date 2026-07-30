# Notes on the papers in `research/`

Read 2026-07-27. This records what is applicable to raw-autotune and what is
not, so the same papers do not have to be re-read to answer that question.

## The applicability caveat, first

Six of the seven papers enhance **8-bit display-referred images** — photographs
that have already been rendered, where the goal is to recover detail a camera
pipeline threw away. raw-autotune does the opposite job: it *renders*
**scene-linear float** data that has not been compressed yet.

So their headline algorithms are largely not portable. Their dynamic range
compression exists to undo damage we never do. Adopting it wholesale would be a
step backwards.

What *is* portable is the algebra and the measurements, because our output
stage is bounded [0, 1] display-referred data — exactly the domain the
logarithmic image processing (LIP) model was built for.

## Finding 1: our highlight curve is already LIP scalar multiplication

The LIP model (Jourlin & Pinoli; see florea2008 eq. 4, albu2009 eq. 6,
nnolim2018 eq. 1) defines scalar multiplication on a bounded range `M` as:

```text
alpha (x) f = M - M * (1 - f/M)^alpha
```

`tone::map_ev`'s highlight branch is:

```text
white_output_ev * (1 - (1 - position)^highlight_power)
```

Substituting `M = white_output_ev` and `f = position * M` makes these
identical, and they agree to machine precision when evaluated. The curve was
derived independently but landed on the LIP form.

This matters because the papers' central claim for LIP is the **closing
property**: the result of a LIP operation on an in-range value is always in
range. That is why our EV-domain curve can never exceed the white point, and it
confirms that the clipping fixed in 0.1.3 was never in the curve — it was in
the channel gain applied afterwards.

The shadow branch is a plain power law, not a LIP operation. Whether it should
be is an open question.

## Finding 2: the papers describe the 0.1.3 bug, and a different fix

albu2009 §2.3, on multiplying colour channels by a gain:

> The multiplications used in equations (5) above can be either the usual
> real-number multiplications or can be replaced by logarithmic scalar
> multiplications, **in order to avoid possible saturation problems (and the
> clipping of the color components)**.

That is precisely the 0.1.3 defect. Their fix is to make the per-channel
amplification a LIP multiplication, which cannot leave the range for any gain.

We fixed it differently, with the `highlight_norm` blend. The two are not
equivalent:

- **LIP per channel** compresses each channel independently, so a bright
  saturated pixel loses more in its strong channel than its weak ones. Channel
  ratios are not preserved, so hue rotates in the highlights. Film emulations
  often want exactly this.
- **Our norm blend** scales all three channels by one ratio, so channel ratios
  and therefore hue are preserved.

nnolim2018 is explicit that per-channel processing in RGB "usually leads to
colour distortion or fading", which is the cost of the LIP route. Our choice is
the hue-preserving one, and that is the right default for a RAW developer.
A per-channel LIP mode is worth having as an optional *look*, not as the
default.

## Finding 3: `highlight_norm` is nnolim2018's intensity-value model

nnolim2018 eq. 21 defines a hybrid norm:

```text
IV = alpha * I + beta * V,  where I = luminance, V = max(R, G, B)
```

That is exactly the blend added in 0.1.3, with our `highlight_norm` playing the
role of `beta`. The paper's experimental findings match what we observed:

- intensity alone oversaturates and bleeds in the dominant channel;
- value (max channel) alone is better balanced but flattens;
- the weighted combination beats either, and beat `max()`, `min()` and
  multiplicative-square-root combinations that they also tried.

Independent confirmation of the design. One difference: they use a fixed
`beta` per image, we ramp it in with brightness so midtone exposure is
untouched. Ours is better suited to tone mapping; theirs is simpler.

Their eq. 24 adds an adaptive exponent `1/m`, where `m` is the mean of the
modal ratios of `R/IV`, `G/IV`, `B/IV`. That is a route to making
`highlight_norm` per-image rather than per-preset. Untried here.

## Finding 4: adopt their metrics (done in 0.1.4)

The highlight work in 0.1.3 was steered by a hand-rolled statistic. The
literature has validated ones, now implemented in `src/metrics.rs`:

- **Colourfulness** (Hasler & Süsstrunk, nnolim2018 eq. 34) — reported to
  correlate above 90% with human ranking. Now in the sidecar and `--summary`.
- **Near-white / clipped fraction** — our own, but split so that "blown" (>=98%
  of full scale) is distinguished from "literally clipped". The distinction
  matters: after 0.1.3 hard clipping is near zero everywhere, so it alone
  cannot steer anything.

Metrics named but not implemented, worth considering:

- **HDI** (hue deviation index) — mean hue change between input and output.
  This would directly measure the hue-preservation claim above, rather than
  asserting it.
- **CEF** — colourfulness ratio, output over input.
- **AMBE** (gupta2016) — absolute mean brightness error. Not applicable as-is,
  since automatic exposure changes brightness deliberately.

## Finding 5: ideas held for later

- **gupta2016**: blend the input histogram toward uniform, `H' = lambda*H +
  (1-lambda)*H_uniform`, as a single well-behaved knob for enhancement
  strength; and a colour-preserving output blend `delta*enhanced +
  (1-delta)*original`. Both are plausible shapes for a future "strength"
  control that is smoother than switching presets.
- **fierro2009**: implemented in 0.1.7, see below.
- **patrascu (color-image-processing-using-logarithmic-operations)**: a full
  logarithmic vector space over colours with an isomorphism `arctanh`, giving
  addition, scalar multiplication and a Euclidean norm that all preserve range.
  More general than what we need today, but it is the formal grounding under
  the LIP results above.

## zhangfeng2015 — local tone mapping (implemented in 0.1.11, partially)

The paper reproduces Reinhard's local photographic operator before combining
its result with a global render through curvelet fusion. The OCR/LaTeX copy has
several damaged equations; the page image in `research/zhangfeng.png` and the
standard Reinhard form resolve them:

```text
Lbar = exp((1/N) * sum(log(delta + Lw)))
L    = (a / Lbar) * Lw
V    = abs(V1(s_i) - V1(s_i+1))
       / (2^phi * a / s_i^2 + V1(s_i))
V1   = L convolved with Gaussian(s_i)
```

`src/localtone.rs` implements that log-average normalization and adjacent-scale
contrast test with `a=0.18`, `phi=8`, epsilon 0.05, and nine scales separated
by 1.6. A three-box Gaussian approximation makes the full-resolution pyramid
linear-time and deterministic.

The implementation intentionally stops before two parts of the paper:

1. It does not use `L/(1+V1)` as the final image. The existing global renderer
   already handles dynamic-range compression and saturated highlights well.
   The selected surround instead produces a bounded EV dodge/burn field, so
   one hue-preserving RGB gain still feeds the established tone curve.
2. It does not implement curvelet fusion. That stage is complex, costly, and
   insufficiently specified for a defensible first pass; it would also mix two
   complete rendered images when this project needs an auditable strength
   control.

Additional safeguards are project-specific: median anchoring prevents an
overall exposure shift, a 0.15 EV dead band suppresses low-amplitude churn,
corrections are capped at +1/-0.75 EV, shadow lift fades below the measured
sensor noise floor, and small bright details are not lifted with a dark
surround. This is Gaussian rather than edge-aware local adaptation, so it stays
opt-in until a wider scene corpus can expose halos and semantic failures.

## kronander2013 — sensor noise model (acted on, partially)

The paper's subject, HDR assembly from multiple exposures or sensors, does not
apply: we develop single exposures. Its §5 does.

It gives the standard sensor model — Poisson photoelectrons plus Gaussian
readout, carried through the gain — which makes variance affine in the signal:

```text
var(s) = shot_slope * s + read_variance
```

That matters because the controller lifts shadows by a median of +1.37 EV
across the Sony batch, up to +5 EV, with no notion of whether there is signal
down there. `src/noise.rs` fits this model per image and reports the scene EV at
which SNR falls to 10 and to 1.

The paper calibrates from bias and flat frames. We have neither, so parameters
are estimated from the image itself, following its reference [9] (Foi et al.).
Two corrections were needed along the way, both worth remembering:

1. **Plain tile variance is useless.** In a photograph it is dominated by
   detail, not noise. The first fit put the SNR=10 crossing at +4.38 EV — above
   sensor saturation, i.e. physically impossible. Fixed by estimating from the
   second difference `x[i-1] - 2x[i] + x[i+1]`, which is exactly zero for any
   linear ramp.
2. **Texture is anisotropic.** Foliage is far busier across one axis than the
   other. Measuring both directions and keeping the quieter one cut the
   across-batch spread from 11 EV to 6 EV on a 10-file sample.

### Validation against EXIF ISO (0.1.6)

The initial across-batch spread of 11 EV looked like a broken estimator. It was
not. `src/shotinfo.rs` reads ISO, and grouping the 258 Sony frames by it gives:

```text
ISO      n   shot_slope   SNR=10 EV
100    126       0.157       -5.81
1000    55       2.620       -2.42
6400     1      17.803       -0.59
8000     1      51.211       +0.94
```

Regressing the SNR=10 crossing on `log2(ISO/100)`, weighted by frame count:

```text
slope      +0.910 EV per stop of ISO
intercept  -5.676 EV at ISO 100
R^2         0.916
```

Theory says read noise referred to the signal scales with gain, so the crossing
must rise **1.00 EV per stop**. Observed 0.910. The measured range of 6.75 EV
also matches the 6.64 stops of ISO present. The spread was real physics.

### What it does now, and the limit of the validation

`analyze` takes a `noise_floor_ev` and will not place the black point below it.
The floor is the SNR=1 crossing: on this batch it binds on 12% of frames and
raises the black point by a median of 1.11 EV.

The validation above is **aggregate, not per-frame**. Individual ISO 100 frames
disagree by up to 7 EV — the ones that trip the floor have a median estimated
floor of -4.78 EV against -11.93 EV for the rest at the same ISO, because dense
texture still inflates the fit. So the floor is additionally capped at the
frame's 5th percentile: however wrong one estimate is, at most about 5% of the
image can be crushed. That cap leaves the median effect unchanged (1.11 vs 1.12
EV) while bounding the worst case from +5.04 EV to +2.80 EV.

### The 43 failures were a bug, not a limitation (fixed in 0.1.8)

The estimator sampled CFA phase (0,0). On an RGGB sensor that is the **red**
plane, which under daylight carries about half the signal of green. Combined
with fixed-width brightness bins, an ordinary frame without bright highlights
put every tile into the darkest few bins and failed the minimum-bin check.
Every one of the 43 failures had `p995_ev <= +0.65`; none occurred above +1.5.

Sampling green (chosen from the camera's CFA layout, not hardcoded) plus
equal-count bins took estimates from 215/258 to 257/258, and improved the ISO
validation on every axis — see the changelog for 0.1.8. The sharper test is on
`shot_slope` itself, since gain is proportional to ISO by definition:
`log2(shot_slope)` against `log2(ISO)` now has slope +1.039 against a theory of
1.000, with R^2 0.976.

Remaining weaknesses, in priority order:

- The per-frame estimate is still content-dependent. Pooling by (make, model,
  ISO) within a batch would fix it properly — the ISO 100 group alone has 164
  frames, so its median is solid — at the cost of making a file's floor depend
  on its batch. That is an interface change and wants a flag.
- `read_variance` still clamps to zero on 142 of 257 frames. There is almost no
  data in the regime where the intercept matters, so this is structural rather
  than a fit bug, and it means the model cannot predict absolute noise. It also
  means the SNR=1 floor is understated at mid to high ISO.
- One frame, with 3.54 EV of dynamic range, is refused by the leverage guard.
  That is correct behaviour rather than a gap.

## fierro2009 — local multi-illuminant white balance (implemented, opt-in)

`src/whitebalance.rs`. The brightest regions are treated as the lights
illuminating the frame; each pixel is corrected by a blend of their white
points weighted by spatial and chromatic proximity (`cf = alpha * beta`).

Three departures from the paper, each forced by something this project has
already committed to:

1. **Opt-in.** `KNOWN_LIMITATIONS.md` records that sunsets, stage light, LEDs
   and underwater scenes are *intentionally* not neutralized. Automatic
   correction would silently reverse that, so it is a flag with a strength dial.
2. **Deterministic clustering.** The paper uses K-means with random seeding plus
   a Gap statistic and states that "repeatability is not granted". This crate
   promises determinism, so lights are found by quantizing the chromaticity
   plane onto a fixed grid and merging cells in a fixed order. There is a test
   asserting repeated detection is identical.
3. **Chromatic only.** The paper's correction "will always increase the
   lightness of the pixel". Exposure is the tone controller's job here, so each
   white point is normalized to unit luminance. Luminance is linear, so a
   weighted blend of unit-luminance white points also has unit luminance.

Their own escalation is implemented too: at the top 5% some frames yield only
one class, so the threshold widens (5, 12, 25, 45%) until two lights appear.
This cannot help when one illuminant is uniformly brighter than the other with
no overlap — then no threshold admits both.

### The failure that required a fourth departure

Run unguarded on the test corpus, the method rendered a campfire **green**. The
fire is genuinely the brightest thing in frame, so it was read as an orange cast
and neutralized — exactly the case `KNOWN_LIMITATIONS.md` had warned about, now
with a picture to prove it.

Lights further than 0.22 in chromaticity from neutral are therefore rejected as
coloured *objects* rather than illuminants. A fire measures about 0.39 from
neutral; a mixed tungsten/daylight interior about 0.12, so the threshold
separates the case the method is for from the case it destroys. With the guard,
the close-up campfire frame finds one light and is left untouched, while a night
street scene still has its magenta cast corrected.

### Status

Works on its intended case and is safe on the corpus, but it has only been
exercised on 7 phone DNGs plus synthetic tests. `MAX_ILLUMINANT_CAST` is set
from two data points and wants a wider corpus of genuinely mixed-lighting
frames before it can be called well-calibrated. It has not been run over the
258 Sony files.

## patrascu — symmetric LIP (read, not acted on)

The LaTeX conversion is complete where the markdown was not. The model puts
colours in `(-1,1)^3` with `arctanh` as the isomorphism, giving

```text
lambda (x) v = tanh(lambda * arctanh(v))
```

which is a contrast operator anchored at the midpoint that preserves *both*
bounds, unlike the asymmetric Jourlin-Pinoli LIP that preserves only the top.

That is structurally close to what `tone::map_ev` does — two segments meeting
at middle grey — and would unify the two branches, since our highlight branch
is already LIP scalar multiplication while the shadow branch is a plain power
law. It is not acted on because the curve works and there is no measured defect
to fix; the presets deliberately want asymmetric shadow and highlight control,
which a symmetric operator would constrain. Recorded in case a future contrast
control wants a single principled operator.

Their own finding is worth noting: their algorithm A "preserves quite well the
hue" while algorithm B "tends to attenuate the dominant colors which results in
hue modifications" — consistent with the hue caveat on per-channel LIP above.

## Conversion quality

`florea2008` and `patrascu` converted poorly — their equations are largely
destroyed by the OCR. The core LIP formulas were recovered by triangulating
across albu2009, florea2008 and nnolim2018, which agree. `patrascu` has since
been converted to LaTeX (`research/latex/patrascu.tex`) and is complete;
`florea2008` remains only partially readable, though nothing further is needed
from it.
