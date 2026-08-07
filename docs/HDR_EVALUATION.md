# Would HDR output help this corpus?

Written 2026-08-07, prompted by the blown-sky frames in `raw/raw_3rd_batch`
(`_DSC1282`, `_DSC1283`, `_DSC1288`, `_DSC1289`, `_DSC1290`), where skies
rendered either flat white or lavender-magenta. HDR output had been proposed as
the fix. This note is the measurement that was run before writing any code.

**Verdict: no, not for this defect, and not for this corpus as it stands.** The
lavender skies were a bug in highlight reconstruction, not a dynamic-range
limitation; that bug is fixed (see `CHANGELOG.md` and `src/highlight.rs`). The
remaining flatness in the worst skies is missing sensor data, which no output
container recovers. HDR output stays on the roadmap as a feature with its own
justification, not as an answer to this.

## What was measured

All 58 files in `raw/raw_3rd_batch` (42 Sony A7C ARW, 16 Samsung DNG), decoded
to normalized camera RGB, classified per pixel by how many of its three channels
sit at or above 0.98 of the recorded white level — the same `CLIP_THRESHOLD`
`src/highlight.rs` uses. "Near-white" means two or three channels clipped: the
pixel's hue is gone, because the surviving ratio is the white balance's, not the
scene's.

| Frame      | 0 clipped | 1 clipped | 2 clipped | 3 clipped |
|------------|-----------|-----------|-----------|-----------|
| `_DSC1282` |    95.56% |     0.93% |     2.74% |     0.77% |
| `_DSC1283` |    84.88% |     2.70% |    10.50% |     1.92% |
| `_DSC1288` |    91.55% |     5.01% |     3.44% |     0.00% |
| `_DSC1289` |    96.44% |     2.41% |     1.15% |     0.00% |
| `_DSC1290` |    96.57% |     0.62% |     2.79% |     0.02% |

Across the whole batch the near-white fraction has a median of 0.0003%, a 90th
percentile of 1.9%, and a maximum of 12.4%. Nine of 58 frames exceed 1%. Five of
the top six are exactly the frames flagged by eye, which is the useful result on
its own: `near_white_pixels`, now reported in the sidecar, predicts the defect
without anyone having to look at the picture.

## Why HDR does not address it

An HDR container buys headroom *above diffuse white on the display*. It is worth
having when the raw holds detail up there that SDR has to compress or discard.
So the question is not "how bright is the sky" but "how much unclipped scene
range sits above the frame's diffuse white".

Measured as the span from the 99th percentile of the *unclipped* population to
the brightest unclipped pixel:

| Frame      | Unclipped headroom above p99 |
|------------|------------------------------|
| `_DSC1282` |                      0.78 EV |
| `_DSC1283` |                      0.39 EV |
| `_DSC1288` |                      0.08 EV |
| `_DSC1289` |                      0.18 EV |
| `_DSC1290` |                      0.88 EV |
| `_DSC1251` |                      0.77 EV |
| `_DSC1261` |                      0.29 EV |

Under one stop everywhere, against the two to four stops an HDR display offers
above diffuse white. There is nothing to put in the headroom. Rendering these
frames to PQ or to a gain-map JPEG would produce the same flat sky, brighter and
in a larger file.

The frames themselves say the same thing from the other direction: the p99 of
each sits only 3.2 to 4.2 EV above the frame median, and the bulk of every frame
spans roughly 8 EV. That is a range SDR renders without strain. These are not
high-dynamic-range scenes that got squeezed; they are ordinary-range scenes whose
brightest region was overexposed at capture and hard-clipped in the sensor.

Once a channel is at the sensor's saturation level the number is a floor, not a
measurement. Two clipped channels leave one number and no hue. Three leave none.
`src/highlight.rs` makes the most defensible reconstruction available — every
clipped channel onto a single white-balanced level, so the pixel comes out as
bright as its brightest evidence and as neutral as the evidence allows — and that
is the end of what any renderer can do. A wider container does not add
information that the exposure destroyed.

### A caveat on the whole-batch dynamic-range figure

The batch scan reports a median scene range of 8.0 EV but a 90th percentile of
23 EV, with 16 frames above 10 EV. Those large numbers are not real scene range:
they come from taking the 0.1st percentile on dark, high-ISO frames, where it
lands in the noise floor rather than on any subject. `src/noise.rs` already
exists because this exact statistic is untrustworthy per frame. The figure is
recorded here so nobody re-derives it later and reads it as evidence for HDR; it
is not.

## What would actually improve these skies

In descending order of measured value:

1. **The highlight-reconstruction fix** (done). Removes the colour cast outright
   on the near-white cohort: on `_DSC1283`, green rendered at 0.63 of the
   brightest channel across the 12.4% near-white population and now renders at
   parity. This was the whole of the visible defect on `_DSC1289` and `_DSC1290`,
   where the near-white fraction is under 3%.
2. **Exposure**, at capture. `_DSC1283` put 12.4% of the frame at hard clip. No
   software stage recovers it. If these are re-shootable, one to two stops down
   changes more than anything in this repository can.
3. **`--local-tone`**, already implemented and off by default. It is the right
   tool for the stated goal of holding shadow detail while keeping the sky in
   range, and it acts on frames that are genuinely high-contrast rather than
   merely clipped. It has not been graded on this batch; that is a cheap
   experiment and a better next step than an encoder.
4. **HDR output**, last. Justified by a use case — the user has an HDR display and
   wants archives that use it — not by this defect.

## If HDR is built anyway

Notes for whoever picks it up, so this evaluation is not read as a blanket no:

- The pipeline is already well placed for it. `tone.rs` works in scene-linear
  RGB and only meets a display transfer function at `encode_srgb_u16`; the
  `[0, 1]` clamp lives in `compress_gamut` and `to_u16`. An HDR path forks after
  the curve, not through it.
- The honest gate is a measurement, not a format choice: the unclipped-headroom
  figure above, computed per frame. Frames below about 1 EV have nothing to gain
  and should keep rendering SDR. That check belongs in `analyze.rs` alongside the
  tonal classification, and it should be reported in the sidecar before any
  encoder is written, so the decision is made on the corpus rather than by hand.
- Determinism is the usual constraint. A gain-map format encodes a second image
  plus metadata; both have to be byte-reproducible, and the roadmap's standing
  rule that a file develops identically alone or in a batch applies to the gain
  map as much as to the base image.
- The tonal classifier already labels five frames in this batch
  `HighDynamicRange`. Those are where an HDR experiment should start, and none of
  them are the five blown-sky frames.
