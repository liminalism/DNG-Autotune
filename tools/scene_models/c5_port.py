#!/usr/bin/env python3
"""Port C5 (cross-camera color constancy, ICCV 2021, Apache-2.0) to ONNX.

Phase 2 of research/LIGHTING_DETECTION_PLAN.md (Model B). The exported graph
contains ONLY the neural half of C5: the encoder/bottleneck/decoders that emit
the CCC filter F and bias B from the input histogram stack. The application
step (circular convolution of the histogram with F in frequency space, bias,
softmax, soft-argmax to an illuminant rgb) is deliberately NOT in the graph —
upstream does it with FFTs, which lege-gpu rejects, and on a 64x64 histogram
it is cheap deterministic post-processing the Rust side owns.

Input assembly (mirrors upstream src/dataset.py):
  per frame -> 4 x 64 x 64: [uv-chroma histogram, edge histogram,
                             normalized u-coord plane, v-coord plane]
  the ONNX graph's input is just that single 1 x 4 x 64 x 64 stack. The
  encoder itself wants m=7 copies (upstream fills the six additional slots
  with RANDOM sibling images from the same sensor); we duplicate the frame's
  own stack instead — deterministic (a file must develop identically alone
  or in a batch), and the degenerate case the architecture tolerates
  (cross-pooling over identical copies is a no-op) — so the graph builds the
  1 x 7 x 4 x 64 x 64 tensor ITSELF via unsqueeze+Concat (no Expand/Tile,
  both hard-rejected by lege-gpu), and the Rust caller only ever has to hand
  over one stack. The parity stage measures what duplicate-mode costs
  against ground truth on the in-repo sample set.

Usage:
  .venv-train/bin/python tools/scene_models/c5_port.py --stage export
  .venv-train/bin/python tools/scene_models/c5_port.py --stage parity
  .venv-train/bin/python tools/scene_models/c5_port.py --stage fixtures
"""

import argparse
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_UPSTREAM = REPO / ".agent/scratch/c5-upstream"
HIST_SIZE = 64
DATA_NUM = 7


def load_upstream(upstream: Path, weights: Path, device):
    sys.path.insert(0, str(upstream))
    import torch
    from src import c5

    net = c5.network(input_size=HIST_SIZE, learn_g=False, data_num=DATA_NUM,
                     device=device)
    net.load_state_dict(torch.load(weights, map_location=device,
                                   weights_only=True))
    net.eval()
    return net


def make_exportable(net):
    """Rewrite ops lege-gpu rejects into its supported set, numerically
    faithfully: LeakyReLU -> PRelu (identical for a scalar slope),
    InstanceNorm2d -> ReduceMean/Sub/Pow/Sqrt/Div (torch IN uses biased
    variance, so mean-of-squares matches), CrossPooling's stacked ReduceMax
    -> a chain of elementwise Max."""
    import torch
    from src import c5

    class DecomposedIN(torch.nn.Module):
        def __init__(self, eps):
            super().__init__()
            self.eps = eps

        def forward(self, x):
            mu = x.mean(dim=(2, 3), keepdim=True)
            var = (x - mu).pow(2).mean(dim=(2, 3), keepdim=True)
            return (x - mu) / torch.sqrt(var + self.eps)

    class ChainMaxPool(torch.nn.Module):
        def forward(self, x):
            parts = torch.unbind(x, dim=-1)
            y = parts[0]
            for p in parts[1:]:
                y = torch.maximum(y, p)
            return y

    def swap(module):
        for name, child in module.named_children():
            if isinstance(child, torch.nn.LeakyReLU):
                prelu = torch.nn.PReLU(num_parameters=1,
                                       init=child.negative_slope)
                setattr(module, name, prelu)
            elif isinstance(child, torch.nn.InstanceNorm2d):
                assert not child.affine
                setattr(module, name, DecomposedIN(child.eps))
            elif isinstance(child, c5.CrossPooling):
                setattr(module, name, ChainMaxPool())
            else:
                swap(child)

    swap(net)
    return net


class C5Heads:
    """torch.nn.Module wrapper built lazily (torch import stays in main).

    Takes the full m=7 stack (1x7x4x64x64) directly -- used by the parity
    stage to run the unpatched torch reference net with its original
    forward semantics."""

    def __new__(cls, net):
        import torch

        class _Wrapper(torch.nn.Module):
            def __init__(self, inner):
                super().__init__()
                self.inner = inner

            def forward(self, model_in_N):
                latent, skip = self.inner.encoder(model_in_N)
                latent = self.inner.bottleneck(latent)
                # keep the batch dim (upstream squeezes it away at b=1)
                B = self.inner.decoder_B(latent, skip).squeeze(1)
                F = self.inner.decoder_F(latent, skip)
                return F, B

        return _Wrapper(net)


