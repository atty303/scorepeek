"""Run a bounded fixed-step title-model fine-tuning pilot."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path
from typing import Any

import paddle
import yaml

from scorepeek_ocr.training_artifacts import (
    MAX_CROP_BYTES,
    MAX_MODEL_FILE_BYTES,
    _hash_unpinned_file,
    _prepared_manifest,
    _verify_prepared_files,
    prepared_rows,
)
from scorepeek_ocr.training_initializer import (
    MAX_MANIFEST_BYTES,
    _evaluate,
    _publish,
    _read_regular,
)
from scorepeek_ocr.training_process import run_checked
from scorepeek_ocr.training_source import load_registered_source, verify_source

PILOT_SCHEMA = "scorepeek-private-title-model-training-pilot-v1"
CANDIDATE_STEPS = (1, 2, 4)
BATCH_SIZE = 4
LEARNING_RATE = 1e-5
TRAINING_TIMEOUT_SECONDS = 30 * 60


class TrainingPilotError(Exception):
    """The bounded title-model fine-tuning pilot could not be completed."""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _initializer(path: Path, expected_sha256: str, preparation_sha256: str) -> dict[str, Any]:
    data = _read_regular(path / "manifest.json", MAX_MANIFEST_BYTES, expected_sha256)
    try:
        record = json.loads(data)
    except json.JSONDecodeError as error:
        raise TrainingPilotError("initializer manifest is invalid JSON") from error
    required = {
        "schema", "training_preparation_sha256", "source_checkpoint",
        "initialized_checkpoint", "tensor_count", "shape_matched_tensor_count",
        "class_mapped_tensor_count", "reused_character_count", "new_character_count",
        "probe", "provisional", "accepted_holdout_truth", "permission_status",
    }
    if (
        not isinstance(record, dict)
        or set(record) != required
        or record["schema"] != "scorepeek-private-title-model-initializer-v1"
        or record["training_preparation_sha256"] != preparation_sha256
        or not record["provisional"]
        or record["accepted_holdout_truth"]
        or record["permission_status"] != "permission_not_recorded"
        or not isinstance(record["initialized_checkpoint"], dict)
        or set(record["initialized_checkpoint"]) != {"sha256", "bytes"}
    ):
        raise TrainingPilotError("initializer manifest values are invalid")
    checkpoint = _read_regular(
        path / "initializer.pdparams",
        MAX_MODEL_FILE_BYTES,
        record["initialized_checkpoint"]["sha256"],
    )
    if len(checkpoint) != record["initialized_checkpoint"]["bytes"]:
        raise TrainingPilotError("initializer checkpoint size mismatched")
    return record


def _select_rows(
    rows: list[tuple[str, str, str]], step_count: int
) -> list[tuple[str, str, str]]:
    count = BATCH_SIZE * step_count
    if len(rows) < count:
        raise TrainingPilotError("training split is too small for a pilot candidate")
    return sorted(
        rows, key=lambda row: hashlib.sha256("\t".join(row).encode()).digest()
    )[:count]


def _first_improvement(baseline: int, candidates: list[dict[str, Any]]) -> dict[str, Any] | None:
    return next(
        (candidate for candidate in candidates if candidate["exact_count"] > baseline),
        None,
    )


def _config(
    base: dict[str, Any],
    initializer: Path,
    train_list: Path,
    output: Path,
    step_count: int,
    width: int,
) -> dict[str, Any]:
    config = yaml.safe_load(yaml.safe_dump(base))
    config["Global"].update(
        {
            "use_gpu": False,
            "distributed": False,
            "epoch_num": 1,
            "print_batch_step": 1,
            "save_model_dir": str(output),
            "save_epoch_step": 1,
            "eval_batch_step": [step_count, 1000],
            "cal_metric_during_train": False,
            "pretrained_model": str(initializer),
            "checkpoints": None,
            "use_visualdl": False,
            "seed": 0,
        }
    )
    config["Optimizer"]["lr"] = {"name": "Const", "learning_rate": LEARNING_RATE}
    config["Train"]["dataset"]["label_file_list"] = [str(train_list)]
    config["Train"]["sampler"].update(
        {"scales": [[width, 48]], "first_bs": BATCH_SIZE, "fix_bs": True}
    )
    config["Train"]["loader"].update(
        {"shuffle": False, "batch_size_per_card": BATCH_SIZE, "num_workers": 0}
    )
    config["Eval"]["loader"].update({"batch_size_per_card": 32, "num_workers": 0})
    return config


def _model(config: dict[str, Any], tokens: list[str]):
    from ppocr.modeling.architectures import build_model

    ctc_tokens = ["blank", *tokens, " "]
    config["Architecture"]["Head"]["out_channels_list"] = {
        "CTCLabelDecode": len(ctc_tokens),
        "NRTRLabelDecode": len(ctc_tokens) + 3,
    }
    return build_model(config["Architecture"]), ctc_tokens


def run(
    preparation: Path,
    preparation_sha256: str,
    source_root: Path,
    initializer: Path,
    initializer_manifest_sha256: str,
    output: Path,
) -> dict[str, Any]:
    prepared_data = _read_regular(
        preparation / "manifest.json", MAX_MANIFEST_BYTES, preparation_sha256
    )
    prepared = _prepared_manifest(json.loads(prepared_data))
    _verify_prepared_files(preparation, prepared)
    initializer_record = _initializer(
        initializer, initializer_manifest_sha256, preparation_sha256
    )
    source = load_registered_source()
    verify_source(source_root, source)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise TrainingPilotError("output must be a new absolute directory")

    config_data = _read_regular(
        preparation / "training-config.yml",
        MAX_MANIFEST_BYTES,
        prepared["derived_training_config_sha256"],
    )
    dictionary_data = _read_regular(
        preparation / "dictionary.txt",
        MAX_MANIFEST_BYTES * 2,
        prepared["dictionary_sha256"],
    )
    train_rows = prepared_rows(preparation, prepared, "train")
    validation_rows = prepared_rows(preparation, prepared, "validation")
    base_config = yaml.safe_load(config_data)
    tokens = dictionary_data.decode().splitlines()

    sys.path.insert(0, str(source_root))
    model, ctc_tokens = _model(base_config, tokens)
    model.set_state_dict(paddle.load(str(initializer / "initializer.pdparams")))
    baseline = _evaluate(
        model, validation_rows, ctc_tokens, prepared["model_input_width"]
    )

    candidates: list[dict[str, Any]] = []
    selected_checkpoint: Path | None = None
    with tempfile.TemporaryDirectory(prefix="scorepeek-title-model-pilot-") as temporary:
        temporary_root = Path(temporary)
        for step_count in CANDIDATE_STEPS:
            candidate_root = temporary_root / f"steps-{step_count}"
            candidate_root.mkdir()
            selected_rows = _select_rows(train_rows, step_count)
            snapshot = candidate_root / "verified-crops"
            snapshot.mkdir(mode=0o700)
            training_rows = []
            for index, (path, title, digest) in enumerate(selected_rows):
                data = _read_regular(Path(path), MAX_CROP_BYTES, digest)
                copied = snapshot / f"{index:04}.ppm"
                copied.write_bytes(data)
                os.chmod(copied, 0o600)
                training_rows.append(f"{copied}\t{title}\n")
            train_list = candidate_root / "train.txt"
            train_list.write_text("".join(training_rows), encoding="utf-8")
            selection = "".join(
                f"{path}\t{title}\t{digest}\n" for path, title, digest in selected_rows
            ).encode()
            config = _config(
                base_config,
                initializer / "initializer.pdparams",
                train_list,
                candidate_root / "output",
                step_count,
                prepared["model_input_width"],
            )
            config_path = candidate_root / "training-config.yml"
            config_path.write_text(
                yaml.safe_dump(config, allow_unicode=True, sort_keys=False),
                encoding="utf-8",
            )
            environment = os.environ.copy()
            environment["NO_ALBUMENTATIONS_UPDATE"] = "1"
            run_checked(
                [
                    sys.executable,
                    str(source_root / source.training_entrypoint.path),
                    "-c",
                    str(config_path),
                ],
                cwd=source_root,
                environment=environment,
                timeout_seconds=TRAINING_TIMEOUT_SECONDS,
            )
            checkpoint = candidate_root / "output/latest.pdparams"
            candidate_model, candidate_tokens = _model(base_config, tokens)
            candidate_model.set_state_dict(paddle.load(str(checkpoint)))
            probe = _evaluate(
                candidate_model,
                validation_rows,
                candidate_tokens,
                prepared["model_input_width"],
            )
            candidate = {
                "steps": step_count,
                "training_sample_count": len(selected_rows),
                "training_list_sha256": _sha256(selection),
                **probe,
            }
            candidates.append(candidate)
            if _first_improvement(baseline["exact_count"], candidates) is candidate:
                selected_checkpoint = checkpoint
                break
        if selected_checkpoint is None:
            raise TrainingPilotError("no bounded candidate improved strict validation recognition")

        staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
        try:
            model_path = staging / "model.pdparams"
            shutil.copyfile(selected_checkpoint, model_path)
            model_file = _hash_unpinned_file(model_path, "selected pilot checkpoint")
            selected = candidates[-1]
            record = {
                "schema": PILOT_SCHEMA,
                "training_preparation_sha256": preparation_sha256,
                "training_source_commit": source.commit,
                "initializer_manifest_sha256": initializer_manifest_sha256,
                "initializer_checkpoint": initializer_record["initialized_checkpoint"],
                "recipe": {
                    "candidate_steps": list(CANDIDATE_STEPS),
                    "batch_size": BATCH_SIZE,
                    "learning_rate": LEARNING_RATE,
                    "learning_rate_schedule": "constant",
                    "train_image_shape": [3, 48, prepared["model_input_width"]],
                    "seed": 0,
                    "device": "cpu",
                },
                "baseline_probe": baseline,
                "candidates": candidates,
                "selected_steps": selected["steps"],
                "selected_checkpoint": model_file,
                "provisional": True,
                "accepted_holdout_truth": False,
                "permission_status": prepared["permission_status"],
            }
            (staging / "manifest.json").write_text(
                json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n",
                encoding="utf-8",
            )
            _publish(staging, output)
            return record
        except BaseException:
            if staging.exists():
                shutil.rmtree(staging)
            raise


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--preparation", type=Path, required=True)
    parser.add_argument("--preparation-sha256", required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--initializer", type=Path, required=True)
    parser.add_argument("--initializer-manifest-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = run(
            arguments.preparation,
            arguments.preparation_sha256,
            arguments.source,
            arguments.initializer,
            arguments.initializer_manifest_sha256,
            arguments.output,
        )
    except Exception as error:
        print(f"scorepeek training pilot failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    print(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    main()
