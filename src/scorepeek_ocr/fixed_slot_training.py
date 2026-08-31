from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import tempfile
import time
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import cv2
import numpy as np
import onnx
import paddle
import paddle.nn.functional as F
from onnx import TensorProto, helper, numpy_helper


CLASSES = "_0123456789"
FEATURE_DIMENSIONS = 2244
HIDDEN_DIMENSIONS = 64
EPOCHS = 40
PREPROCESSOR_ID = "scorepeek-fixed-slot-hog-hybrid-0p25-v1"
MODEL_SCHEMA = "scorepeek-private-numeric-model-runtime-v2"
REPORT_SCHEMA = "scorepeek-private-fixed-slot-hog-mlp-build-v1"
MANDATORY = ("current_score", "pgreat", "great", "good", "bad", "poor")
FAMILIES = {
    "level": "level",
    "notes": "notes",
    "current_score": "score",
    "previous_score": "score",
    "pgreat": "judgment",
    "great": "judgment",
    "good": "judgment",
    "bad": "judgment",
    "poor": "judgment",
    "previous_miss_count": "supplemental",
    "miss_count": "supplemental",
    "fast": "supplemental",
    "slow": "supplemental",
    "combo_break": "supplemental",
}


class FixedSlotError(RuntimeError):
    """The fixed-slot training or export contract was not satisfied."""


@dataclass(frozen=True)
class Cell:
    session: str
    episode: str
    sequence: int
    field: str
    stable: bool
    slot: int
    label: str
    digest: str
    image: np.ndarray


@dataclass
class Row:
    session: str
    episode: str
    sequence: int
    field: str
    truth: str
    stable: bool
    cells: list[Cell]


class Classifier(paddle.nn.Layer):
    def __init__(self) -> None:
        super().__init__()
        self.hidden = paddle.nn.Linear(FEATURE_DIMENSIONS, HIDDEN_DIMENSIONS)
        self.output = paddle.nn.Linear(HIDDEN_DIMENSIONS, len(CLASSES))

    def forward(self, value: paddle.Tensor) -> paddle.Tensor:
        return self.output(F.relu(self.hidden(value)))


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode() + b"\n"


def load_truth(store: Path) -> tuple[str, dict[tuple[str, str], dict[str, Any]]]:
    active = json.loads((store / "active-suite.json").read_text())["generation_sha256"]
    suite = json.loads((store / "suites" / f"{active}.json").read_text())
    truth: dict[tuple[str, str], dict[str, Any]] = {}
    for entry in suite["entries"]:
        label = json.loads((store / "labels" / f'{entry["label_sha256"]}.json').read_text())
        for episode in label["episodes"]:
            truth[(entry["session_sha256"], episode["episode_id"])] = {
                "stable": set(episode["stable_sequences"]),
                "difficulty": episode["expected_result"]["difficulty"],
            }
    return active, truth


def field_cells(layout: dict[str, Any], field: str, difficulty: str, label: str) -> list[dict[str, int]]:
    if field != "level":
        return layout[field]["digit_cells"]
    matched = [
        item["digit_cells"] for item in layout["level"]
        if item["difficulty"] == difficulty and item["displayed_digits"] == len(label)
    ]
    if len(matched) != 1:
        raise FixedSlotError(f"unsupported level layout: {difficulty} {label}")
    return matched[0]


def cell_digest(image: np.ndarray) -> str:
    digest = hashlib.sha256()
    digest.update(np.asarray(image.shape, dtype=np.uint32).tobytes())
    digest.update(image.tobytes())
    return digest.hexdigest()


def has_not_displayed_marker(image: np.ndarray) -> tuple[bool, dict[str, int]]:
    low = image.min(axis=2)
    high = image.max(axis=2)
    white = (low >= 145) & ((high - low) <= 70)
    band = white[28:45, :]
    row_counts = band.sum(axis=1)
    maximum_row = int(row_counts.max(initial=0))
    long_rows = int((row_counts >= 40).sum())
    occupied_columns = int(band.any(axis=0).sum())
    detected = (
        70 <= maximum_row <= 78
        and 2 <= long_rows <= 3
        and 70 <= occupied_columns <= 78
    )
    return detected, {
        "maximum_row": maximum_row,
        "long_rows": long_rows,
        "occupied_columns": occupied_columns,
    }


