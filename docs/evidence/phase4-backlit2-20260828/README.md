# 2026-08-28 — `raw/raw_backlit2` contre-jour A/B: no backlit fill policy ships

Phase 4 leftover after indoor WB was rejected: silhouette-vs-fill tone
policy, gated on `scene_lighting.backlit == Actionable`. New set: 14
RAW+JPEG pairs in `raw/raw_backlit2`.

Two arms, same binary:

- `default.json` — archive-auto-v8, `--scene-classify --illuminant --reference`
  (preview oracle on, 14/14)
- `nopreview.json` — `--no-preview --reference` on the five hand-against-sky
  frames (independent controller)

`silhouette-vs-fill-sheet.jpg` is camera JPEG | default | independent for
`_DSC1342/1343` (fill) and `_DSC1346/1347` (dusk silhouette).

## The set is real contre-jour

Unlike `raw/raw_backlit` (ajar-door, Indoor, negative EV split), this batch
has:

- **Fill:** hand against the sun, skin readable (`_DSC1342`, `_DSC1343`,
  `_DSC1344`/`_DSC1345` with the sun more central)
- **Silhouette:** the same hand at dusk, almost no skin (`_DSC1346`,
  `_DSC1347`)
- Architecture against bright sky (`_DSC1337–1341`)
- Indoor window looking out (`_DSC1348–1350`)

The classifier **does** see Backlight on the hand series (top-3 on
1342–1347; 1343/1345 top-1 at 0.55/0.67). Architecture and window frames
are quiet or Indoor, which is honest.

## Fusion never reaches Actionable

`BACKLIT_EV_SPLIT_MARGIN_EV = 1.25` (above the old corpus p99 of 1.11).
Geometric `p50_ev − center_median_ev` on this set:

| file | Backlight p | split | state |
|---|---|---|---|
| `_DSC1342` fill | 0.16 | **+0.21** | observed |
| `_DSC1343` fill | 0.55 | **+0.17** | observed |
| `_DSC1345` sun-centred | 0.67 | −1.15 | observed |
| `_DSC1346` silhouette | 0.31 | −1.53 | observed |
| `_DSC1347` silhouette | 0.29 | −1.83 | observed |

The split is the wrong statistic for an off-centre subject: on 1346 the
hand is left, the geometric centre is sky, so the split goes *negative*
(centre brighter than the median) even though the frame is textbook
contre-jour. Even the centred fill hands only reach +0.21 EV. Lowering
the margin to catch them would still miss the silhouettes, and would
false-promote ordinary corpus frames the 1.25 threshold was set to
exclude.

**0 / 14 actionable.** Observed is the honest state.

## Default already matches the camera’s silhouette-vs-fill

Preview-guided centre-key vs camera JPEG, 14/14:

- median **+0.06 EV**
- fill hands `_DSC1342/1343`: **−0.01 / +0.01 EV** (skin, rim-light, and
  sun placement match the camera)
- dusk silhouettes `_DSC1346/1347`: **+0.32 / +0.38 EV** (slight extra
  shadow detail; the hand stays a silhouette). That is the same archival
  toe already sealed as `decision.midtone-and-shadow-placement-stays`,
  not a fill policy.

Independent (`--no-preview`) on the same five hands:

| file | default key Δ | independent key Δ |
|---|---|---|
| `_DSC1342` fill | −0.01 | +0.43 |
| `_DSC1343` fill | +0.01 | +0.64 |
| `_DSC1346` silhouette | +0.32 | **+3.51** |
| `_DSC1347` silhouette | +0.38 | **+3.73** |

Without the oracle, the dusk silhouettes become daytime-filled hands
(+3.5 EV, sky goes blue). That is the ruin a “backlit ⇒ fill the
subject” policy would ship. The oracle is already the silhouette-vs-fill
mechanism; a second policy that lifts dark centres when the classifier
says Backlight would fight it.

## Decision

Do not consume `backlit == Actionable` (and do not lower the 1.25 EV
margin to manufacture Actionable) for a tone or exposure change. Leave
the fusion observational. Preview-guided auto remains the unattended
path.

Window frames (`_DSC1348` +1.50 EV) and the high-ISO shower (`_DSC1338`
+1.27 EV) are the existing shadow-retention vs camera crush, classifier
Indoor/Architecture, not Backlight.
