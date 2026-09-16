import struct
import sys
from pathlib import Path

import numpy as np
import onnx
import tf2onnx
from onnx import helper, numpy_helper

CUSTOM_OP = "TFL_Convolution2DTransposeBias"
OPSET = 17


def producer_of(graph, tensor_name):
    return next((n for n in graph.node if tensor_name in n.output), None)


def consumers_of(graph, tensor_name):
    return [n for n in graph.node if tensor_name in n.input]


def custom_op_params(tflite_path):
    from tensorflow.lite.python import schema_py_generated as schema

    buffer = Path(tflite_path).read_bytes()
    model = schema.ModelT.InitFromObj(schema.Model.GetRootAsModel(buffer, 0))
    for op in model.subgraphs[0].operators:
        code = model.operatorCodes[op.opcodeIndex].customCode
        if code == b"Convolution2DTransposeBias":
            padding, stride_w, stride_h = struct.unpack("<3i", bytes(op.customOptions))
            return padding, stride_w, stride_h
    raise SystemExit("custom op not found in tflite model")


def replace_transpose_bias(model, tflite_path):
    padding, stride_w, stride_h = custom_op_params(tflite_path)
    if padding != 1 or stride_w != stride_h:
        raise SystemExit(f"unsupported custom op params: {padding} {stride_w} {stride_h}")

    graph = model.graph
    initializers = {t.name: t for t in graph.initializer}
    for node in [n for n in graph.node if n.op_type == CUSTOM_OP]:
        data_nhwc, kernel_name, bias_name = node.input
        to_nhwc = producer_of(graph, data_nhwc)
        if to_nhwc is None or to_nhwc.op_type != "Transpose" or len(consumers_of(graph, data_nhwc)) != 1:
            raise SystemExit("unexpected graph around custom op")

        kernel_ohwi = numpy_helper.to_array(initializers[kernel_name])
        kernel_size = kernel_ohwi.shape[1]
        if stride_w != kernel_size:
            raise SystemExit("SAME padding mapping only verified for stride == kernel size")
        kernel_iohw = np.ascontiguousarray(kernel_ohwi.transpose(3, 0, 1, 2))
        graph.initializer.append(numpy_helper.from_array(kernel_iohw, kernel_name + "_iohw"))

        output_nhwc = node.output[0]
        output_nchw = output_nhwc + "_nchw"
        index = list(graph.node).index(to_nhwc)
        replacement = [
            helper.make_node(
                "ConvTranspose",
                [to_nhwc.input[0], kernel_name + "_iohw", bias_name],
                [output_nchw],
                kernel_shape=[kernel_size, kernel_size],
                strides=[stride_h, stride_w],
                name=node.name + "_convtranspose",
            ),
            helper.make_node("Transpose", [output_nchw], [output_nhwc], perm=[0, 2, 3, 1], name=node.name + "_to_nhwc"),
        ]
        graph.node.remove(node)
        graph.node.remove(to_nhwc)
        for offset, new_node in enumerate(replacement):
            graph.node.insert(index + offset, new_node)

    used = {i for n in graph.node for i in n.input}
    for init in [t for t in graph.initializer if t.name not in used]:
        graph.initializer.remove(init)
    return model


def convert(tflite_path, onnx_path):
    model, _ = tf2onnx.convert.from_tflite(str(tflite_path), opset=OPSET)
    model = replace_transpose_bias(model, tflite_path)
    if any(n.op_type.startswith("TFL_") for n in model.graph.node):
        raise SystemExit("unconverted tflite ops remain")
    onnx.checker.check_model(model, full_check=True)
    onnx.save(model, str(onnx_path))


def main():
    models_dir = Path(__file__).parent
    names = sys.argv[1:] or ["selfie_segmenter", "selfie_segmenter_landscape"]
    for name in names:
        convert(models_dir / f"{name}.tflite", models_dir / f"{name}.onnx")
        print(f"converted {name}")


if __name__ == "__main__":
    main()
