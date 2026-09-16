import sys
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort
import onnxsim
from onnx import numpy_helper

TOLERANCE = {"float16": 5e-3, "float32": 1e-4}
STEPS = 3
ENCODER_WIDTH = 320


def element_type(session):
    return np.float16 if session.get_inputs()[0].type == "tensor(float16)" else np.float32


def dynamic_feeds(src, states, ratio):
    return {"src": src, **{f"r{i + 1}i": s for i, s in enumerate(states)}, "downsample_ratio": np.array([ratio], np.float32)}


def recurrent_state_shapes(session, width, height, ratio):
    dtype = element_type(session)
    zero_states = [np.zeros((1, 1, 1, 1), dtype)] * 4
    outputs = dict(zip([o.name for o in session.get_outputs()], session.run(None, dynamic_feeds(np.zeros((1, 3, height, width), dtype), zero_states, ratio))))
    return [list(outputs[f"r{i}o"].shape) for i in range(1, 5)]


def freeze(model, width, height, ratio, state_shapes):
    graph = model.graph
    ratio_input = next(i for i in graph.input if i.name == "downsample_ratio")
    graph.input.remove(ratio_input)
    graph.initializer.append(numpy_helper.from_array(np.array([ratio], np.float32), "downsample_ratio"))
    fixed_shapes = {"src": [1, 3, height, width], **{f"r{i + 1}i": shape for i, shape in enumerate(state_shapes)}}
    for graph_input in graph.input:
        for dim, value in zip(graph_input.type.tensor_type.shape.dim, fixed_shapes[graph_input.name]):
            dim.ClearField("dim_param")
            dim.dim_value = value
    for graph_output in graph.output:
        for dim in graph_output.type.tensor_type.shape.dim:
            dim.ClearField("dim_param")
    simplified, ok = onnxsim.simplify(model)
    if not ok:
        raise SystemExit("onnxsim could not validate the simplified model")
    return simplified


def verify(dynamic, static_path, width, height, ratio, state_shapes):
    frozen = ort.InferenceSession(str(static_path), providers=["CPUExecutionProvider"])
    dtype = element_type(dynamic)
    rng = np.random.default_rng(0)
    dynamic_states = [np.zeros((1, 1, 1, 1), dtype)] * 4
    static_states = [np.zeros(shape, dtype) for shape in state_shapes]
    worst = 0.0
    for _ in range(STEPS):
        src = rng.random((1, 3, height, width)).astype(dtype)
        a = dict(zip([o.name for o in dynamic.get_outputs()], dynamic.run(None, dynamic_feeds(src, dynamic_states, ratio))))
        b = dict(zip([o.name for o in frozen.get_outputs()], frozen.run(None, {"src": src, **{f"r{i + 1}i": s for i, s in enumerate(static_states)}})))
        worst = max(worst, float(np.abs(a["pha"].astype(np.float32) - b["pha"].astype(np.float32)).max()))
        dynamic_states = [a[f"r{i}o"] for i in range(1, 5)]
        static_states = [b[f"r{i}o"] for i in range(1, 5)]
    return worst


def freeze_variant(models_dir, precision, width, height):
    ratio = ENCODER_WIDTH / width
    source = models_dir / f"rvm_mobilenetv3_{precision}.onnx"
    target = models_dir / f"rvm_mobilenetv3_{precision}_{width}x{height}_static.onnx"
    dynamic = ort.InferenceSession(str(source), providers=["CPUExecutionProvider"])
    state_shapes = recurrent_state_shapes(dynamic, width, height, ratio)
    onnx.save(freeze(onnx.load(str(source)), width, height, ratio, state_shapes), str(target))
    worst = verify(dynamic, target, width, height, ratio, state_shapes)
    ok = worst < TOLERANCE["float16" if precision == "fp16" else "float32"]
    print(f"{'ok' if ok else 'FAIL':4} {target.name}: ratio={ratio:.4f} states={state_shapes} max|pha diff|={worst:.2e}")
    return ok


def main():
    if len(sys.argv) < 3:
        raise SystemExit("usage: make_rvm_static.py <fp16|fp32> <width>x<height> [<width>x<height> ...]")
    precision = sys.argv[1]
    models_dir = Path(__file__).parent
    results = []
    for resolution in sys.argv[2:]:
        width, height = (int(v) for v in resolution.lower().split("x"))
        results.append(freeze_variant(models_dir, precision, width, height))
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