class C5HeadsSingleFrame:
    """Export wrapper: takes ONE 1x4x64x64 stack and duplicates it into the
    m=7 slots INSIDE the graph (Concat of repeated inputs), so the Rust
    caller does not need to build the 7x stack itself. Uses unsqueeze+cat
    rather than expand/repeat/tile -- ONNX Expand/Tile are hard-rejected by
    lege-gpu."""

    def __new__(cls, net):
        import torch

        class _Wrapper(torch.nn.Module):
            def __init__(self, inner):
                super().__init__()
                self.inner = inner

            def forward(self, chroma_histograms):
                x = chroma_histograms.unsqueeze(1)  # 1x1x4x64x64
                model_in_N = torch.cat([x] * DATA_NUM, dim=1)  # 1x7x4x64x64
                latent, skip = self.inner.encoder(model_in_N)
                latent = self.inner.bottleneck(latent)
                B = self.inner.decoder_B(latent, skip).squeeze(1)
                F = self.inner.decoder_F(latent, skip)
                return F, B

        return _Wrapper(net)


def apply_ccc(N, F, B):
    """The post-processing the Rust side will own: author's torch>=1.8 path."""
    import torch
    import torch.fft as fft
    from src import ops

    N_fft = fft.rfft2(N[:, :2, :, :])
    F_fft = fft.rfft2(F)
    heat = fft.irfft2(N_fft * F_fft).sum(dim=1) + B
    heat = torch.clamp(heat, -100, 100)
    P = torch.softmax(heat.reshape(heat.shape[0], -1), dim=-1)
    P = P.reshape(heat.shape)
    u_coord, v_coord = ops.get_uv_coord(HIST_SIZE, tensor=True, device="cpu")
    u = torch.sum(P * u_coord, dim=[-1, -2])
    v = torch.sum(P * v_coord, dim=[-1, -2])
    u, v = ops.from_coord_to_uv(HIST_SIZE, u, v)
    return ops.uv_to_rgb(torch.stack([u, v], dim=1), tensor=True)


def frame_stack(img):
    """One frame -> 4 x 64 x 64 float32 (histogram, edges, u, v planes)."""
    import numpy as np
    from src import ops

    boundary = ops.get_hist_boundary()
    hist = np.zeros((HIST_SIZE, HIST_SIZE, 2))
    chroma, colors = ops.get_hist_colors(img, ops.rgb_to_uv)
    hist[:, :, 0] = ops.compute_histogram(chroma, boundary, HIST_SIZE,
                                          rgb_input=colors)
    edges = ops.compute_edges(img)
    chroma_e, colors_e = ops.get_hist_colors(edges, ops.rgb_to_uv)
    hist[:, :, 1] = ops.compute_histogram(chroma_e, boundary, HIST_SIZE,
                                          rgb_input=colors_e)
    u, v = ops.get_uv_coord(HIST_SIZE, tensor=False, normalize=True)
    stack = np.dstack([hist, u[:, :, None], v[:, :, None]])
    return stack.transpose(2, 0, 1).astype("float32")


def load_frame(path):
    from src import ops

    img = ops.read_image(str(path))
    return ops.resize_image(img, [384, 256])


def sample_frames(upstream: Path):
    frames = sorted((upstream / "images").glob("*_sensorname_*.png"))
    if not frames:
        raise SystemExit(f"no sample frames under {upstream}/images")
    return frames


def angular_error_deg(a, b):
    import numpy as np

    a = np.asarray(a, dtype="float64").ravel()
    b = np.asarray(b, dtype="float64").ravel()
    cos = np.clip(a @ b / ((np.linalg.norm(a) * np.linalg.norm(b)) + 1e-12),
                  -1.0, 1.0)
    return float(np.degrees(np.arccos(cos)))


