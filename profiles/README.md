# Camera profiles

Tables in this directory are **data**, not code. Each file needs its own
provenance before it is shipped.

| File | Camera | Source | Copyright tag | Ship? |
|---|---|---|---|---|
| `SONY_ILCE-7C.dcp` | Sony ILCE-7C | [ART](https://github.com/artraweditor/ART) `rtdata/dcpprofiles/SONY ILCE-7C.dcp` (also in RawTherapee) | `ProfileCopyright` = `public domain`. Dual-illuminant (StdA / D65) HueSatMap 90×30×1, identity tone curve. Not an Adobe profile (those say `Adobe Systems`). | Yes, with this notice. |

Do **not** add Adobe Camera Raw / Lightroom `.dcp` files. The DNG spec being
open does not license Adobe's profile contents.

`--hue-sat-map` is off by default. Pass a path explicitly:

```
raw-autotune raw --hue-sat-map profiles/SONY_ILCE-7C.dcp --output out
```

Strength 0 is an exact no-op.