def load_rows(dataset: Path, layout_path: Path, store: Path) -> tuple[dict[str, Any], dict[str, Any], str, list[Row], set[str], list[dict[str, Any]]]:
    manifest_bytes = (dataset / "manifest.json").read_bytes()
    manifest = json.loads(manifest_bytes)
    layout = json.loads(layout_path.read_text())
    suite, truth = load_truth(store)
    if manifest.get("suite_sha256") != suite:
        raise FixedSlotError("dataset does not bind the active suite")
    rows: list[Row] = []
    marker_rows = []
    bindings: dict[str, tuple[str, set[str]]] = {}
    for source in manifest["samples"]:
        metadata = truth[(source["session_sha256"], source["episode_id"])]
        image = cv2.imread(str(dataset / source["filename"]), cv2.IMREAD_COLOR)
        if image is None:
            raise FixedSlotError(f"undecodable crop: {source['filename']}")
        if source["field"] in {"previous_score", "previous_miss_count", "miss_count"}:
            detected, metrics = has_not_displayed_marker(image)
            expected = source["label"] == "--"
            marker_rows.append({
                "session": source["session_sha256"],
                "episode": source["episode_id"],
                "sequence": source["sequence"],
                "field": source["field"],
                "truth": source["label"],
                "expected": expected,
                "detected": detected,
                "correct": detected == expected,
                **metrics,
            })
        if not source["label"].isdigit():
            continue
        cells = field_cells(layout, source["field"], metadata["difficulty"], source["label"])
        if len(source["label"]) > len(cells):
            raise FixedSlotError("numeric label does not fit its fixed cells")
        labels = ["_"] * (len(cells) - len(source["label"])) + list(source["label"])
        extracted = []
        for slot, (region, label) in enumerate(zip(cells, labels, strict=True)):
            x0 = region["x"] - source["roi"]["x"]
            y0 = region["y"] - source["roi"]["y"]
            x1 = x0 + region["width"]
            y1 = y0 + region["height"]
            if x0 < 0 or y0 < 0 or x1 > image.shape[1] or y1 > image.shape[0]:
                raise FixedSlotError("fixed cell leaves its owning crop")
            pixels = image[y0:y1, x0:x1].copy()
            digest = cell_digest(pixels)
            binding = bindings.setdefault(digest, (label, set()))
            if binding[0] != label:
                raise FixedSlotError(f"cell digest has conflicting truth: {digest}")
            binding[1].add(source["session_sha256"])
            extracted.append(Cell(
                source["session_sha256"], source["episode_id"], source["sequence"],
                source["field"], source["sequence"] in metadata["stable"], slot,
                label, digest, pixels,
            ))
        rows.append(Row(
            source["session_sha256"], source["episode_id"], source["sequence"],
            source["field"], source["label"], source["sequence"] in metadata["stable"], extracted,
        ))
    cross_session = {digest for digest, (_, sessions) in bindings.items() if len(sessions) > 1}
    manifest["manifest_sha256"] = sha256_bytes(manifest_bytes)
    return manifest, layout, suite, rows, cross_session, marker_rows


def hard_mask(image: np.ndarray, field: str) -> np.ndarray:
    blue, green, red = cv2.split(image)
    if field == "level":
        selected = (red >= 180) & (green >= 70) & (blue <= 120) & ((red.astype(int) - blue) >= 80)
    elif field in {"current_score", "miss_count"}:
        selected = (blue >= 150) & (green >= 130) & (red <= 170) & ((blue.astype(int) - red) >= 35)
    else:
        low = image.min(axis=2)
        high = image.max(axis=2)
        selected = (low >= 145) & ((high - low) <= 70)
    return cv2.morphologyEx(selected.astype(np.uint8) * 255, cv2.MORPH_CLOSE, np.ones((2, 2), np.uint8))


