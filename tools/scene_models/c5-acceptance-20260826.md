# C5 phase-2 acceptance run — 2026-08-26

`--illuminant` (observational) over the new paired RAW+JPEG batch
`raw/indoor_tungsten/` (shot 2026-08-26: overhead tungsten, small lamp,
and daylight-contaminated porch/room frames) and a 12-frame every-22nd
sample of the `raw/arw` daylight corpus.

## indoor_tungsten (n=9)

| frame | CCT (K) | duv | vs as-shot (°) |
| --- | --- | --- | --- |
| _DSC1323 | 5049 | −0.0052 | 4.07 |
| _DSC1324 | 3398 | −0.0037 | 3.04 |
| _DSC1325 | 3107 | −0.0078 | 4.44 |
| _DSC1326 | 2936 | −0.0039 | 1.53 |
| _DSC1327 | 2972 | −0.0011 | 1.60 |
| _DSC1328 | 3124 | −0.0015 | 1.28 |
| _DSC1329 | 5354 | +0.0008 | 12.07 † |
| _DSC1330 | 5089 | +0.0041 | 2.04 |
| _DSC1331 | 2946 | −0.0119 | 4.36 |

† _DSC1329 is not an indoor frame: per the photographer (2026-08-27) it is
an **outdoor night shot with the illuminating light out of frame**. Its
12.07° disagreement with the camera neutral and its daylight-band CCT are
therefore not a mixed-light indoor reading; treat it as an outlier outside
both clusters, not as evidence either way.

## daylight corpus control (n=12, every 22nd ARW)

| frame | CCT (K) | duv | vs as-shot (°) |
| --- | --- | --- | --- |
| _DSC0885 | 4339 | −0.0016 | 1.59 |
| _DSC0907 | 4579 | −0.0006 | 2.30 |
| _DSC0929 | 7459 | +0.0045 | 8.19 |
| _DSC0951 | 7941 | +0.0069 | 7.82 |
| _DSC0973 | 5002 | −0.0011 | 1.66 |
| _DSC0995 | 5982 | +0.0011 | 4.79 |
| _DSC1017 | 5387 | +0.0016 | 0.92 |
| _DSC1039 | 5942 | −0.0010 | 6.22 |
| _DSC1061 | 6092 | −0.0005 | 5.44 |
| _DSC1083 | 5864 | +0.0062 | 4.21 |
| _DSC1105 | 5610 | +0.0012 | 2.78 |
| _DSC1127 | 6259 | +0.0015 | 5.44 |

## Verdict

- **Separation**: tungsten cluster 2936–3398 K (negative duv, as tungsten
  should be); daylight-dominated frames ≥ 4339 K, median 5903 K. Zero
  overlap (gap 3398 → 4339 K). Acceptance check
  `c5-cct-separates-known-illuminants`: **pass**.
- **Independence**: `vs as-shot` angular error spans 0.9–12.1° — the
  estimate is not a read-back of the camera's own neutral.
- **Regression gates**: flag-off summaries byte-identical to the
  pre-change HEAD binary over both batches (ignoring `elapsed_ms`);
  repeated flag-on runs at `--jobs 8` identical; 357 lib + 15 integration
  tests pass; clippy clean.
- **Cost when enabled**: ~80–130 ms/frame (histogram ~8 ms, apply_ccc
  0.4 ms, remainder lege-gpu CPU ONNX reference). Zero when off.
