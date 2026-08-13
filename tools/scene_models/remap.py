#!/usr/bin/env python3
"""Committed class remaps from pretrained taxonomies onto RegionMasks."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

# Destination names match sources/scenedetection.md RegionMasks.
REGION_CLASSES = (
    "face",
    "person",
    "sky",
    "vegetation",
    "water",
    "snow_or_sand",
    "building_or_interior",
    "text_or_document",
    "salient_foreground",
)

# Cityscapes trainId order used by ekzhang/fastseg and most Cityscapes exports.
CITYSCAPES = [
    "road",
    "sidewalk",
    "building",
    "wall",
    "fence",
    "pole",
    "traffic_light",
    "traffic_sign",
    "vegetation",
    "terrain",
    "sky",
    "person",
    "rider",
    "car",
    "truck",
    "bus",
    "train",
    "motorcycle",
    "bicycle",
]

CITYSCAPES_TO_REGION = {
    "building": "building_or_interior",
    "wall": "building_or_interior",
    "vegetation": "vegetation",
    "terrain": "vegetation",
    "sky": "sky",
    "person": "person",
    "rider": "person",
}

# torchvision lraspp_mobilenet_v3_large / deeplabv3 COCO-VOC 21-class.
COCO_VOC = [
    "background",
    "aeroplane",
    "bicycle",
    "bird",
    "boat",
    "bottle",
    "bus",
    "car",
    "cat",
    "chair",
    "cow",
    "diningtable",
    "dog",
    "horse",
    "motorbike",
    "person",
    "pottedplant",
    "sheep",
    "sofa",
    "train",
    "tvmonitor",
]

COCO_VOC_TO_REGION = {
    "person": "person",
    "pottedplant": "vegetation",
    "boat": "water",
}


def invert_map(source_classes: list[str], forward: dict[str, str]) -> dict[str, list[int]]:
    inverse: dict[str, list[int]] = {name: [] for name in REGION_CLASSES}
    for index, name in enumerate(source_classes):
        dest = forward.get(name)
        if dest is not None:
            inverse[dest].append(index)
    return inverse


REMAPS = {
    "cityscapes": {
        "source": "cityscapes-19",
        "classes": CITYSCAPES,
        "to_region": invert_map(CITYSCAPES, CITYSCAPES_TO_REGION),
    },
    "coco_voc": {
        "source": "coco-voc-21",
        "classes": COCO_VOC,
        "to_region": invert_map(COCO_VOC, COCO_VOC_TO_REGION),
    },
}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("taxonomy", choices=sorted(REMAPS), nargs="?", default=None)
    args = parser.parse_args()
    payload = REMAPS if args.taxonomy is None else {args.taxonomy: REMAPS[args.taxonomy]}
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
