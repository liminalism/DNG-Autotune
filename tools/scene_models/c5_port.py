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
  model input -> 1 x 7 x 4 x 64 x 64 (m=7 encoders). Upstream fills the six
  additional slots with RANDOM sibling images from the same sensor; we
  duplicate the frame's own stack instead — deterministic (a file must develop
  identically alone or in a batch), and the degenerate case the architecture
  tolerates (cross-pooling over identical copies is a no-op). The parity
  stage measures what that choice costs on the in-repo sample set.

Usage:
  .venv-train/bin/python tools/scene_models/c5_port.py --stage export
  .venv-train/bin/python tools/scene_models/c5_port.py --stage parity
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
    """torch.nn.Module wrapper built lazily (torch import stays in main)."""

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

    wrapper = C5Heads(make_exportable(net))
    wrapper.eval()
    dummy = torch.zeros(1, DATA_NUM, 4, HIST_SIZE, HIST_SIZE)
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
    print(f"exported {out} (1x{DATA_NUM}x4x{HIST_SIZE}x{HIST_SIZE} -> "
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
    wrapper = C5Heads(net)
    wrapper.eval()

    print(f"{'frame':44s} {'mode':10s} {'torch-vs-onnx':>13s} "
          f"{'ang.err torch':>13s} {'ang.err onnx':>12s}")
    worst_gap, errs_dup, errs_sib = 0.0, [], []
    for i, (f, stack, gt) in enumerate(zip(frames, stacks, gts)):
        for mode in ("duplicate", "siblings"):
            if mode == "duplicate":
                model_in = np.stack([stack] * DATA_NUM)[None]
            else:
                sibs = [stacks[j] for j in range(len(stacks)) if j != i]
                fill = [sibs[k % len(sibs)] for k in range(DATA_NUM - 1)]
                model_in = np.stack([stack] + fill)[None]
            t_in = torch.from_numpy(model_in)
            N = t_in[:, 0]
            with torch.no_grad():
                F_t, B_t = wrapper(t_in)
                rgb_t = apply_ccc(N, F_t, B_t).numpy()[0]
            F_o, B_o = sess.run(None, {"chroma_histograms": model_in})
            with torch.no_grad():
                rgb_o = apply_ccc(N, torch.from_numpy(F_o),
                                  torch.from_numpy(B_o)).numpy()[0]
            gap = angular_error_deg(rgb_t, rgb_o)
            worst_gap = max(worst_gap, gap)
            err_t = angular_error_deg(rgb_t, gt)
            err_o = angular_error_deg(rgb_o, gt)
            (errs_dup if mode == "duplicate" else errs_sib).append(err_o)
            print(f"{f.name[:44]:44s} {mode:10s} {gap:12.4f}° "
                  f"{err_t:12.2f}° {err_o:11.2f}°")

    import statistics
    print(f"\ntorch-vs-onnx worst angular gap: {worst_gap:.4f}°")
    print(f"mean angular error vs GT — duplicate mode: "
          f"{statistics.mean(errs_dup):.2f}°, siblings mode: "
          f"{statistics.mean(errs_sib):.2f}° (n={len(errs_dup)} frames)")
    verdict = "PROMISE" if worst_gap < 0.1 else "FAILURE — export diverges"
    print(f"PARITY: {verdict}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--stage", choices=["export", "parity"], required=True)
    ap.add_argument("--upstream", default=str(DEFAULT_UPSTREAM))
    ap.add_argument("--weights", default=None,
                    help="default: <upstream>/models/C5_m_7_h_64.pth")
    ap.add_argument("--export-onnx",
                    default=str(REPO / "models/artifacts/c5_m7_h64.onnx"))
    args = ap.parse_args()

    weights = Path(args.weights) if args.weights else (
        Path(args.upstream) / "models/C5_m_7_h_64.pth")
    net = load_upstream(Path(args.upstream), weights, "cpu")
    if args.stage == "export":
        export_stage(args, net)
    else:
        parity_stage(args, net)


if __name__ == "__main__":
    main()
