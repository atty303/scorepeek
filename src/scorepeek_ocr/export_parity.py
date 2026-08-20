"""Create a Paddle tensor reference for a scorepeek-owned ONNX export."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
import tempfile
from pathlib import Path
from typing import Any

import numpy as np
import paddle.inference as paddle_infer
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
    _preprocess,
    _publish,
    _read_regular,
)

REFERENCE_SCHEMA = "scorepeek-private-title-model-export-parity-reference-v1"


class ExportParityError(Exception):
    """The selected export could not produce a valid Paddle parity reference."""


def _export_record(path: Path, expected_sha256: str, preparation_sha256: str) -> dict[str, Any]:
    data = _read_regular(path / "manifest.json", MAX_MANIFEST_BYTES, expected_sha256)
    try:
        record = json.loads(data)
    except json.JSONDecodeError as error:
        raise ExportParityError("export manifest is invalid JSON") from error
    files = record.get("files") if isinstance(record, dict) else None
    required_files = {
        "paddle_graph": "inference.json",
        "paddle_parameters": "inference.pdiparams",
        "inference_config": "inference.yml",
        "onnx_model": "inference.onnx",
    }
    if (
        record.get("schema") != "scorepeek-private-title-model-converted-export-v1"
        or record.get("training_preparation_sha256") != preparation_sha256
        or record.get("onnx_checker") is not True
        or record.get("provisional") is not True
        or record.get("accepted_for_runtime") is not False
        or not isinstance(files, dict)
        or set(files) != set(required_files)
    ):
        raise ExportParityError("export manifest values are invalid")
    for name, filename in required_files.items():
        file_record = files[name]
        if not isinstance(file_record, dict) or set(file_record) != {"sha256", "bytes"}:
            raise ExportParityError("export file record is invalid")
        data = _read_regular(path / filename, MAX_MODEL_FILE_BYTES, file_record["sha256"])
        if len(data) != file_record["bytes"]:
            raise ExportParityError("export file size mismatched")
    return record


def _tensor_file(path: Path, tensor: np.ndarray) -> dict[str, Any]:
    if tensor.dtype != np.float32 or not np.isfinite(tensor).all():
        raise ExportParityError("parity tensor is not finite float32")
    path.write_bytes(tensor.astype("<f4", copy=False).tobytes(order="C"))
    return {
        "filename": path.name,
        **_hash_unpinned_file(path, path.name),
        "shape": list(tensor.shape),
    }


def generate(
    preparation: Path,
    preparation_sha256: str,
    model_export: Path,
    export_manifest_sha256: str,
    output: Path,
) -> dict[str, Any]:
    prepared_data = _read_regular(
        preparation / "manifest.json", MAX_MANIFEST_BYTES, preparation_sha256
    )
    prepared = _prepared_manifest(json.loads(prepared_data))
    _verify_prepared_files(preparation, prepared)
    export_record = _export_record(model_export, export_manifest_sha256, preparation_sha256)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise ExportParityError("output must be a new absolute directory")

    rows = prepared_rows(preparation, prepared, "validation")
    if not rows:
        raise ExportParityError("validation split is empty")
    crop_path, _, crop_sha256 = rows[0]
    crop_data = _read_regular(Path(crop_path), MAX_CROP_BYTES, crop_sha256)
    input_tensor = np.stack(
        [_preprocess(crop_path, prepared["model_input_width"], crop_sha256)]
    )

    inference_config = yaml.safe_load((model_export / "inference.yml").read_bytes())
    characters = [
        "blank",
        *inference_config["PostProcess"]["character_dict"],
        " ",
    ]
    prepared_tokens = (
        _read_regular(
            preparation / "dictionary.txt",
            MAX_MANIFEST_BYTES * 2,
            prepared["dictionary_sha256"],
        )
        .decode()
        .splitlines()
    )
    config = paddle_infer.Config(
        str(model_export / "inference.json"),
        str(model_export / "inference.pdiparams"),
    )
    config.disable_gpu()
    predictor = paddle_infer.create_predictor(config)
    if predictor.get_input_names() != ["x"] or len(predictor.get_output_names()) != 1:
        raise ExportParityError("Paddle graph input or output contract is invalid")
    handle = predictor.get_input_handle("x")
    handle.reshape(input_tensor.shape)
    handle.copy_from_cpu(input_tensor)
    predictor.run()
    paddle_output = predictor.get_output_handle(
        predictor.get_output_names()[0]
    ).copy_to_cpu()
    if (
        list(input_tensor.shape) != [1, 3, 48, prepared["model_input_width"]]
        or list(paddle_output.shape)
        != [1, prepared["output_timesteps"], prepared["output_classes"]]
        or len(characters) != prepared["output_classes"]
        or characters != ["blank", *prepared_tokens, " "]
        or characters[0] != "blank"
        or characters[-1] != " "
        or not np.all(np.abs(paddle_output.sum(axis=2) - 1.0) <= 2e-5)
        or np.any(paddle_output <= 0)
    ):
        raise ExportParityError("Paddle tensor or dictionary contract mismatched")
    raw = paddle_output[0].argmax(axis=1).tolist()
    collapsed: list[int] = []
    previous = -1
    for token in raw:
        if token != 0 and token != previous:
            collapsed.append(token)
        previous = token

    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        input_record = _tensor_file(staging / "input.f32le", input_tensor)
        output_record = _tensor_file(staging / "paddle-output.f32le", paddle_output)
        record = {
            "schema": REFERENCE_SCHEMA,
            "training_preparation_sha256": preparation_sha256,
            "validation_list_sha256": prepared["label_file_sha256"]["validation"],
            "dictionary_sha256": prepared["dictionary_sha256"],
            "validation_row_index": 0,
            "crop_file_sha256": hashlib.sha256(crop_data).hexdigest(),
            "export_manifest_sha256": export_manifest_sha256,
            "onnx_model_sha256": export_record["files"]["onnx_model"]["sha256"],
            "inference_config_sha256": export_record["files"]["inference_config"]["sha256"],
            "input": input_record,
            "paddle_output": output_record,
            "ctc_blank_token": 0,
            "argmax_token_order": raw,
            "collapsed_token_order": collapsed,
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
    parser.add_argument("--model-export", type=Path, required=True)
    parser.add_argument("--export-manifest-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = generate(
            arguments.preparation,
            arguments.preparation_sha256,
            arguments.model_export,
            arguments.export_manifest_sha256,
            arguments.output,
        )
    except Exception as error:
        print(f"scorepeek export parity reference failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    print(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    main()
