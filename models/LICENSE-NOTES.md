# Scene-model licenses

Weights are not committed. This file records what a fetch/export may download
and whether those files can be redistributed with this crate.

| Artifact | Upstream | License | Redistribute? |
| --- | --- | --- | --- |
| YuNet `face_detection_yunet_2023mar.onnx` | OpenCV Zoo / Shiqi Yu | MIT | Yes, with notice |
| MobileOne-S0 fused ImageNet checkpoint | Apple ml-mobileone | Apple sample-code license | **Do not ship** until audited |
| `ekzhang/fastseg` MobileV3-Large Cityscapes | ekzhang/fastseg | MIT | Yes, with notice |
| torchvision LR-ASPP MobileNetV3-Large COCO | torchvision | BSD-3-Clause | Yes, with notice |
| CamSDD-trained scene classifier (`camsdd_resnet50`) | trained in-repo on CamSDD (ETH Zurich) | CC BY-NC-SA 4.0 (inherited from dataset) | **Only** under BY-NC-SA terms: attribution to CamSDD, non-commercial, share-alike. See `tools/scene_models/TRAINING.md`. |
| C5 illuminant estimator (`c5_m7_h64.onnx`) | mahmoudnafifi/C5 (Google LLC), weights `C5_m_7_h_64.pth`, converted by `tools/scene_models/c5_port.py` | Apache-2.0 | Yes, with notice |

Prepared ONNX files inherit the upstream weight license. `models/manifest.json`
pins hashes after a successful fetch; empty `sha256` fields mean “not yet
pinned.” The torchvision COCO-VOC LR-ASPP fallback was not exported: the
Cityscapes MobileNetV3-LR-ASPP graph prepared and runs on `lege-gpu` CPU,
and it is the one that remaps onto sky/person/vegetation/building.

ImageNet logits from MobileOne are a backbone for a later custom head. They
are not scene-class probabilities and must not be written into `SceneScores`.
