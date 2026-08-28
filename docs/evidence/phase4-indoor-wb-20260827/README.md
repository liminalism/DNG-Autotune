# 2026-08-27 — indoor tungsten WB A/B: local mixed-light correction fails the camera JPEG gate

Phase 4 leftover: "WB-corrector gate consuming indoor-actionable." The
operator that exists today is `--local-white-balance` (fierro2009 mixed-light
correction, off by default because it greens campfires). This run asks
whether indoor-actionable is a safe permission bit for that operator.

Set: `raw/indoor_tungsten` (9 RAW+JPEG pairs). Two arms, same binary,
`--reference --scene-classify --illuminant --jobs 2`:

- `default.json` — as-shot WB, no local correction
- `lwb.json` — `--local-white-balance 1`

Fusion on this set matches the C5 phase-2 split: five frames indoor-actionable
(CCT 2941–3209 K: `_DSC1325–1328`, `_DSC1331`); `_DSC1323`/`_DSC1330`
observed (daylight-contaminated, CCT ~5050–5080 K); `_DSC1329` observed
(outdoor night, CCT 5348 K, photographer note); `_DSC1324` quiet (classifier
Night_shot, not Indoor).

## Local-WB vs camera JPEG

Mean hue error against the camera JPEG, lower is closer:

| file | indoor | C5 vs as-shot | default hue | local-WB hue | Δ |
|---|---|---|---|---|---|
| `_DSC1323` | observed | 4.07° | 29.4° | 30.9° | +1.6 |
| `_DSC1324` | quiet | 3.83° | 23.8° | 42.0° | +18.3 |
| `_DSC1325` | **actionable** | 4.11° | 16.7° | **93.2°** | **+76.5** |
| `_DSC1326` | **actionable** | 1.31° | 18.3° | 31.8° | +13.6 |
| `_DSC1327` | **actionable** | 1.52° | 25.1° | 54.3° | +29.2 |
| `_DSC1328` | **actionable** | 1.15° | 11.3° | 23.0° | +11.7 |
| `_DSC1329` | observed | 11.97° | 11.2° | 11.2° | 0 (1 light, skipped) |
| `_DSC1330` | observed | 1.98° | 26.0° | 26.0° | 0 (separation 0.019, skipped) |
| `_DSC1331` | **actionable** | 3.69° | 9.0° | 9.0° | 0 (1 light, skipped) |

Batch: default hue mean 18.25° / p90 41.9°; local-WB 30.93° / p90 75.4°.
Every frame the operator actually touched moved *away* from the camera.
`_DSC1325` is the ruin case. Saturation ratio moved 1.41 → 1.19 (closer to
1.0) while hue collapsed — a classic mixed-light over-correction: the
estimator found two "lights" 0.04–0.12 apart in tungsten rooms (lamp vs
bounce, not two illuminants) and neutralized them.

So indoor-actionable must **not** enable `--local-white-balance`. The
fierro2009 operator is the wrong tool for single-illuminant tungsten; its
2-light trigger fires on spatial chromaticity variation inside one light.

## C5 vs as-shot is not the camera-JPEG gap either

On the tungsten-actionable cluster, C5's `as_shot_angular_error_deg` is
1.15–4.11°. The default render's hue error vs the camera JPEG is 9–25°.
A von Kries swap from as-shot to C5 cannot close that gap: the residual is
an order of magnitude larger than the illuminant disagreement. That residual
is the same look gap Candidate D is for (per-hue table after the matrix),
not an illuminant miss.

## What this does not decide

- Mixed window+tungsten interiors (the original fierro2009 job) still have
  no paired corpus. Local-WB stays opt-in, off by default, ungated.
- A C5-driven *global* WB replacement was not implemented; the angular
  errors above say it is not the indoor JPEG gap.
- Backlit tone policy still needs true subject-against-light pairs.
