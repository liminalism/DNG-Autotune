"""Ops lege-gpu will accept after preparation.

Mirrors `lege-gpu/src/vision/onnx/load.rs` so inspect/prepare can reject a
graph before the Rust session does.
"""

from __future__ import annotations

# Registered lege-gpu `ModelTarget` input names (`load.rs::TARGETS`). A graph
# whose primary input is not one of these is rejected by the Rust session
# before a single op runs, so exports must use the registered name verbatim.
SCENE_INPUT_NAME = "scene_image"
C5_INPUT_NAME = "chroma_histograms"

HARD_REJECT = frozenset(
    {
        "QuantizeLinear",
        "DequantizeLinear",
        "TopK",
        "GatherElements",
        "Range",
        "Expand",
        "Tile",
    }
)

SUPPORTED = frozenset(
    {
        "AveragePool",
        "Add",
        "Concat",
        "Constant",
        "Conv",
        "CumSum",
        "DepthToSpace",
        "Div",
        "Flatten",
        "Gemm",
        "GridSample",
        "GlobalAveragePool",
        "HardSigmoid",
        "HardSwish",
        "Identity",
        "MatMul",
        "Max",
        "MaxPool",
        "Mul",
        "PRelu",
        "Pad",
        "Pow",
        "ReduceMean",
        "ReduceSum",
        "Relu",
        "Resize",
        "Reshape",
        "Sigmoid",
        "Slice",
        "Softmax",
        "SpaceToDepth",
        "Split",
        "Sqrt",
        "Sub",
        "Squeeze",
        "Transpose",
        "Unsqueeze",
    }
)

# Folded at prep time in lege-gpu; allowed to remain only if they actually fold.
FOLDABLE = frozenset(
    {
        "Cast",
        "ConstantOfShape",
        "Equal",
        "Gather",
        "Neg",
        "Not",
        "Shape",
    }
)

SCENE_INPUT_NAME = "scene_image"