def export_stage(args, net):
    import torch

    wrapper = C5HeadsSingleFrame(make_exportable(net))
    wrapper.eval()
    dummy = torch.zeros(1, 4, HIST_SIZE, HIST_SIZE)
    out = Path(args.export_onnx)
    out.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(
        wrapper,
        dummy,
        str(out),
        input_names=["chroma_histograms"],
        output_names=["ccc_filter", "ccc_bias"],
        opset_version=17,
        dynamo=False,
    )

    # Three value-preserving rewrites, in this order, so lege-gpu's compiled
    # graph can accept what torch emitted:
    #   1. onnxsim folds the constant shape-plumbing torch generates for
    #      `Conv(padding_mode=...)` — including a reverse-step `Slice` the
    #      bridge's own constant folder declines to evaluate;
    #   2. the encoder's `x[:, i]` frame selection exports as a scalar-index
    #      `Gather`, for which the bridge has no kernel, and which is exactly a
    #      one-element `Slice` plus a `Squeeze`;
    #   3. the convolutions pad by replication, which exports as an edge-mode
    #      `Pad` the bridge has no kernel for, and which is a Slice+Concat of
    #      the border planes;
    #   4. the normalization blocks reduce over both spatial axes at once,
    #      which the bridge takes only as a GlobalAveragePool;
    #   5. every remaining literal is still a `Constant` node, and the bridge
    #      expects initializers.
    import onnx

    from prepare import (
        lift_constants,
        rewrite_edge_pad,
        rewrite_gather_to_slice,
        rewrite_spatial_reducemean,
        simplify,
    )

    # `simplify` degrades to a warning when onnxsim is missing, which would
    # leave a graph the later rewrites cannot fix and lege-gpu silently
    # rejects at load. Fail here instead, where the cause is obvious.
    import onnxsim  # noqa: F401

    model = simplify(onnx.load(str(out)))
    gathers = rewrite_gather_to_slice(model)
    edge_pads = rewrite_edge_pad(model)
    means = rewrite_spatial_reducemean(model)
    lifted = lift_constants(model)
    onnx.checker.check_model(model)
    onnx.save(model, str(out))
    print(f"simplified; rewrote {gathers} Gather node(s) to Slice+Squeeze, "
          f"{edge_pads} edge Pad node(s) to Slice+Concat, {means} spatial "
          f"ReduceMean node(s) to GlobalAveragePool, and lifted "
          f"{lifted} Constant node(s) into initializers")

    print(f"exported {out} (1x4x{HIST_SIZE}x{HIST_SIZE} -> duplicated to "
          f"1x{DATA_NUM}x4x{HIST_SIZE}x{HIST_SIZE} inside the graph -> "
          f"F 1x2x{HIST_SIZE}x{HIST_SIZE}, B 1x{HIST_SIZE}x{HIST_SIZE})")
    print(f"screen next: python3 tools/scene_models/report.py {out}")


def parity_stage(args, net):
    import numpy as np
    import onnxruntime as ort
    import torch

    frames = sample_frames(Path(args.upstream))
    stacks = [frame_stack(load_frame(f)) for f in frames]
    gts = []
    for f in frames:
        meta = json.loads(f.with_name(f.stem + "_metadata.json").read_text())
        gts.append(meta.get("illuminant_color_raw") or meta.get("gt_ill"))

    sess = ort.InferenceSession(args.export_onnx,
                                providers=["CPUExecutionProvider"])
    # Full m=7 wrapper for the torch reference: unpatched net, original
    # forward semantics, fed the duplicate-mode 1x7x4x64x64 tensor built
    # here (mirrors what the ONNX graph now builds internally).
    wrapper = C5Heads(net)
    wrapper.eval()

    print(f"{'frame':44s} {'torch-vs-onnx':>13s} "
          f"{'ang.err torch':>13s} {'ang.err onnx':>12s}")
    worst_gap, errs_dup = 0.0, []
    for f, stack, gt in zip(frames, stacks, gts):
        model_in_full = np.stack([stack] * DATA_NUM)[None]  # 1x7x4x64x64
        model_in_single = stack[None]  # 1x4x64x64 (ONNX input)
        t_in = torch.from_numpy(model_in_full)
        N = t_in[:, 0]
        with torch.no_grad():
            F_t, B_t = wrapper(t_in)
            rgb_t = apply_ccc(N, F_t, B_t).numpy()[0]
        F_o, B_o = sess.run(None, {"chroma_histograms": model_in_single})
        with torch.no_grad():
            rgb_o = apply_ccc(N, torch.from_numpy(F_o),
                              torch.from_numpy(B_o)).numpy()[0]
        gap = angular_error_deg(rgb_t, rgb_o)
        worst_gap = max(worst_gap, gap)
        err_t = angular_error_deg(rgb_t, gt)
        err_o = angular_error_deg(rgb_o, gt)
        errs_dup.append(err_o)
        print(f"{f.name[:44]:44s} {gap:12.4f}° "
              f"{err_t:12.2f}° {err_o:11.2f}°")

    import statistics
    print(f"\ntorch-vs-onnx worst angular gap: {worst_gap:.4f}°")
    print(f"mean angular error vs GT — duplicate mode: "
          f"{statistics.mean(errs_dup):.2f}° (n={len(errs_dup)} frames)")
    verdict = "PROMISE" if worst_gap < 0.1 else "FAILURE — export diverges"
    print(f"PARITY: {verdict}")


