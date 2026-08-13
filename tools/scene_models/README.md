# Scene-model pack

Turn the perception models from `sources/scenedetection.md` into
`lege-gpu`-compatible prepared ONNX graphs.

```bash
# 1. Pinned downloads (YuNet ONNX, MobileOne .pth)
python3 tools/scene_models/fetch.py

# 2. Rename / fuse / stamp YuNet
python3 tools/scene_models/prepare.py \
    models/artifacts/upstream/face_detection_yunet_2023mar.onnx \
    models/artifacts/yunet.prepared.onnx \
    --source-url https://huggingface.co/opencv/face_detection_yunet \
    --license MIT --role face-detector \
    --preprocess '{"color":"bgr","range":"0-255"}'

# 3. Export MobileOne-S0 + Cityscapes / COCO-VOC segmenters (needs torch)
python3 tools/scene_models/export.py

# 4. Inspect a prepared file
python3 tools/scene_models/report.py models/artifacts/yunet.prepared.onnx
python3 tools/scene_models/remap.py cityscapes
```

`models/artifacts/` is gitignored. Commit only `models/manifest.json`,
`models/LICENSE-NOTES.md`, and these scripts.

`lege-gpu` is an unconditional dependency and scene inference prefers its
shared wgpu compute adapter, with the CPU reference executor retained as a
fallback. On the dual-boot Linux host, select the RTX 4060 explicitly for a
hardware-path check with `WGPU_ADAPTER_NAME=4060 WGPU_REQUIRE_REAL_GPU=1`.

A graph is keepable when `report.py` reports no hard-reject ops
(`QuantizeLinear`, `TopK`, `Expand`, …) and as few leftover
`BatchNormalization` / `Clip` / `ConvTranspose` nodes as we can rewrite.
The Cityscapes LR-ASPP is preferred over COCO-VOC because it actually has
sky, vegetation, person, and building.
