#!/usr/bin/env python3
"""Train the CamSDD 30-way scene/lighting classifier (Model A).

Implements phase 3a of research/LIGHTING_DETECTION_PLAN.md rev 2, with the
gates documented in tools/scene_models/TRAINING.md. Stages:

  probe     Gate 0 pipeline smoke: frozen ImageNet backbone, train the head.
  finetune  Gate 1 baseline: full fine-tune with the challenge-report recipe.
  eval      Gate 2 report: top-1/top-3 + per-class recall + confusion matrix.
  export    Gate 3: ONNX export of a run's best checkpoint for lege-gpu.

Needs torch + torchvision (see TRAINING.md for the venv line).
"""

from __future__ import annotations

import argparse
import json
import random
import re
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DATASET = ROOT / "CamSDD" / "CamSDD"
RUNS = Path(__file__).resolve().parent / "runs"

# Classes whose output actually gates render policy (TRAINING.md Gate 2).
POLICY_CLASSES = ("6_Macro", "21_Night_shot", "24_Candle_light", "26_Indoor", "27_Backlight")

# ImageNet normalization — must match the pretrained backbone's training.
MEAN = (0.485, 0.456, 0.406)
STD = (0.229, 0.224, 0.225)

# Dataset-native resolution is 576x384 (WxH); we train at it by default
# (plan §3: resolution matters, and no larger pixels exist in the dataset).
# --input overrides for the phase-3b resolution ablation.
INPUT_HW = (384, 576)


def class_names() -> list[str]:
    """The 30 folder names, sorted by their numeric prefix. Deterministic."""
    dirs = [p.name for p in (DATASET / "training").iterdir() if p.is_dir()]
    dirs.sort(key=lambda n: int(re.match(r"(\d+)_", n).group(1)))
    if len(dirs) != 30:
        raise SystemExit(f"expected 30 class folders, found {len(dirs)} in {DATASET}/training")
    return dirs


def seed_everything(seed: int) -> None:
    import torch

    random.seed(seed)
    torch.manual_seed(seed)
    torch.cuda.manual_seed_all(seed)


def build_transform(train_aug: bool):
    from torchvision import transforms as T

    if train_aug:
        # The challenge-report augmentation menu (§3): flips, crops, small
        # rotations, photometric jitter, occasional blur. Kept deliberately
        # standard so a Gate-1 miss indicts the pipeline, not exotic tricks.
        tf = T.Compose(
            [
                T.RandomResizedCrop(INPUT_HW, scale=(0.5, 1.0), ratio=(1.2, 1.8)),
                T.RandomHorizontalFlip(),
                T.RandomRotation(10),
                T.ColorJitter(brightness=0.2, contrast=0.2, saturation=0.2, hue=0.02),
                T.RandomApply([T.GaussianBlur(5, sigma=(0.1, 1.5))], p=0.1),
                T.ToTensor(),
                T.Normalize(MEAN, STD),
            ]
        )
    else:
        tf = T.Compose([T.Resize(INPUT_HW), T.ToTensor(), T.Normalize(MEAN, STD)])
    return tf


def build_dataset(split: str, train_aug: bool, classes: list[str]):
    from torchvision.datasets import ImageFolder

    ds = ImageFolder(str(DATASET / split), transform=build_transform(train_aug))
    # ImageFolder sorts folders lexically ("10_" before "1_"); remap targets
    # so index == numeric prefix order, i.e. classes[i] is stable and shared
    # with inference-time mapping in Rust.
    lexical_to_ours = {ds.class_to_idx[name]: i for i, name in enumerate(classes)}
    ds.samples = [(p, lexical_to_ours[t]) for p, t in ds.samples]
    ds.targets = [t for _, t in ds.samples]
    return ds


