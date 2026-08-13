#!/usr/bin/env python3
"""Export MobileOne-S0 and segmenter candidates to ONNX, then prepare them."""

from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOLS = Path(__file__).resolve().parent
DEFAULT_MANIFEST = ROOT / "models" / "manifest.json"
MOBILEONE_PY = (
    "https://raw.githubusercontent.com/apple/ml-mobileone/main/mobileone.py"
)


def _setup_path() -> None:
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))


def _require_torch():
    try:
        import torch
    except ImportError as exc:
        raise SystemExit("export.py needs PyTorch (pip install torch torchvision)") from exc
    return torch


def run_prepare(raw: Path, prepared: Path, **kwargs) -> None:
    cmd = [
        sys.executable,
        str(TOOLS / "prepare.py"),
        str(raw),
        str(prepared),
    ]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        if value is True:
            cmd.append(flag)
        elif value is False or value is None:
            continue
        else:
            cmd.extend([flag, str(value)])
    print("+", " ".join(cmd), file=sys.stderr)
    subprocess.check_call(cmd)


def export_mobileone(upstream: Path, raw_onnx: Path, height: int, width: int) -> None:
    torch = _require_torch()
    with tempfile.TemporaryDirectory() as tmp:
        dest = Path(tmp) / "mobileone.py"
        urllib.request.urlretrieve(MOBILEONE_PY, dest)
        spec = importlib.util.spec_from_file_location("apple_mobileone", dest)
        if spec is None or spec.loader is None:
            raise SystemExit("failed to load apple mobileone.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        model = module.mobileone(variant="s0", inference_mode=True)
        checkpoint = torch.load(str(upstream), map_location="cpu")
        if isinstance(checkpoint, dict) and "state_dict" in checkpoint:
            checkpoint = checkpoint["state_dict"]
        model.load_state_dict(checkpoint)
        model.eval()
        dummy = torch.zeros(1, 3, height, width)
        raw_onnx.parent.mkdir(parents=True, exist_ok=True)
        torch.onnx.export(
            model,
            dummy,
            str(raw_onnx),
            input_names=["x"],
            output_names=["logits"],
            opset_version=17,
            dynamo=False,
        )


def _replace_hardswish(module) -> None:
    """Swap geffnet's JIT HardSwish for nn.Hardswish so ONNX emits HardSwish."""
    torch = _require_torch()
    for name, child in list(module.named_children()):
        if type(child).__name__.startswith("HardSwish"):
            setattr(module, name, torch.nn.Hardswish())
        else:
            _replace_hardswish(child)


def export_fastseg_cityscapes(raw_onnx: Path, height: int, width: int) -> None:
    torch = _require_torch()
    try:
        from fastseg import MobileV3Large
    except ImportError as exc:
        raise SystemExit(
            "Cityscapes LR-ASPP export needs `fastseg` (pip install fastseg)"
        ) from exc
    model = MobileV3Large.from_pretrained()
    model.eval()
    _replace_hardswish(model)
    dummy = torch.zeros(1, 3, height, width)
    raw_onnx.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(
        model,
        dummy,
        str(raw_onnx),
        input_names=["image"],
        output_names=["logits"],
        opset_version=17,
        dynamo=False,
    )


def export_torchvision_lraspp(raw_onnx: Path, height: int, width: int) -> None:
    torch = _require_torch()
    try:
        from torchvision.models.segmentation import (
            LRASPP_MobileNet_V3_Large_Weights,
            lraspp_mobilenet_v3_large,
        )
    except ImportError as exc:
        raise SystemExit("COCO-VOC LR-ASPP export needs torchvision") from exc
    weights = LRASPP_MobileNet_V3_Large_Weights.DEFAULT
    model = lraspp_mobilenet_v3_large(weights=weights)
    model.eval()

    class Logits(torch.nn.Module):
        def __init__(self, inner):
            super().__init__()
            self.inner = inner

        def forward(self, image):
            return self.inner(image)["out"]

    wrapped = Logits(model)
    dummy = torch.zeros(1, 3, height, width)
    raw_onnx.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(
        wrapped,
        dummy,
        str(raw_onnx),
        input_names=["image"],
        output_names=["logits"],
        opset_version=17,
        dynamo=False,
    )


def main() -> int:
    _setup_path()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument(
        "--only",
        action="append",
        default=[],
        help="mobileone, cityscapes, coco_voc",
    )
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    artifacts = ROOT / manifest["artifacts_dir"]
    wanted = set(args.only) or {"mobileone", "cityscapes", "coco_voc"}

    if "mobileone" in wanted:
        upstream = artifacts / "upstream" / manifest["upstream"]["mobileone_s0"]["filename"]
        if not upstream.exists():
            raise SystemExit(f"missing {upstream}; run tools/scene_models/fetch.py first")
        raw = artifacts / "raw" / "mobileone_s0.onnx"
        prepared = artifacts / "mobileone_s0.prepared.onnx"
        export_mobileone(upstream, raw, 224, 224)
        spec = manifest["prepared"]["mobileone_s0"]
        run_prepare(
            raw,
            prepared,
            height=224,
            width=224,
            source_url=manifest["upstream"]["mobileone_s0"]["url"],
            license=spec["license"],
            role=spec["role"],
            preprocess=json.dumps(spec["preprocess"]),
        )

    if "cityscapes" in wanted:
        raw = artifacts / "raw" / "lraspp_mnv3_cityscapes.onnx"
        prepared = artifacts / "lraspp_mnv3_cityscapes.prepared.onnx"
        export_fastseg_cityscapes(raw, 512, 512)
        spec = manifest["prepared"]["lraspp_mnv3_cityscapes"]
        run_prepare(
            raw,
            prepared,
            height=512,
            width=512,
            source_url=spec["source_url"],
            license=spec["license"],
            role=spec["role"],
            preprocess=json.dumps(spec["preprocess"]),
        )

    if "coco_voc" in wanted:
        raw = artifacts / "raw" / "lraspp_mnv3_coco.onnx"
        prepared = artifacts / "lraspp_mnv3_coco.prepared.onnx"
        export_torchvision_lraspp(raw, 512, 512)
        spec = manifest["prepared"]["lraspp_mnv3_coco"]
        run_prepare(
            raw,
            prepared,
            height=512,
            width=512,
            source_url=spec["source_url"],
            license=spec["license"],
            role=spec["role"],
            preprocess=json.dumps(spec["preprocess"]),
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