def soft_mask(image: np.ndarray, field: str) -> np.ndarray:
    value = image.astype(np.float64)
    blue, green, red = cv2.split(value)
    if field == "level":
        selected = np.clip((red - blue) / 160.0, 0.0, 1.0) * np.clip((red - 80.0) / 175.0, 0.0, 1.0)
    elif field in {"current_score", "miss_count"}:
        selected = np.clip((((blue + green) * 0.5) - red) / 100.0, 0.0, 1.0) * np.clip((np.minimum(blue, green) - 70.0) / 185.0, 0.0, 1.0)
    else:
        low = value.min(axis=2)
        high = value.max(axis=2)
        selected = np.clip((low - 70.0) / 185.0, 0.0, 1.0) * np.clip((110.0 - (high - low)) / 110.0, 0.0, 1.0)
    return np.round(selected * 255.0).astype(np.uint8)


def hog_pixels(masked: np.ndarray, pixels_per_cell: tuple[int, int], include_pixels: bool) -> np.ndarray:
    # The durable preprocessor uses the repository's OpenCV-compatible fixed-point linear resize
    # in Rust; keep the offline reference on the same interpolation contract.
    canvas = cv2.resize(masked, (24, 32), interpolation=cv2.INTER_LINEAR)
    value = scorepeek_hog(canvas, pixels_per_cell[0])
    if include_pixels:
        pixels = canvas.astype(np.float64).reshape(-1) / 255.0
        norm = np.linalg.norm(pixels)
        if norm:
            pixels /= norm
        value = np.concatenate((value, pixels))
    norm = np.linalg.norm(value)
    return value if norm == 0 else value / norm


def scorepeek_hog(image: np.ndarray, pixels_per_cell: int) -> np.ndarray:
    value = image.astype(np.float64)
    row = np.zeros_like(value)
    column = np.zeros_like(value)
    row[1:-1, :] = value[2:, :] - value[:-2, :]
    column[:, 1:-1] = value[:, 2:] - value[:, :-2]
    magnitude = np.hypot(column, row)
    orientation = np.rad2deg(np.arctan2(row, column)) % 180.0
    cells_y = image.shape[0] // pixels_per_cell
    cells_x = image.shape[1] // pixels_per_cell
    histogram = np.zeros((cells_y, cells_x, 9), dtype=np.float64)
    step = 180.0 / 9.0
    for cell_y in range(cells_y):
        for cell_x in range(cells_x):
            y0 = cell_y * pixels_per_cell
            x0 = cell_x * pixels_per_cell
            for direction in range(9):
                total = np.float32(0.0)
                lower = step * direction
                upper = step * (direction + 1)
                for y in range(y0, y0 + pixels_per_cell):
                    for x in range(x0, x0 + pixels_per_cell):
                        if lower <= orientation[y, x] < upper:
                            total = np.float32(total + magnitude[y, x])
                histogram[cell_y, cell_x, direction] = float(total) / (pixels_per_cell * pixels_per_cell)
    blocks = []
    for cell_y in range(cells_y - 1):
        for cell_x in range(cells_x - 1):
            block = histogram[cell_y:cell_y + 2, cell_x:cell_x + 2, :].reshape(-1)
            block = block / np.sqrt(np.sum(block**2) + 1e-10)
            block = np.minimum(block, 0.2)
            block = block / np.sqrt(np.sum(block**2) + 1e-10)
            blocks.append(block)
    return np.concatenate(blocks)


def feature(image: np.ndarray, field: str) -> np.ndarray:
    coarse = hog_pixels(hard_mask(image, field), (8, 8), False)
    fine = hog_pixels(soft_mask(image, field), (4, 4), True)
    value = np.concatenate((coarse, fine * 0.25))
    norm = np.linalg.norm(value)
    value = value if norm == 0 else value / norm
    if value.shape != (FEATURE_DIMENSIONS,):
        raise FixedSlotError(f"unexpected feature shape: {value.shape}")
    return value.astype(np.float32)