class PseudoCsvDataset:
    """path,label rows from pseudo_label.py, served with the train transform.

    Labels are hand-reviewed (or confidence-filtered) pseudo-labels on our own
    proxy renders — the ByteScene/ALONG domain-adaptation trick from the plan.
    """

    def __init__(self, csv_path: Path, classes: list[str], train_aug: bool = True):
        self.items = []
        with open(csv_path) as fh:
            header = next(fh)
            assert header.strip() == "path,label", f"unexpected header {header!r}"
            for line in fh:
                path, label = line.strip().rsplit(",", 1)
                self.items.append((path, classes.index(label)))
        self.tf = build_transform(train_aug)

    def __len__(self):
        return len(self.items)

    def __getitem__(self, index):
        from PIL import Image

        path, label = self.items[index]
        return self.tf(Image.open(path).convert("RGB")), label


def build_model(num_classes: int = 30, arch: str = "resnet50"):
    import torch.nn as nn

    if arch == "resnet50":
        from torchvision.models import ResNet50_Weights, resnet50

        model = resnet50(weights=ResNet50_Weights.IMAGENET1K_V2)
        model.fc = nn.Linear(model.fc.in_features, num_classes)
    elif arch == "mobilenet_v2":
        from torchvision.models import MobileNet_V2_Weights, mobilenet_v2

        model = mobilenet_v2(weights=MobileNet_V2_Weights.IMAGENET1K_V2)
        model.classifier[1] = nn.Linear(model.classifier[1].in_features, num_classes)
    else:
        raise SystemExit(f"unknown arch {arch}")
    return model


def head_params(model):
    """The freshly initialized classifier head, whatever the arch calls it."""
    head = model.fc if hasattr(model, "fc") else model.classifier[1]
    return list(head.parameters())


def evaluate(model, loader, device, num_classes: int = 30):
    import torch

    model.eval()
    correct1 = correct3 = total = 0
    per_class_correct = torch.zeros(num_classes)
    per_class_correct3 = torch.zeros(num_classes)
    per_class_total = torch.zeros(num_classes)
    confusion = torch.zeros(num_classes, num_classes, dtype=torch.long)
    with torch.no_grad():
        for images, targets in loader:
            images = images.to(device, non_blocking=True)
            targets = targets.to(device, non_blocking=True)
            with torch.autocast(device_type=device.type, enabled=device.type == "cuda"):
                logits = model(images)
            top3 = logits.topk(3, dim=1).indices
            pred = top3[:, 0]
            in3 = (top3 == targets[:, None]).any(dim=1)
            correct1 += (pred == targets).sum().item()
            correct3 += in3.sum().item()
            total += targets.numel()
            for t, p, hit3 in zip(targets.cpu(), pred.cpu(), in3.cpu()):
                per_class_total[t] += 1
                confusion[t, p] += 1
                if t == p:
                    per_class_correct[t] += 1
                if hit3:
                    per_class_correct3[t] += 1
    recall = (per_class_correct / per_class_total.clamp(min=1)).tolist()
    recall3 = (per_class_correct3 / per_class_total.clamp(min=1)).tolist()
    return {
        "top1": correct1 / total,
        "top3": correct3 / total,
        "per_class_recall": recall,
        "per_class_recall3": recall3,
        "confusion": confusion.tolist(),
        "n": total,
    }


def make_loaders(args, classes):
    from torch.utils.data import ConcatDataset, DataLoader

    train_ds = build_dataset("training", True, classes)
    if getattr(args, "pseudo_csv", None):
        pseudo = PseudoCsvDataset(args.pseudo_csv, classes)
        # Oversample the small own-corpus set so each epoch actually sees it.
        train_ds = ConcatDataset([train_ds] + [pseudo] * args.pseudo_repeat)
        print(f"mixed in {len(pseudo)} pseudo-labeled images x{args.pseudo_repeat}", flush=True)
    val_ds = build_dataset("validation", False, classes)
    train_loader = DataLoader(
        train_ds,
        batch_size=args.batch,
        shuffle=True,
        num_workers=args.workers,
        pin_memory=True,
        drop_last=True,
        persistent_workers=args.workers > 0,
    )
    val_loader = DataLoader(
        val_ds, batch_size=args.batch, shuffle=False, num_workers=args.workers, pin_memory=True
    )
    return train_loader, val_loader