def fixtures_stage(args, net):
    """Golden fixtures for the upcoming Rust C5 implementation, written to
    tests/fixtures/c5/. Uses the first upstream sample frame, the exported
    ONNX graph's outputs on that frame's stack, and the frame's ground-truth
    illuminant from its _metadata.json."""
    import numpy as np
    import onnxruntime as ort
    import torch

    frames = sample_frames(Path(args.upstream))
    frame = frames[0]
    img = load_frame(frame)
    stack = frame_stack(img)  # 4x64x64 CHW float32

    meta = json.loads(
        frame.with_name(frame.stem + "_metadata.json").read_text())
    gt_ill = meta.get("illuminant_color_raw") or meta.get("gt_ill")

    sess = ort.InferenceSession(args.export_onnx,
                                providers=["CPUExecutionProvider"])
    F_o, B_o = sess.run(None, {"chroma_histograms": stack[None]})

    N_t = torch.from_numpy(stack[None])
    with torch.no_grad():
        illuminant_rgb = apply_ccc(
            N_t, torch.from_numpy(F_o), torch.from_numpy(B_o)
        ).numpy()[0]

    out_dir = Path(args.fixtures_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    image_hwc = np.ascontiguousarray(img.astype("float32"))
    assert image_hwc.shape == (256, 384, 3)
    (out_dir / "image.bin").write_bytes(image_hwc.tobytes())

    stack_c = np.ascontiguousarray(stack.astype("float32"))
    assert stack_c.shape == (4, HIST_SIZE, HIST_SIZE)
    (out_dir / "hist_stack.bin").write_bytes(stack_c.tobytes())

    ccc_filter = np.ascontiguousarray(F_o[0].astype("float32"))
    assert ccc_filter.shape == (2, HIST_SIZE, HIST_SIZE)
    (out_dir / "ccc_filter.bin").write_bytes(ccc_filter.tobytes())

    ccc_bias = np.ascontiguousarray(B_o[0].astype("float32"))
    assert ccc_bias.shape == (HIST_SIZE, HIST_SIZE)
    (out_dir / "ccc_bias.bin").write_bytes(ccc_bias.tobytes())

    expected = {
        "illuminant_rgb": [float(x) for x in illuminant_rgb],
        "gt_ill": [float(x) for x in gt_ill],
        "hist_boundary": [-2.85, 2.85],
        "image_hw": [256, 384],
        "source_frame": frame.name,
    }
    (out_dir / "expected.json").write_text(json.dumps(expected, indent=2) +
                                           "\n")

    readme = out_dir / "README.md"
    readme.write_text(FIXTURES_README)

    print(f"wrote fixtures to {out_dir}")
    for name in ("image.bin", "hist_stack.bin", "ccc_filter.bin",
                "ccc_bias.bin", "expected.json", "README.md"):
        p = out_dir / name
        print(f"  {name}: {p.stat().st_size} bytes")
    print(f"source frame: {frame.name}")
    print(f"illuminant_rgb (onnx): {expected['illuminant_rgb']}")
    print(f"gt_ill: {expected['gt_ill']}")


FIXTURES_README = """\
# C5 golden fixtures

Golden fixtures for the Rust C5 illuminant estimator, generated by
`tools/scene_models/c5_port.py --stage fixtures` from the first upstream
sample frame (sorted glob of
`.agent/scratch/c5-upstream/images/*_sensorname_*.png`) and the ONNX graph
exported by `--stage export` (`models/artifacts/c5_m7_h64.onnx`).

Regenerate with:

```
.venv-train/bin/python tools/scene_models/c5_port.py --stage fixtures
```

## Files

- `image.bin` — float32 little-endian, HWC RGB, row-major (row, then
  column, then channel), shape 256x384x3 (256\\*384\\*3 = 294912 floats).
  This is the source frame after `ops.read_image` (BGR->RGB, uint8/16 ->
  [0,1] double) and `ops.resize_image` to `[384, 256]` (i.e. width=384,
  height=256 — `cv2.resize` target size is (width, height), so the output
  array is height x width x channel = 256 x 384 x 3).
- `hist_stack.bin` — float32 LE, CHW, shape 4x64x64 (4\\*64\\*64 = 16384
  floats). Channel 0: uv-chroma histogram. Channel 1: edge histogram.
  Channel 2: normalized u-coordinate plane. Channel 3: normalized
  v-coordinate plane. This is exactly the model's `chroma_histograms` ONNX
  input for this frame (the export duplicates it 7x inside the graph; the
  fixture stores only the one stack the Rust side needs to provide).
- `ccc_filter.bin` — float32 LE, CHW, shape 2x64x64. ONNX `ccc_filter`
  output for `hist_stack.bin`, batch dimension stripped.
- `ccc_bias.bin` — float32 LE, shape 64x64. ONNX `ccc_bias` output for
  `hist_stack.bin`, batch dimension stripped.
- `expected.json` — `illuminant_rgb` (from `apply_ccc` on the ONNX
  `ccc_filter`/`ccc_bias` outputs above, batch stripped, plain floats),
  `gt_ill` (ground truth from the frame's `_metadata.json`,
  `illuminant_color_raw` or `gt_ill` key), `hist_boundary` (`[-2.85, 2.85]`),
  `image_hw` (`[256, 384]`), and `source_frame` (the PNG filename).

## Upstream histogram semantics (verified against `.agent/scratch/c5-upstream/src/ops.py`)

All formulas below are transcribed directly from `ops.py`, not summarized
from the paper — read the cited function if in doubt.

### `rgb_to_uv` (log-chroma)

```
EPS = 1e-9
log_rgb = log(rgb + EPS)          # elementwise, rgb = [r, g, b]
u = log_rgb[1] - log_rgb[0]       # log(g + EPS) - log(r + EPS)
v = log_rgb[1] - log_rgb[2]       # log(g + EPS) - log(b + EPS)
```

There is no color-transform matrix in `rgb_to_uv` itself — it is a plain
per-pixel log-ratio of g to r and g to b. (The `A_u`/`A_v` arrays that
appear in `compute_histogram` are histogram bin-center grids, not a color
matrix — see below.)

### Valid-pixel rule (`get_hist_colors`)

A pixel is included in a histogram only if `sum(r, g, b) > EPS` with
`EPS = 1e-9` (from `ops.py`; this excludes exact-zero pixels, e.g. masked
regions). Both the histogram of the image itself and the histogram of the
edge image apply this same rule to their respective triplets.

### Pixel weighting

Each valid pixel's histogram contribution is weighted by
`Iy = sqrt(r^2 + g^2 + b^2)`, i.e. the Euclidean norm of its own [r, g, b]
triplet (for the edge histogram, the triplet is the edge image's [r, g, b]
at that pixel, not the original image's).

### `compute_histogram` — box soft-binning

```
eps = (|boundary[0]| + |boundary[1]|) / (nbins - 1)   # = 5.7 / 63 for nbins=64
A_u = arange(boundary_sorted[0], boundary_sorted[1] + eps/2, eps)  # ascending, 64 bin centers, -2.85..2.85
A_v = flip(A_u)                                        # descending, 2.85..-2.85
```

For every valid pixel `p` with chroma `(u_p, v_p)` and weight `Iy_p`, and
every bin `(i, j)` (row `i` indexes `A_v`, column `j` indexes `A_u`):

```
diff_u = |u_p - A_u[j]|
diff_v = |v_p - A_v[i]|
indicator = 1 if (0 < diff_u <= eps) and (0 < diff_v <= eps) else 0
N[i, j] += Iy_p * indicator
```

i.e. a pixel increments every bin whose center is within `eps` of it in
*both* u and v — a box kernel, not a single nearest-bin assignment, so a
pixel near a bin boundary can land in more than one bin. Note the exact
upstream implementation detail: a pixel whose distance to a bin center is
*exactly* 0 does **not** increment that bin (the code zeroes `diff <= eps`
survivors only via `diff != 0`, so an exact coincidence — vanishingly rare
with continuous log-chroma values — is not counted). Row 0 of `N`
corresponds to `v = +2.85` (top), increasing row index moves to more
negative `v`; column 0 corresponds to `u = -2.85`, increasing column index
moves to more positive `u`.

Final normalization: `N = sqrt(N / (sum(N) + EPS))` with `EPS = 1e-9`.

### Edge image (`compute_edges`)

Reflect-pad the image by 1px on each side (`cv2.BORDER_REFLECT`), then for
each of the 8 neighbor offsets `(dx, dy) in {-1,0,1}^2 \\ {(0,0)}`, accumulate
`|image - shifted_neighbor|` elementwise (per channel), then divide the sum
by 8 — the mean absolute difference to the 8-connected neighborhood.

### `get_uv_coord(hist_size, normalize)` — coordinate planes

```
u_range = arange(-(N-1)/2, (N-1)/2 + 1)   # ascending, N values, N=hist_size
v_range = arange((N-1)/2, -(N-1)/2 - 1, -1)  # descending, N values
u_coord, v_coord = meshgrid(u_range, v_range)   # numpy default 'xy' indexing:
                                                 # u_coord[i,j] = u_range[j]
                                                 # v_coord[i,j] = v_range[i]
if normalize:
    u_coord = (u_coord + (N-1)/2) / (N-1)   # -> [0, 1]
    v_coord = (v_coord + (N-1)/2) / (N-1)   # -> [0, 1]
```

`hist_stack.bin` channels 2/3 are these planes with `normalize=True`
(unbatched, i.e. plain HxW arrays, not the `1xHxW` tensor form
`get_uv_coord` returns when `tensor=True`).

### `from_coord_to_uv(hist_size, u, v)` — coordinate grid to log-chroma

```
coord_range = [-2.85, 2.85]
space_range = coord_range[1] - coord_range[0]   # 5.7
scale = space_range / hist_size                 # 5.7 / 64  (NOT /(hist_size-1))
U = u * scale
V = v * scale
```

### `apply_ccc` post-processing (the part the ONNX graph does NOT do — Rust
owns it, driven by `ccc_filter.bin` / `ccc_bias.bin`)

Given the histogram+edge channels of the input stack `N` (shape `Bx2x64x64`,
channels 0/1 of `hist_stack.bin`), the predicted filter `F` (`ccc_filter`,
`Bx2x64x64`) and bias `B` (`ccc_bias`, `Bx64x64`):

1. `N_fft = rfft2(N)`, `F_fft = rfft2(F)` — 2D real FFT per channel.
2. `heat = irfft2(N_fft * F_fft).sum(dim=channels) + B` — elementwise
   complex multiply (circular convolution theorem), inverse real FFT, sum
   the 2 channels together, add the per-pixel bias.
3. `heat = clamp(heat, -100, 100)`.
4. `P = softmax(flatten(heat))` reshaped back to `64x64` — softmax over the
   whole flattened grid (not per-row/per-column).
5. Expectation over the *unnormalized* (`normalize=False`, i.e. integer-ish
   in `[-(63)/2, 63/2]`) coordinate grids from `get_uv_coord`:
   `u = sum(P * u_coord_unnorm)`, `v = sum(P * v_coord_unnorm)` (summed
   over both spatial dims).
6. `U, V = from_coord_to_uv(64, u, v)` (scale by `5.7/64`, see above).
7. `illuminant_rgb = uv_to_rgb([U, V])`.

### `uv_to_rgb` (log-chroma to RGB)

```
r = exp(-U)
g = 1
b = exp(-V)
rgb = [r, g, b] / ||[r, g, b]||_2     # L2-normalize to a unit vector
```

`illuminant_rgb` in `expected.json` is this final unit-norm `[r, g, b]`.
"""


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--stage", choices=["export", "parity", "fixtures"],
                    required=True)
    ap.add_argument("--upstream", default=str(DEFAULT_UPSTREAM))
    ap.add_argument("--weights", default=None,
                    help="default: <upstream>/models/C5_m_7_h_64.pth")
    ap.add_argument("--export-onnx",
                    default=str(REPO / "models/artifacts/c5_m7_h64.onnx"))
    ap.add_argument("--fixtures-dir",
                    default=str(REPO / "tests/fixtures/c5"))
    args = ap.parse_args()

    weights = Path(args.weights) if args.weights else (
        Path(args.upstream) / "models/C5_m_7_h_64.pth")
    net = load_upstream(Path(args.upstream), weights, "cpu")
    if args.stage == "export":
        export_stage(args, net)
    elif args.stage == "parity":
        parity_stage(args, net)
    else:
        fixtures_stage(args, net)


if __name__ == "__main__":
    main()
