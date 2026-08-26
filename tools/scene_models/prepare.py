#!/usr/bin/env python3
"""Rewrite an upstream ONNX graph into a lege-gpu prepared artifact."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def _setup_path() -> None:
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _require_onnx():
    try:
        import onnx
        from onnx import TensorProto, helper, numpy_helper
    except ImportError as exc:
        raise SystemExit("prepare.py needs `onnx` (pip install onnx)") from exc
    return onnx, TensorProto, helper, numpy_helper


def rename_input(model, old: str | None, new: str):
    graph = model.graph
    if old is None:
        initializer = {init.name for init in graph.initializer}
        candidates = [item.name for item in graph.input if item.name not in initializer]
        if len(candidates) != 1:
            raise SystemExit(f"cannot guess image input, found {candidates}")
        old = candidates[0]
    if old == new:
        return old
    for item in graph.input:
        if item.name == old:
            item.name = new
    for node in graph.node:
        for index, name in enumerate(node.input):
            if name == old:
                node.input[index] = new
    return old


def set_nchw_hw(model, input_name: str, height: int, width: int) -> None:
    for item in model.graph.input:
        if item.name != input_name:
            continue
        dims = item.type.tensor_type.shape.dim
        if len(dims) != 4:
            raise SystemExit(f"{input_name} is rank {len(dims)}, expected 4")
        dims[0].Clear()
        dims[0].dim_value = 1
        dims[1].Clear()
        dims[1].dim_value = 3
        dims[2].Clear()
        dims[2].dim_value = height
        dims[3].Clear()
        dims[3].dim_value = width
        return
    raise SystemExit(f"input {input_name} not found")


def simplify(model):
    try:
        import onnxsim
    except ImportError:
        print("onnxsim not installed; skipping simplify", file=sys.stderr)
        return model
    simplified, ok = onnxsim.simplify(model)
    if not ok:
        print("onnxsim reported check failure; keeping unsimplified graph", file=sys.stderr)
        return model
    return simplified


def remaining_bn(model) -> int:
    return sum(1 for node in model.graph.node if node.op_type == "BatchNormalization")


def _clip_bounds(node, tensor_by_name) -> tuple[float, float]:
    """Return (min, max) for a Clip node from attrs or constant inputs."""
    import numpy as np

    min_v, max_v = -3.4028234663852886e38, 3.4028234663852886e38
    for attr in node.attribute:
        if attr.name == "min":
            min_v = float(attr.f)
        elif attr.name == "max":
            max_v = float(attr.f)
    if len(node.input) > 1 and node.input[1]:
        min_v = float(np.array(tensor_by_name[node.input[1]]).reshape(-1)[0])
    if len(node.input) > 2 and node.input[2]:
        max_v = float(np.array(tensor_by_name[node.input[2]]).reshape(-1)[0])
    return min_v, max_v


def rewrite_clip(model) -> int:
    """Replace Clip with Relu/Sub so lege-gpu does not need a Clip kernel.

    clip(x, a, b) = b - relu(b - (relu(x - a) + a))
    """
    onnx, _tensor_proto, helper, numpy_helper = _require_onnx()
    import numpy as np

    graph = model.graph
    tensor_by_name = {init.name: numpy_helper.to_array(init) for init in graph.initializer}
    new_nodes = []
    rewritten = 0
    next_id = 0

    def const(name: str, value: float):
        tensor = numpy_helper.from_array(np.array([value], dtype=np.float32), name=name)
        graph.initializer.append(tensor)
        return name

    for node in graph.node:
        if node.op_type != "Clip":
            new_nodes.append(node)
            continue
        lo, hi = _clip_bounds(node, tensor_by_name)
        src = node.input[0]
        dst = node.output[0]
        prefix = f"__clip{next_id}_"
        next_id += 1
        lo_name = const(prefix + "lo", lo)
        hi_name = const(prefix + "hi", hi)
        shifted = prefix + "shifted"
        relu_lo = prefix + "relu_lo"
        lifted = prefix + "lifted"
        from_hi = prefix + "from_hi"
        relu_hi = prefix + "relu_hi"
        new_nodes.extend(
            [
                helper.make_node("Sub", [src, lo_name], [shifted], name=prefix + "sub_lo"),
                helper.make_node("Relu", [shifted], [relu_lo], name=prefix + "relu_lo"),
                helper.make_node("Add", [relu_lo, lo_name], [lifted], name=prefix + "add_lo"),
                helper.make_node("Sub", [hi_name, lifted], [from_hi], name=prefix + "sub_hi"),
                helper.make_node("Relu", [from_hi], [relu_hi], name=prefix + "relu_hi"),
                helper.make_node("Sub", [hi_name, relu_hi], [dst], name=prefix + "out"),
            ]
        )
        rewritten += 1
    del graph.node[:]
    graph.node.extend(new_nodes)
    return rewritten


def lift_constants(model) -> int:
    """Move `Constant` nodes into the initializer list.

    lege-gpu's compiled graph has no `Constant` op: it expects every constant
    to arrive as an initializer, which is what its shape-inference and
    constant-folding passes read. A torch export emits `Constant` nodes for
    every shape-plumbing literal instead, so a graph that is otherwise entirely
    supported is rejected at load with "shape inference is not implemented for
    op Constant".

    The rewrite is value-preserving by construction — a `Constant` node *is* an
    initializer with extra steps — and only handles the `value` attribute form;
    a sparse or generated Constant is left alone and will still be rejected,
    which is the honest outcome.

    Returns the number of nodes lifted.
    """
    _require_onnx()

    graph = model.graph
    kept = []
    lifted = 0
    for node in graph.node:
        value = None
        if node.op_type == "Constant" and len(node.output) == 1:
            for attr in node.attribute:
                if attr.name == "value":
                    value = attr.t
        if value is None:
            kept.append(node)
            continue
        tensor = graph.initializer.add()
        tensor.CopyFrom(value)
        tensor.name = node.output[0]
        lifted += 1

    if lifted:
        del graph.node[:]
        graph.node.extend(kept)
    return lifted


def rewrite_gather_to_slice(model) -> int:
    """Replace scalar-index `Gather` with `Slice` + `Squeeze`.

    `x[:, i]` on a data tensor exports as `Gather(data, scalar, axis=k)`, which
    lege-gpu has no kernel for. On a scalar index it is exactly a one-element
    slice followed by dropping that axis, both of which the bridge supports, and
    the substitution is value-identical rather than an approximation.

    Only scalar-index Gathers whose index is a constant initializer are
    rewritten; a vector index or a computed one is left alone, since it is not
    the same operation.

    Returns the number of nodes rewritten.
    """
    _onnx, tensor_proto, helper, numpy_helper = _require_onnx()
    import numpy as np

    graph = model.graph
    constants = {init.name: numpy_helper.to_array(init) for init in graph.initializer}
    new_nodes = []
    rewritten = 0

    def const(name: str, values):
        tensor = numpy_helper.from_array(np.asarray(values, dtype=np.int64), name=name)
        graph.initializer.append(tensor)
        return name

    for node in graph.node:
        index = constants.get(node.input[1]) if node.op_type == "Gather" else None
        if index is None or index.ndim != 0:
            new_nodes.append(node)
            continue
        axis = 0
        for attr in node.attribute:
            if attr.name == "axis":
                axis = int(attr.i)
        start = int(index)
        if start < 0:
            # A negative index needs the axis length to turn into an end bound;
            # the exports in this repo never produce one, so leave it be.
            new_nodes.append(node)
            continue

        prefix = f"__gather{rewritten}_"
        sliced = prefix + "sliced"
        new_nodes.append(
            helper.make_node(
                "Slice",
                [
                    node.input[0],
                    const(prefix + "starts", [start]),
                    const(prefix + "ends", [start + 1]),
                    const(prefix + "axes", [axis]),
                ],
                [sliced],
                name=prefix + "slice",
            )
        )
        new_nodes.append(
            helper.make_node(
                "Squeeze",
                [sliced, const(prefix + "squeeze_axes", [axis])],
                [node.output[0]],
                name=prefix + "squeeze",
            )
        )
        rewritten += 1

    if rewritten:
        del graph.node[:]
        graph.node.extend(new_nodes)
    del tensor_proto
    return rewritten


def rewrite_edge_pad(model) -> int:
    """Replace `Pad(mode='edge')` with `Slice` + `Concat` of the border planes.

    `nn.Conv2d(padding_mode='replicate')` exports as an edge-mode `Pad`, which
    lege-gpu's bridge does not implement (it has constant and reflect kernels
    only). Replicating a border is exactly concatenating copies of the first and
    last slab along that axis, so the substitution is value-identical.

    Requires statically known shapes and constant pads — both true after
    `simplify` — and rewrites one axis at a time so a pad on several axes
    chains. A dynamic or negative pad is left alone.

    Returns the number of `Pad` nodes rewritten.
    """
    onnx, _tensor_proto, helper, numpy_helper = _require_onnx()
    import numpy as np

    inferred = onnx.shape_inference.infer_shapes(model)
    shapes = {}
    for value in list(inferred.graph.value_info) + list(inferred.graph.input):
        dims = value.type.tensor_type.shape.dim
        if all(dim.HasField("dim_value") for dim in dims):
            shapes[value.name] = [dim.dim_value for dim in dims]

    graph = model.graph
    constants = {init.name: numpy_helper.to_array(init) for init in graph.initializer}
    new_nodes = []
    rewritten = 0

    def const(name: str, values):
        tensor = numpy_helper.from_array(np.asarray(values, dtype=np.int64), name=name)
        graph.initializer.append(tensor)
        return name

    for node in graph.node:
        mode = None
        for attr in node.attribute:
            if attr.name == "mode":
                mode = attr.s.decode()
        pads = constants.get(node.input[1]) if len(node.input) > 1 else None
        shape = shapes.get(node.input[0])
        if (
            node.op_type != "Pad"
            or mode != "edge"
            or pads is None
            or shape is None
            or len(pads) != 2 * len(shape)
            or any(int(pad) < 0 for pad in pads)
        ):
            new_nodes.append(node)
            continue

        rank = len(shape)
        current = node.input[0]
        current_shape = list(shape)
        prefix = f"__edgepad{rewritten}_"
        step = 0
        for axis in range(rank):
            before, after = int(pads[axis]), int(pads[axis + rank])
            if before == 0 and after == 0:
                continue
            length = current_shape[axis]
            parts = []

            def slab(start: int, end: int, tag: str):
                name = f"{prefix}{step}_{tag}"
                new_nodes.append(
                    helper.make_node(
                        "Slice",
                        [
                            current,
                            const(name + "_starts", [start]),
                            const(name + "_ends", [end]),
                            const(name + "_axes", [axis]),
                        ],
                        [name],
                        name=name,
                    )
                )
                return name

            if before:
                parts += [slab(0, 1, "first")] * before
            parts.append(current)
            if after:
                parts += [slab(length - 1, length, "last")] * after

            joined = f"{prefix}{step}_joined"
            new_nodes.append(
                helper.make_node("Concat", parts, [joined], axis=axis, name=joined)
            )
            current = joined
            current_shape[axis] = length + before + after
            step += 1

        # The last Concat has to carry the Pad's own output name; rename it
        # rather than adding an Identity the bridge would have to lower.
        if current == node.input[0]:
            new_nodes.append(node)
            continue
        new_nodes[-1].output[0] = node.output[0]
        rewritten += 1

    if rewritten:
        del graph.node[:]
        graph.node.extend(new_nodes)
    return rewritten


def rewrite_spatial_reducemean(model) -> int:
    """Rewrite NCHW ReduceMean(axes=[2,3], keepdims=1) to GlobalAveragePool."""
    _onnx, _tensor_proto, helper, _numpy_helper = _require_onnx()
    graph = model.graph
    new_nodes = []
    rewritten = 0
    for node in graph.node:
        if node.op_type != "ReduceMean":
            new_nodes.append(node)
            continue
        attrs = {attr.name: attr for attr in node.attribute}
        axes = None
        if "axes" in attrs:
            axes = list(attrs["axes"].ints)
        keepdims = attrs["keepdims"].i if "keepdims" in attrs else 1
        if axes == [2, 3] and keepdims == 1 and len(node.input) == 1:
            new_nodes.append(
                helper.make_node(
                    "GlobalAveragePool",
                    list(node.input),
                    list(node.output),
                    name=node.name or f"{node.output[0]}_gap",
                )
            )
            rewritten += 1
        else:
            new_nodes.append(node)
    del graph.node[:]
    graph.node.extend(new_nodes)
    return rewritten


def rewrite_avgpool_count_include_pad(model) -> int:
    """Clear count_include_pad when pads are zero; the two modes then match."""
    graph = model.graph
    rewritten = 0
    for node in graph.node:
        if node.op_type != "AveragePool":
            continue
        attrs = {attr.name: attr for attr in node.attribute}
        pads = list(attrs["pads"].ints) if "pads" in attrs else [0, 0, 0, 0]
        include = attrs["count_include_pad"].i if "count_include_pad" in attrs else 0
        if include == 1 and all(pad == 0 for pad in pads):
            attrs["count_include_pad"].i = 0
            rewritten += 1
    return rewritten


def main() -> int:
    _setup_path()
    from tools.scene_models.report import inspect_model
    from tools.scene_models.ops import SCENE_INPUT_NAME

    onnx, _tensor, _helper, _numpy_helper = _require_onnx()

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--input-name", default=None)
    parser.add_argument("--height", type=int, default=512)
    parser.add_argument("--width", type=int, default=512)
    parser.add_argument("--no-static-hw", action="store_true")
    parser.add_argument("--no-simplify", action="store_true")
    parser.add_argument("--source-url", default="")
    parser.add_argument("--license", default="")
    parser.add_argument("--role", default="")
    parser.add_argument("--preprocess", default="{}")
    args = parser.parse_args()

    source_sha = sha256_file(args.input)
    model = onnx.load(str(args.input))
    old_input = rename_input(model, args.input_name, SCENE_INPUT_NAME)
    if not args.no_static_hw:
        set_nchw_hw(model, SCENE_INPUT_NAME, args.height, args.width)
    if not args.no_simplify:
        model = simplify(model)
    rewritten_clip = rewrite_clip(model)
    if rewritten_clip:
        print(f"rewrote {rewritten_clip} Clip node(s) to Relu/Sub", file=sys.stderr)
    rewritten_gap = rewrite_spatial_reducemean(model)
    if rewritten_gap:
        print(
            f"rewrote {rewritten_gap} spatial ReduceMean node(s) to GlobalAveragePool",
            file=sys.stderr,
        )
    rewritten_pool = rewrite_avgpool_count_include_pad(model)
    if rewritten_pool:
        print(
            f"cleared count_include_pad on {rewritten_pool} zero-pad AveragePool node(s)",
            file=sys.stderr,
        )
    if (rewritten_clip or rewritten_gap) and not args.no_simplify:
        model = simplify(model)
    bn = remaining_bn(model)
    if bn:
        print(f"warning: {bn} BatchNormalization node(s) remain", file=sys.stderr)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    onnx.save(model, str(args.output))
    prepared_sha = sha256_file(args.output)
    report = inspect_model(args.output)
    provenance = {
        "prepared": args.output.name,
        "prepared_sha256": prepared_sha,
        "source_path": str(args.input),
        "source_sha256": source_sha,
        "source_url": args.source_url,
        "license": args.license,
        "role": args.role,
        "renamed_input_from": old_input,
        "input_name": SCENE_INPUT_NAME,
        "static_hw": None if args.no_static_hw else [args.height, args.width],
        "preprocess": json.loads(args.preprocess),
        "prepared_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "batchnorm_remaining": bn,
        "inspect": report,
    }
    provenance_path = args.output.with_suffix(".provenance.json")
    provenance_path.write_text(json.dumps(provenance, indent=2) + "\n")
    print(json.dumps({"prepared": str(args.output), "sha256": prepared_sha, "inspect": report}, indent=2))
    if report["hard_reject"]:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