def train_stage(args, classes, probe: bool):
    import torch
    import torch.nn as nn

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    seed_everything(args.seed)
    run_dir = Path(args.run)
    run_dir.mkdir(parents=True, exist_ok=True)

    adapt = args.stage == "adapt"
    if adapt:
        init = torch.load(Path(args.init) / "best.pt", map_location="cpu", weights_only=True)
        args.arch = init.get("arch", "resnet50")
        global INPUT_HW
        INPUT_HW = tuple(init.get("input_hw", INPUT_HW))
    model = build_model(arch=args.arch).to(device)
    if adapt:
        model.load_state_dict(init["model"])
    if device.type == "cuda":
        model = model.to(memory_format=torch.channels_last)

    head = head_params(model)
    epochs = args.epochs or (3 if probe else 5 if adapt else 30)
    if probe:
        for p in model.parameters():
            p.requires_grad = False
        for p in head:
            p.requires_grad = True
        params = [{"params": head, "lr": args.lr or 1e-3}]
    else:
        head_ids = {id(p) for p in head}
        backbone = [p for p in model.parameters() if id(p) not in head_ids]
        # Paper recipe: AdamW, wd 4e-5, label smoothing 0.1. lr is scaled
        # down from the paper's 1.5e-3@batch256 for our smaller batches;
        # the fresh head gets 10x the backbone rate. The adaptation pass is
        # a light touch on an already-converged model: 10x lower again.
        base = args.lr or (1e-5 if adapt else 1e-4)
        params = [
            {"params": backbone, "lr": base},
            {"params": head, "lr": base * 10},
        ]
    optimizer = torch.optim.AdamW(params, weight_decay=4e-5)
    criterion = nn.CrossEntropyLoss(label_smoothing=0.1)

    train_loader, val_loader = make_loaders(args, classes)
    steps_per_epoch = max(1, len(train_loader) // args.accum)
    warmup = 0 if probe else 2 * steps_per_epoch
    total_steps = epochs * steps_per_epoch

    def lr_lambda(step: int) -> float:
        if step < warmup:
            return (step + 1) / warmup
        import math

        t = (step - warmup) / max(1, total_steps - warmup)
        return 0.5 * (1 + math.cos(math.pi * t))

    scheduler = torch.optim.lr_scheduler.LambdaLR(optimizer, lr_lambda)
    scaler = torch.amp.GradScaler(enabled=device.type == "cuda")

    (run_dir / "config.json").write_text(
        json.dumps(
            {
                "stage": args.stage,
                "arch": args.arch,
                "classes": classes,
                "input_hw": INPUT_HW,
                "epochs": epochs,
                "batch": args.batch,
                "accum": args.accum,
                "seed": args.seed,
                "lr": args.lr,
                "device": str(device),
                "torch": torch.__version__,
            },
            indent=2,
        )
    )

    best_top1 = 0.0
    metrics_path = run_dir / "metrics.jsonl"
    for epoch in range(epochs):
        model.train()
        if probe:  # keep frozen BN statistics honest
            for m in model.modules():
                if isinstance(m, nn.BatchNorm2d) and not m.weight.requires_grad:
                    m.eval()
        t0 = time.time()
        running = 0.0
        optimizer.zero_grad(set_to_none=True)
        for i, (images, targets) in enumerate(train_loader):
            images = images.to(device, non_blocking=True)
            if device.type == "cuda":
                images = images.to(memory_format=torch.channels_last)
            targets = targets.to(device, non_blocking=True)
            with torch.autocast(device_type=device.type, enabled=device.type == "cuda"):
                loss = criterion(model(images), targets) / args.accum
            scaler.scale(loss).backward()
            running += loss.item() * args.accum
            if (i + 1) % args.accum == 0:
                scaler.step(optimizer)
                scaler.update()
                optimizer.zero_grad(set_to_none=True)
                scheduler.step()
        stats = evaluate(model, val_loader, device)
        line = {
            "epoch": epoch,
            "train_loss": running / max(1, len(train_loader)),
            "val_top1": stats["top1"],
            "val_top3": stats["top3"],
            "policy_recall": {
                name: stats["per_class_recall"][classes.index(name)] for name in POLICY_CLASSES
            },
            "lr": scheduler.get_last_lr()[0],
            "seconds": round(time.time() - t0, 1),
        }
        with metrics_path.open("a") as fh:
            fh.write(json.dumps(line) + "\n")
        print(
            f"epoch {epoch:3d}  loss {line['train_loss']:.3f}  "
            f"top1 {stats['top1']:.4f}  top3 {stats['top3']:.4f}  {line['seconds']}s",
            flush=True,
        )
        torch.save({"model": model.state_dict(), "epoch": epoch, "classes": classes, "arch": args.arch, "input_hw": INPUT_HW}, run_dir / "last.pt")
        if stats["top1"] > best_top1:
            best_top1 = stats["top1"]
            torch.save({"model": model.state_dict(), "epoch": epoch, "classes": classes, "arch": args.arch, "input_hw": INPUT_HW}, run_dir / "best.pt")

    if probe:
        gate = "GATE 0 (probe)"
        verdict = "PROMISE" if best_top1 >= 0.75 else ("FAILURE — pipeline bug, fix before tuning" if best_top1 < 0.40 else "INCONCLUSIVE — inspect before proceeding")
    elif adapt:
        gate = f"GATE 4 (adapt, baseline {args.baseline:.4f})"
        verdict = (
            "PROMISE — no CamSDD regression"
            if best_top1 >= args.baseline - 0.005
            else "FAILURE — adaptation regressed CamSDD val; inspect pseudo-labels / lower lr"
        )
    else:
        gate = "GATE 1 (finetune)"
        verdict = "PROMISE" if best_top1 >= 0.94 else ("FAILURE band — one retry allowed, then rewrite" if best_top1 < 0.90 else "BELOW PAPER BAND — retry with lr/resolution before Gate 2")
    print(f"{gate}: best val top-1 {best_top1:.4f} -> {verdict}", flush=True)
    return best_top1


def eval_stage(args, classes):
    import torch
    from torch.utils.data import ConcatDataset, DataLoader

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    ckpt = torch.load(Path(args.run) / "best.pt", map_location=device, weights_only=True)
    global INPUT_HW
    INPUT_HW = tuple(ckpt.get("input_hw", INPUT_HW))
    model = build_model(arch=ckpt.get("arch", "resnet50")).to(device)
    model.load_state_dict(ckpt["model"])

    splits = {"val": ["validation"], "test": ["test"], "both": ["validation", "test"]}[args.split]
    ds = ConcatDataset([build_dataset(s, False, classes) for s in splits])
    loader = DataLoader(ds, batch_size=args.batch, num_workers=args.workers, pin_memory=True)
    stats = evaluate(model, loader, device)

    out = Path(args.run) / f"eval_{args.split}.json"
    out.write_text(json.dumps({**stats, "classes": classes}, indent=2))
    print(f"top-1 {stats['top1']:.4f}  top-3 {stats['top3']:.4f}  n={stats['n']}  -> {out}")
    print("\nper-class recall (top-1 / top-3):")
    worst_policy1 = worst_policy3 = 1.0
    for i, name in enumerate(classes):
        mark = "  <- policy" if name in POLICY_CLASSES else ""
        print(f"  {name:24s} {stats['per_class_recall'][i]:.3f} / {stats['per_class_recall3'][i]:.3f}{mark}")
        if name in POLICY_CLASSES:
            worst_policy1 = min(worst_policy1, stats["per_class_recall"][i])
            worst_policy3 = min(worst_policy3, stats["per_class_recall3"][i])
    # Gate 2 (as amended 2026-08-26, see TRAINING.md): the Rust mapping layer
    # consumes the top-k softmax, not the argmax, so the operative promise
    # metric is per-policy-class top-3 recall >= 95%; top-1 recall >= 80% is
    # kept as the hard floor for a genuinely broken class.
    # float32 tolerance: 38/40 must count as 0.95, not 0.94999999.
    if worst_policy3 >= 0.95 - 1e-6 and worst_policy1 >= 0.80 - 1e-6:
        verdict = "PROMISE"
    elif worst_policy1 < 0.80:
        verdict = "FAILURE — targeted class fixes needed"
    else:
        verdict = "INCONCLUSIVE — inspect confusion matrix"
    print(
        f"\nGATE 2 (policy classes): worst top-3 recall {worst_policy3:.3f}, "
        f"worst top-1 recall {worst_policy1:.3f} -> {verdict}"
    )


def export_stage(args, classes):
    import torch

    ckpt = torch.load(Path(args.run) / "best.pt", map_location="cpu", weights_only=True)
    global INPUT_HW
    INPUT_HW = tuple(ckpt.get("input_hw", INPUT_HW))
    model = build_model(arch=ckpt.get("arch", "resnet50"))
    model.load_state_dict(ckpt["model"])
    model.eval()
    dummy = torch.zeros(1, 3, *INPUT_HW)
    out = Path(args.export_onnx)
    out.parent.mkdir(parents=True, exist_ok=True)
    # Raw logits on purpose: prepare.py stamps preprocessing metadata and the
    # Rust side owns softmax + the 30-way -> policy-class mapping.
    torch.onnx.export(
        model,
        dummy,
        str(out),
        input_names=["scene_image"],
        output_names=["scene_logits"],
        opset_version=17,
        dynamo=False,
    )
    print(f"exported {out} (1x3x{INPUT_HW[0]}x{INPUT_HW[1]}, raw logits, opset 17)")
    print("GATE 3 next: python3 tools/scene_models/report.py " + str(out))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", required=True, choices=["probe", "finetune", "adapt", "eval", "export"])
    parser.add_argument("--run", required=True, help="run directory (checkpoints + metrics)")
    parser.add_argument("--arch", default="resnet50", choices=["resnet50", "mobilenet_v2"])
    parser.add_argument("--input", default=None, metavar="HxW", help="input size, e.g. 384x576 (default: dataset-native)")
    parser.add_argument("--epochs", type=int, default=None)
    parser.add_argument("--batch", type=int, default=24)
    parser.add_argument("--accum", type=int, default=4, help="gradient accumulation steps")
    parser.add_argument("--lr", type=float, default=None)
    parser.add_argument("--workers", type=int, default=8)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--split", default="val", choices=["val", "test", "both"])
    parser.add_argument("--export-onnx", type=Path, default=None)
    parser.add_argument("--init", default=None, help="adapt: run dir whose best.pt seeds the adaptation")
    parser.add_argument("--pseudo-csv", type=Path, default=None, help="adapt: pseudo_label.py output")
    parser.add_argument("--pseudo-repeat", type=int, default=4)
    parser.add_argument("--baseline", type=float, default=0.97, help="adapt: CamSDD val top-1 that must not regress")
    args = parser.parse_args()

    if not DATASET.is_dir():
        raise SystemExit(f"CamSDD dataset not found at {DATASET}")
    if args.input:
        global INPUT_HW
        h, w = args.input.lower().split("x")
        INPUT_HW = (int(h), int(w))
    classes = class_names()

    if args.stage == "probe":
        train_stage(args, classes, probe=True)
    elif args.stage == "finetune":
        train_stage(args, classes, probe=False)
    elif args.stage == "adapt":
        if not args.init or not args.pseudo_csv:
            raise SystemExit("--stage adapt requires --init and --pseudo-csv")
        train_stage(args, classes, probe=False)
    elif args.stage == "eval":
        eval_stage(args, classes)
    elif args.stage == "export":
        if not args.export_onnx:
            raise SystemExit("--export-onnx is required for --stage export")
        export_stage(args, classes)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
