mod gpu;

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use bgcam_core::{
    Background, BgraFrame, ColorMatrix, FrameSize, MaskParams, MaskRefiner, ModelVariant, Nv12Frame, Orientation, Rgb,
    RgbImage, RvmModel, SegmentationModel, blend_nv12, nv12_to_bgra,
};
use ort::AsPointer;
use ort::ep::directml::{DMLResource, DMLSessionBuilderExt, resource_from_d3d};
use ort::memory::{AllocationDevice, AllocatorType, MemoryInfo, MemoryType};
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::sys::ONNXTensorElementDataType;
use ort::value::DynValue;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_TYPE_READBACK, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_RESOURCE_STATE_GENERIC_READ, ID3D12CommandQueue, ID3D12Resource,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::core::Interface;

use gpu::{Gpu, Mapped};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const MODEL: ModelVariant = ModelVariant::new(640, 360);
const MATRIX: ColorMatrix = ColorMatrix::Bt709;
const STATE_INPUTS: [&str; 4] = ["r1i", "r2i", "r3i", "r4i"];
const STATE_OUTPUTS: [&str; 4] = ["r1o", "r2o", "r3o", "r4o"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn process_cpu_time() -> Duration {
    let (mut creation, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) }.is_err() {
        return Duration::ZERO;
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

fn gpu_tensor(resource: &DMLResource, bytes: usize, shape: &[i64]) -> Result<DynValue> {
    let info = MemoryInfo::new(
        AllocationDevice::DIRECTML,
        0,
        AllocatorType::Device,
        MemoryType::Default,
    )?;
    let mut value = std::ptr::null_mut();
    let status = unsafe {
        (ort::api().CreateTensorWithDataAsOrtValue)(
            info.ptr(),
            **resource,
            bytes,
            shape.as_ptr(),
            shape.len(),
            ONNXTensorElementDataType::ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT16,
            &mut value,
        )
    };
    if !status.0.is_null() {
        let message = unsafe { std::ffi::CStr::from_ptr((ort::api().GetErrorMessage)(status.0)) }
            .to_string_lossy()
            .into_owned();
        unsafe { (ort::api().ReleaseStatus)(status.0) };
        bail!("CreateTensorWithDataAsOrtValue failed: {message}");
    }
    let value = NonNull::new(value).context("null tensor")?;
    Ok(unsafe { DynValue::from_ptr(value, None) })
}

struct GpuTensor {
    _resource: ID3D12Resource,
    dml: DMLResource,
    shape: Vec<i64>,
    bytes: usize,
}

impl GpuTensor {
    fn new(gpu: &Gpu, shape: &[i64]) -> Result<(Self, ID3D12Resource)> {
        let bytes = shape.iter().product::<i64>() as usize * 2;
        let aligned = bytes.div_ceil(4) * 4;
        let resource = gpu.uav_buffer(aligned as u64)?;
        let dml = unsafe { resource_from_d3d(resource.as_raw() as *mut ()) }?;
        Ok((
            Self {
                _resource: resource.clone(),
                dml,
                shape: shape.to_vec(),
                bytes,
            },
            resource,
        ))
    }

    fn value(&self) -> Result<DynValue> {
        gpu_tensor(&self.dml, self.bytes, &self.shape)
    }
}

fn ort_error(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{error}")
}

fn float_bits(value: f32) -> u32 {
    value.to_bits()
}

fn frames(photo: &Nv12Frame) -> Result<[Nv12Frame; 2]> {
    let mirror = Orientation {
        mirror_horizontal: true,
        ..Default::default()
    };
    let mut mirrored = Nv12Frame::new(photo.size());
    mirror.apply(photo, &mut mirrored)?;
    Ok([photo.clone(), mirrored])
}

fn save_png(frame: &Nv12Frame, path: &Path) -> Result<()> {
    let mut bgra = BgraFrame::new(frame.size());
    nv12_to_bgra(frame, None, MATRIX, &mut bgra)?;
    let rgba: Vec<u8> = bgra
        .data()
        .chunks_exact(4)
        .flat_map(|p| [p[2], p[1], p[0], 255])
        .collect();
    image::RgbaImage::from_raw(frame.size().width(), frame.size().height(), rgba)
        .context("image buffer")?
        .save(path)?;
    Ok(())
}

fn cpu_reference(models: &Path, inputs: &[Nv12Frame; 2], count: usize) -> Result<Nv12Frame> {
    let size = inputs[0].size();
    let mut model = RvmModel::load(models, MODEL, bgcam_core::InferenceDevice::default())?;
    let mut refiner = MaskRefiner::new(MODEL.width, MODEL.height, size, MaskParams::RECURRENT_MODEL);
    let green = MATRIX.to_yuv(Rgb::GREEN_SCREEN);
    let mut output = Nv12Frame::new(size);
    for index in 0..count {
        let frame = &inputs[index % 2];
        let alpha = model.segment(frame, MATRIX)?;
        let mask = refiner.refine(alpha.data)?;
        blend_nv12(frame, Background::Solid(green), mask.luma, mask.chroma, &mut output)?;
    }
    Ok(output)
}

fn main() -> Result<()> {
    let models = repo_root().join("models");
    let photo_path = std::env::var_os("BGCAM_PERSON_IMAGE").context("set BGCAM_PERSON_IMAGE")?;
    let size = FrameSize::new(WIDTH, HEIGHT)?;
    let photo = RgbImage::load(Path::new(&photo_path))?.to_nv12(size, MATRIX)?;
    let inputs = frames(&photo)?;
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("results");
    std::fs::create_dir_all(&out_dir)?;

    let exe_dir = std::env::current_exe()?.parent().context("exe dir")?.to_path_buf();
    ort::util::preload_dylib(exe_dir.join("DirectML.dll")).map_err(|e| anyhow!("{e}"))?;

    let mut builder = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort_error)?
        .with_intra_op_spinning(false)
        .map_err(ort_error)?
        .with_inter_op_spinning(false)
        .map_err(ort_error)?
        .with_memory_pattern(false)
        .map_err(ort_error)?
        .with_parallel_execution(false)
        .map_err(ort_error)?
        .with_execution_providers([ort::ep::DirectML::default()
            .with_device_id(0)
            .build()
            .error_on_failure()])
        .map_err(ort_error)?;
    let queue_pointer = builder
        .d3d_command_queue()
        .context("DirectML unavailable")?
        .map_err(|e| anyhow!("{e}"))?;
    let raw_queue = queue_pointer as *mut c_void;
    let queue = unsafe { ID3D12CommandQueue::from_raw_borrowed(&raw_queue) }
        .context("null command queue")?
        .clone();
    let mut session = builder
        .commit_from_file(models.join(MODEL.rvm_file_name()))
        .map_err(ort_error)?;
    let mut gpu = Gpu::from_queue(queue)?;
    println!("command list type: {:?}", gpu.list_type());

    let preprocess = gpu
        .compute_pipeline(include_str!("../shaders/preprocess.hlsl"))
        .context("preprocess pipeline")?;
    let composite = gpu
        .compute_pipeline(include_str!("../shaders/composite.hlsl"))
        .context("composite pipeline")?;

    let nv12_bytes = size.luma_len() + size.chroma_len();
    let mut upload = Mapped::new(
        gpu.buffer(
            nv12_bytes as u64,
            D3D12_HEAP_TYPE_UPLOAD,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        )?,
        nv12_bytes,
    )?;
    let output = gpu.uav_buffer(nv12_bytes as u64).context("output buffer")?;
    let readback = Mapped::new(
        gpu.buffer(
            nv12_bytes as u64,
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?,
        nv12_bytes,
    )?;

    let (source_tensor, source_buffer) =
        GpuTensor::new(&gpu, &[1, 3, i64::from(MODEL.height), i64::from(MODEL.width)])?;
    let (alpha_tensor, alpha_buffer) = GpuTensor::new(&gpu, &[1, 1, i64::from(MODEL.height), i64::from(MODEL.width)])?;
    let state_shapes: Vec<Vec<i64>> = STATE_INPUTS
        .iter()
        .map(|name| {
            let input = session
                .inputs()
                .iter()
                .find(|i| i.name() == *name)
                .context("state input")?;
            Ok(input.dtype().tensor_shape().context("state shape")?.to_vec())
        })
        .collect::<Result<_>>()?;
    let states: [Vec<GpuTensor>; 2] = [
        state_shapes
            .iter()
            .map(|s| Ok(GpuTensor::new(&gpu, s)?.0))
            .collect::<Result<_>>()?,
        state_shapes
            .iter()
            .map(|s| Ok(GpuTensor::new(&gpu, s)?.0))
            .collect::<Result<_>>()?,
    ];

    let mut bindings = [session.create_binding()?, session.create_binding()?];
    for (index, binding) in bindings.iter_mut().enumerate() {
        binding.bind_input("src", &source_tensor.value()?)?;
        for (i, name) in STATE_INPUTS.iter().enumerate() {
            binding.bind_input(*name, &states[index][i].value()?)?;
        }
        for (i, name) in STATE_OUTPUTS.iter().enumerate() {
            binding.bind_output(*name, states[1 - index][i].value()?)?;
        }
        binding.bind_output("pha", alpha_tensor.value()?)?;
    }

    let k = MATRIX.inverse();
    let preprocess_constants = [
        WIDTH,
        HEIGHT,
        MODEL.width,
        MODEL.height,
        float_bits(k.red_v as f32 / 256.0),
        float_bits(k.green_u as f32 / 256.0),
        float_bits(k.green_v as f32 / 256.0),
        float_bits(k.blue_u as f32 / 256.0),
    ];
    let green = MATRIX.to_yuv(Rgb::GREEN_SCREEN);
    let composite_constants = [
        WIDTH,
        HEIGHT,
        MODEL.width,
        MODEL.height,
        float_bits(f32::from(green.y)),
        float_bits(f32::from(green.u)),
        float_bits(f32::from(green.v)),
        0,
    ];
    let preprocess_groups = ((MODEL.width / 2).div_ceil(16), MODEL.height.div_ceil(16));
    let composite_groups = ((WIDTH / 4).div_ceil(16), (HEIGHT + HEIGHT / 2).div_ceil(16));
    let mut result = Nv12Frame::new(size);

    let tensor_bytes = source_tensor.bytes;
    let tensor_readback = Mapped::new(
        gpu.buffer(
            tensor_bytes as u64,
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?,
        tensor_bytes,
    )?;
    upload.bytes_mut()[..size.luma_len()].copy_from_slice(inputs[0].luma());
    upload.bytes_mut()[size.luma_len()..].copy_from_slice(inputs[0].chroma());
    gpu.begin(true)?;
    gpu.dispatch(
        &preprocess,
        &preprocess_constants,
        upload.resource(),
        &source_buffer,
        &alpha_buffer,
        preprocess_groups,
    );
    gpu.copy_to_readback(&source_buffer, tensor_readback.resource());
    gpu.submit()?;
    gpu.wait()?;
    let gpu_tensor_values: Vec<f32> = tensor_readback
        .bytes()
        .chunks_exact(2)
        .map(|b| half::f16::from_le_bytes([b[0], b[1]]).to_f32())
        .collect();
    let mut builder = bgcam_core::ModelInputBuilder::<f32>::new(
        size,
        FrameSize::new(MODEL.width, MODEL.height)?,
        bgcam_core::TensorLayout::Nchw,
        MATRIX,
    );
    let cpu_tensor_values = builder.build(&inputs[0])?;
    let tensor_diffs: Vec<f32> = gpu_tensor_values
        .iter()
        .zip(cpu_tensor_values)
        .map(|(a, b)| (a - b).abs())
        .collect();
    println!(
        "input tensor gpu vs cpu: mean |diff| {:.4}, max {:.4} (in 0..1 units, 1/255 = 0.0039)",
        tensor_diffs.iter().sum::<f32>() / tensor_diffs.len() as f32,
        tensor_diffs.iter().copied().fold(0.0, f32::max)
    );

    let mut process_frame =
        |gpu: &mut Gpu, session: &mut Session, index: usize, result: &mut Nv12Frame| -> Result<()> {
            let frame = &inputs[index % 2];
            let bytes = upload.bytes_mut();
            bytes[..size.luma_len()].copy_from_slice(frame.luma());
            bytes[size.luma_len()..].copy_from_slice(frame.chroma());

            gpu.begin(true)?;
            gpu.dispatch(
                &preprocess,
                &preprocess_constants,
                upload.resource(),
                &source_buffer,
                &alpha_buffer,
                preprocess_groups,
            );
            gpu.submit()?;

            drop(session.run_binding(&bindings[index % 2])?);

            gpu.begin(false)?;
            gpu.dispatch(
                &composite,
                &composite_constants,
                upload.resource(),
                &output,
                &alpha_buffer,
                composite_groups,
            );
            gpu.copy_to_readback(&output, readback.resource());
            gpu.submit()?;
            gpu.wait()?;

            let (luma, chroma) = result.planes_mut();
            luma.copy_from_slice(&readback.bytes()[..size.luma_len()]);
            chroma.copy_from_slice(&readback.bytes()[size.luma_len()..]);
            Ok(())
        };

    let warmup = 30;
    for index in 0..warmup {
        process_frame(&mut gpu, &mut session, index, &mut result)?;
    }
    save_png(&result, &out_dir.join("gpu_output.png"))?;
    let reference = cpu_reference(&models, &inputs, warmup)?;
    save_png(&reference, &out_dir.join("cpu_reference.png"))?;
    let differences: Vec<u8> = result
        .luma()
        .iter()
        .zip(reference.luma())
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let mean = differences.iter().map(|&d| f64::from(d)).sum::<f64>() / differences.len() as f64;
    let above_16 = differences.iter().filter(|&&d| d > 16).count() as f64 / differences.len() as f64 * 100.0;
    println!(
        "gpu vs cpu luma: mean |diff| {mean:.2}, max {}, pixels with diff > 16: {above_16:.2}%",
        differences.iter().max().copied().unwrap_or(0)
    );

    let period = Duration::from_secs(1) / 30;
    let run_for = Duration::from_secs(10);
    let cpu_start = process_cpu_time();
    let start = Instant::now();
    let mut next = start;
    let mut timings = Vec::new();
    let mut index = warmup;
    while start.elapsed() < run_for {
        let frame_start = Instant::now();
        process_frame(&mut gpu, &mut session, index, &mut result)?;
        timings.push(frame_start.elapsed().as_secs_f64() * 1000.0);
        index += 1;
        next += period;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let cpu_ms = (process_cpu_time() - cpu_start).as_secs_f64() * 1000.0;
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    timings.sort_by(f64::total_cmp);
    println!(
        "gpu pipeline 1280x720 -> RVM {}x{} -> green screen, paced 30 fps: frames {}, frame median {:.2} ms, p95 {:.2} ms, CPU {:.2} ms/frame ({:.1}% of one core)",
        MODEL.width,
        MODEL.height,
        timings.len(),
        timings[timings.len() / 2],
        timings[timings.len() * 95 / 100],
        cpu_ms / timings.len() as f64,
        cpu_ms / elapsed_ms * 100.0
    );
    Ok(())
}
