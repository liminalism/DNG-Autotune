#!/usr/bin/env python3
"""Print ONNX I/O, dtype, and op histogram; exit 1 if lege-gpu would hard-reject it."""

from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from pathlib import Path


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def _setup_path() -> None:
    root = str(_repo_root())
    if root not in sys.path:
        sys.path.insert(0, root)


def _require_onnx():
    try:
        import onnx
    except ImportError as exc:
        raise SystemExit(
            "report.py needs the `onnx` package (pip install onnx)"
        ) from exc
    return onnx


def inspect_model(path: Path) -> dict:
    from tools.scene_models.ops import FOLDABLE, HARD_REJECT, SUPPORTED

    onnx = _require_onnx()
    model = onnx.load(str(path))
    graph = model.graph
    histogram: Counter[str] = Counter(node.op_type for node in graph.node)
    initializer_names = {init.name for init in graph.initializer}

    def value_info(info) -> dict:
        ttype = info.type.tensor_type
        dims = []
        for dim in ttype.shape.dim:
            if dim.HasField("dim_value"):
                dims.append(dim.dim_value)
            elif dim.HasField("dim_param"):
                dims.append(dim.dim_param)
            else:
                dims.append(None)
        elem = ttype.elem_type
        return {
            "name": info.name,
            "dtype": onnx.TensorProto.DataType.Name(elem) if elem else "UNDEFINED",
            "shape": dims,
        }

    return {
        "path": str(path),
        "ir_version": model.ir_version,
        "producer": model.producer_name or "",
        "opsets": [
            f"{imp.domain or 'ai.onnx'}:{imp.version}" for imp in model.opset_import
        ],
        "inputs": [
            value_info(item) for item in graph.input if item.name not in initializer_names
        ],
        "outputs": [value_info(item) for item in graph.output],
        "node_count": len(graph.node),
        "initializer_count": len(graph.initializer),
        "op_histogram": dict(
            sorted(histogram.items(), key=lambda item: (-item[1], item[0]))
        ),
        "hard_reject": sorted(op for op in histogram if op in HARD_REJECT),
        "unsupported": sorted(
            op
            for op in histogram
            if op not in SUPPORTED and op not in FOLDABLE and op not in HARD_REJECT
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("model", type=Path)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    report = inspect_model(args.model)
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(f"path: {report['path']}")
        print(f"ir_version: {report['ir_version']}")
        print(f"producer: {report['producer']}")
        print(f"opsets: {', '.join(report['opsets'])}")
        print(f"nodes: {report['node_count']}")
        print(f"initializers: {report['initializer_count']}")
        print("\ninputs:")
        for item in report["inputs"]:
            print(f"  {item['name']}: {item['dtype']} {item['shape']}")
        print("\noutputs:")
        for item in report["outputs"]:
            print(f"  {item['name']}: {item['dtype']} {item['shape']}")
        print("\nop histogram:")
        for op, count in report["op_histogram"].items():
            print(f"  {op}: {count}")
        if report["hard_reject"]:
            print("\nhard reject:")
            for op in report["hard_reject"]:
                print(f"  - {op}")
        if report["unsupported"]:
            print("\nunsupported (must rewrite or fold):")
            for op in report["unsupported"]:
                print(f"  - {op}")
        if not report["hard_reject"] and not report["unsupported"]:
            print("\ntarget: ops look compatible with lege-gpu")
    return 1 if report["hard_reject"] else 0


if __name__ == "__main__":
    _setup_path()
    raise SystemExit(main())
