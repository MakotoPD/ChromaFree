import sys
from pathlib import Path

import mediapipe as mp
import numpy as np
import onnxruntime as ort
from mediapipe.tasks.python import BaseOptions, vision

TOLERANCE = 1e-4
MODELS = {"selfie_segmenter": (256, 256), "selfie_segmenter_landscape": (144, 256)}


def synthetic_person(height, width, rng):
    yy, xx = np.mgrid[0:height, 0:width]
    image = np.zeros((height, width, 3), np.uint8)
    image[:] = (40, 90, 160)
    body = ((xx - width / 2) / (width * 0.28)) ** 2 + ((yy - height) / (height * 0.55)) ** 2 < 1
    head = ((xx - width / 2) / (width * 0.12)) ** 2 + ((yy - height * 0.38) / (height * 0.2)) ** 2 < 1
    image[body] = (60, 60, 70)
    image[head] = (200, 160, 130)
    noise = rng.integers(-25, 25, image.shape)
    return np.clip(image.astype(int) + noise, 0, 255).astype(np.uint8)


def reference_mask(model_path, image):
    options = vision.ImageSegmenterOptions(
        base_options=BaseOptions(model_asset_path=str(model_path)),
        output_confidence_masks=True,
        output_category_mask=False,
    )
    with vision.ImageSegmenter.create_from_options(options) as segmenter:
        result = segmenter.segment(mp.Image(image_format=mp.ImageFormat.SRGB, data=image))
        return result.confidence_masks[0].numpy_view().reshape(image.shape[:2]).copy()


def onnx_mask(model_path, image):
    session = ort.InferenceSession(str(model_path), providers=["CPUExecutionProvider"])
    tensor = (image / 255.0).astype(np.float32)[None]
    return session.run(None, {session.get_inputs()[0].name: tensor})[0][0, :, :, 0]


def main():
    models_dir = Path(__file__).parent
    rng = np.random.default_rng(1)
    failed = False
    for name, (height, width) in MODELS.items():
        image = synthetic_person(height, width, rng)
        expected = reference_mask(models_dir / f"{name}.tflite", image)
        actual = onnx_mask(models_dir / f"{name}.onnx", image)
        max_diff = float(np.abs(actual - expected).max())
        status = "ok" if max_diff < TOLERANCE else "FAIL"
        failed |= status == "FAIL"
        print(f"{status:4} {name}: max|diff| = {max_diff:.2e}, mask mean = {float(expected.mean()):.3f}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