def variants(cell: Cell) -> list[np.ndarray]:
    image = cell.image
    rng = np.random.default_rng(int(cell.digest[:16], 16))
    height, width = image.shape[:2]
    contrast = np.clip(image.astype(np.float32) * 1.08 + 6.0, 0, 255).astype(np.uint8)
    blurred = cv2.GaussianBlur(image, (3, 3), 0.55).astype(np.float32)
    blurred += rng.normal(0.0, 2.0, image.shape)
    blurred = np.clip(blurred, 0, 255).astype(np.uint8)
    down = cv2.resize(image, (max(1, round(width * 0.9)), max(1, round(height * 0.9))), interpolation=cv2.INTER_AREA)
    down = cv2.resize(down, (width, height), interpolation=cv2.INTER_LINEAR)
    shifted = cv2.warpAffine(image, np.float32([[1, 0, 1], [0, 1, 0]]), (width, height), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
    return [image, contrast, blurred, down, shifted]


def train(cells: list[Cell]) -> tuple[Classifier, dict[str, Any]]:
    unique = {cell.digest: cell for cell in cells}
    ordered = [unique[key] for key in sorted(unique)]
    inputs = []
    labels = []
    for cell in ordered:
        for image in variants(cell):
            inputs.append(feature(image, cell.field))
            labels.append(CLASSES.index(cell.label))
    x = np.stack(inputs).astype(np.float32)
    y = np.asarray(labels, dtype=np.int64)
    counts = Counter(y.tolist())
    if set(counts) != set(range(len(CLASSES))):
        raise FixedSlotError("training data does not contain every slot class")
    weights = np.asarray([1.0 / math.sqrt(counts[index]) for index in range(len(CLASSES))], dtype=np.float32)
    weights /= weights.mean()
    paddle.seed(0)
    np.random.seed(0)
    model = Classifier()
    optimizer = paddle.optimizer.Adam(learning_rate=0.001, parameters=model.parameters(), weight_decay=1e-4)
    generator = np.random.default_rng(0)
    model.train()
    final_loss = 0.0
    for _ in range(EPOCHS):
        permutation = generator.permutation(len(x))
        for start in range(0, len(x), 128):
            indices = permutation[start:start + 128]
            logits = model(paddle.to_tensor(x[indices]))
            loss = F.cross_entropy(logits, paddle.to_tensor(y[indices]), weight=paddle.to_tensor(weights))
            loss.backward()
            optimizer.step()
            optimizer.clear_grad()
            final_loss = float(loss)
    return model, {
        "unique_training_cells": len(ordered),
        "augmented_training_cells": len(x),
        "class_counts": dict(sorted(Counter(cell.label for cell in ordered).items())),
        "final_batch_loss": final_loss,
    }


def valid_sequences(field: str, slots: int) -> list[tuple[str, list[int]]]:
    if field == "notes":
        return [(f"{value:04d}", [int(digit) + 1 for digit in f"{value:04d}"]) for value in range(10000)]
    lower, upper = ((1, 9) if slots == 1 else (10, 12)) if field == "level" else (0, 10**slots - 1)
    result = []
    for value in range(lower, upper + 1):
        text = str(value)
        result.append((text, [0] * (slots - len(text)) + [int(digit) + 1 for digit in text]))
    return result


def decode(probabilities: np.ndarray, field: str) -> dict[str, Any]:
    logp = np.log(np.maximum(probabilities.astype(np.float64), np.finfo(np.float64).tiny))
    scored = [(float(sum(logp[index, token] for index, token in enumerate(tokens))), text) for text, tokens in valid_sequences(field, len(probabilities))]
    scored.sort(key=lambda item: (-item[0], item[1]))
    maximum = scored[0][0]
    normalizer = maximum + math.log(sum(math.exp(score - maximum) for score, _ in scored))
    candidates = [{"text": text, "log_probability": score, "probability": math.exp(score - normalizer)} for score, text in scored[:8]]
    return {
        "candidates": candidates,
        "all_blank_log_probability": float(sum(logp[:, 0])),
        "runner_up_margin": candidates[0]["log_probability"] - candidates[1]["log_probability"],
        "raw_text": "".join(CLASSES[int(index)] for index in probabilities.argmax(axis=1)),
    }


def infer(model: Classifier, rows: list[Row]) -> list[dict[str, Any]]:
    flat = [cell for row in rows for cell in row.cells]
    x = np.stack([feature(cell.image, cell.field) for cell in flat]).astype(np.float32)
    model.eval()
    with paddle.no_grad():
        probabilities = F.softmax(model(paddle.to_tensor(x)), axis=1).numpy()
    output = []
    offset = 0
    for row in rows:
        current = decode(probabilities[offset:offset + len(row.cells)], row.field)
        offset += len(row.cells)
        current.update({
            "session": row.session, "episode": row.episode, "sequence": row.sequence,
            "field": row.field, "truth": row.truth, "stable": row.stable,
            "correct": current["candidates"][0]["text"] == row.truth,
        })
        output.append(current)
    return output


def calibrate(rows: list[dict[str, Any]]) -> dict[str, dict[str, float | bool]]:
    result: dict[str, dict[str, float | bool]] = {}
    for family in ("level", "notes", "score", "judgment", "supplemental"):
        selected = [row for row in rows if row["stable"] and FAMILIES[row["field"]] == family]
        wrong = [row for row in selected if not row["correct"]]
        threshold = max((row["candidates"][0]["probability"] for row in wrong), default=0.0)
        if wrong:
            threshold = min(1.0, float(np.nextafter(np.float32(threshold), np.float32(1.0))))
        result[family] = {
            "enabled": True,
            "temperature": 1.0,
            "minimum_probability": threshold,
            "minimum_runner_up_margin": 0.0,
        }
    return result


def export_onnx(model: Classifier) -> bytes:
    state = model.state_dict()
    initializers = [
        numpy_helper.from_array(state["hidden.weight"].numpy(), "hidden_weight"),
        numpy_helper.from_array(state["hidden.bias"].numpy(), "hidden_bias"),
        numpy_helper.from_array(state["output.weight"].numpy(), "output_weight"),
        numpy_helper.from_array(state["output.bias"].numpy(), "output_bias"),
    ]
    nodes = [
        helper.make_node("MatMul", ["features", "hidden_weight"], ["hidden_linear"]),
        helper.make_node("Add", ["hidden_linear", "hidden_bias"], ["hidden_biased"]),
        helper.make_node("Relu", ["hidden_biased"], ["hidden"]),
        helper.make_node("MatMul", ["hidden", "output_weight"], ["output_linear"]),
        helper.make_node("Add", ["output_linear", "output_bias"], ["logits"]),
    ]
    graph = helper.make_graph(
        nodes, "scorepeek_fixed_slot_hog_mlp_v1",
        [helper.make_tensor_value_info("features", TensorProto.FLOAT, [None, FEATURE_DIMENSIONS])],
        [helper.make_tensor_value_info("logits", TensorProto.FLOAT, [None, len(CLASSES)])],
        initializers,
    )
    artifact = helper.make_model(graph, producer_name="scorepeek", opset_imports=[helper.make_opsetid("", 17)])
    onnx.checker.check_model(artifact, full_check=True)
    return artifact.SerializeToString()


def build(dataset: Path, layout_path: Path, store: Path, output: Path) -> None:
    if output.exists() or not output.is_absolute():
        raise FixedSlotError("output must be an absent absolute directory")
    manifest, layout, suite, source_rows, cross_session, marker_rows = load_rows(dataset, layout_path, store)
    wrong_marker_rows = [row for row in marker_rows if not row["correct"]]
    if wrong_marker_rows:
        raise FixedSlotError(
            f"not-displayed marker predicate misclassified {len(wrong_marker_rows)} source rows"
        )
    stable_cells = [cell for row in source_rows if row.stable for cell in row.cells]
    sessions = sorted({row.session for row in source_rows})
    evaluated = []
    folds = []
    for held_out in sessions:
        started = time.perf_counter()
        model, training = train([cell for cell in stable_cells if cell.session != held_out and cell.digest not in cross_session])
        rows = [row for row in source_rows if row.session == held_out]
        observed = infer(model, rows)
        evaluated.extend(observed)
        folds.append({"held_out_session": held_out, "training": training, "rows": len(rows), "wall_ms": (time.perf_counter() - started) * 1000.0})
    calibrations = calibrate(evaluated)
    final_model, final_training = train([cell for cell in stable_cells if cell.digest not in cross_session])
    model_bytes = export_onnx(final_model)
    evaluation = {
        "schema": REPORT_SCHEMA,
        "suite_sha256": suite,
        "dataset_sha256": manifest["manifest_sha256"],
        "layout_sha256": sha256_bytes(layout_path.read_bytes()),
        "canonical_layout_sha256": layout["canonical_layout_sha256"],
        "configuration": {"classes": CLASSES, "feature_dimensions": FEATURE_DIMENSIONS, "hidden_dimensions": HIDDEN_DIMENSIONS, "epochs": EPOCHS},
        "folds": folds,
        "stable_fields": {
            "total": sum(row["stable"] for row in evaluated),
            "correct": sum(row["stable"] and row["correct"] for row in evaluated),
            "wrong": sum(row["stable"] and not row["correct"] for row in evaluated),
        },
        "wrong_stable_fields": [
            {key: row[key] for key in ("session", "episode", "sequence", "field", "truth", "candidates")}
            for row in evaluated if row["stable"] and not row["correct"]
        ],
        "calibrations": calibrations,
        "not_displayed_marker": {
            "total": len(marker_rows),
            "correct": len(marker_rows) - len(wrong_marker_rows),
            "wrong": len(wrong_marker_rows),
            "truth_counts": dict(sorted(Counter(row["truth"] for row in marker_rows).items())),
        },
        "final_training": final_training,
    }
    evaluation_bytes = canonical_bytes(evaluation)
    contract = {
        "schema": MODEL_SCHEMA,
        "model_id": f"scorepeek-numeric-fixed-slot-{sha256_bytes(model_bytes)[:12]}-v1",
        "model_filename": "inference.onnx",
        "model_sha256": sha256_bytes(model_bytes),
        "model_bytes": len(model_bytes),
        "candidate": "shared_hog_mlp",
        "classes": CLASSES,
        "preprocessor_id": PREPROCESSOR_ID,
        "feature_dimensions": FEATURE_DIMENSIONS,
        "hidden_dimensions": HIDDEN_DIMENSIONS,
        "output_classes": len(CLASSES),
        "numeric_character_layout_sha256": sha256_bytes(layout_path.read_bytes()),
        "canonical_layout_sha256": layout["canonical_layout_sha256"],
        "dataset_sha256": manifest["manifest_sha256"],
        "evaluation_manifest_sha256": sha256_bytes(evaluation_bytes),
        "final_training_sha256": sha256_bytes(canonical_bytes(final_training)),
        "license_id": "LicenseRef-Scorepeek-Private-Trained-Weights",
        "calibrations": {**calibrations, "joint_minimum_runner_up_margin": 0.0},
    }
    contract_bytes = canonical_bytes(contract)
    with tempfile.TemporaryDirectory(prefix="scorepeek-fixed-slot-build-", dir=output.parent) as temporary:
        staging = Path(temporary) / output.name
        staging.mkdir(mode=0o700)
        (staging / "inference.onnx").write_bytes(model_bytes)
        (staging / "manifest.json").write_bytes(contract_bytes)
        (staging / "evaluation.json").write_bytes(evaluation_bytes)
        for path in staging.iterdir():
            path.chmod(0o600)
        os.rename(staging, output)
    print(json.dumps({"bundle": str(output), "manifest_sha256": sha256_bytes(contract_bytes), "model_sha256": contract["model_sha256"], "evaluation_manifest_sha256": contract["evaluation_manifest_sha256"]}, sort_keys=True))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("dataset", type=Path)
    parser.add_argument("layout", type=Path)
    parser.add_argument("store", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        paddle.set_device("cpu")
        build(args.dataset.resolve(), args.layout.resolve(), args.store.resolve(), args.output)
    except (FixedSlotError, KeyError, OSError, ValueError, json.JSONDecodeError) as error:
        parser.exit(2, f"scorepeek fixed-slot training failed: {error}\n")


if __name__ == "__main__":
    main()
