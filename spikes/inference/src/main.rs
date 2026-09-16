use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use half::f16;
use ort::{
    ep,
    logging::LogLevel,
    session::{
        OutputSelector, RunOptions, Session,
        builder::{GraphOptimizationLevel, SessionBuilder},
    },
    value::{DynValue, Tensor, TensorElementType},
};
use windows::{
    Win32::{
        Foundation::FILETIME,
        Graphics::Dxgi::{
            CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
            IDXGIAdapter3, IDXGIFactory1,
        },
        System::{
            ProcessStatus::{
                K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
            },
            Threading::{GetCurrentProcess, GetProcessTimes},
        },
    },
    core::Interface,
};

const FRAME_WIDTH: i64 = 1280;
const FRAME_HEIGHT: i64 = 720;
const STATIC_RESOLUTIONS: [(i64, i64); 23] = [
    (640, 360),
    (960, 540),
    (1024, 576),
    (1280, 720),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
    (640, 400),
    (960, 600),
    (1280, 800),
    (1440, 900),
    (1680, 1050),
    (1920, 1200),
    (2560, 1600),
    (640, 480),
    (800, 600),
    (960, 720),
    (1024, 768),
    (1280, 960),
    (1440, 1080),
    (1600, 1200),
    (1920, 1440),
    (2560, 1920),
];
const WARMUP_FRAMES: usize = 30;
const DEFAULT_FRAMES: usize = 500;

#[derive(Clone, Copy, Debug)]
enum Device {
    DirectMl,
    Cpu,
}

#[derive(Clone, Copy, Debug)]
enum Model {
    Rvm { fp16: bool, ratio: f32, width: i64 },
    RvmStatic { width: i64, height: i64 },
    MediaPipe { landscape: bool },
}

#[derive(Clone, Copy, Debug)]
struct Case {
    model: Model,
    device: Device,
}

impl Model {
    fn file_name(self) -> String {
        match self {
            Model::Rvm { fp16: false, .. } => "rvm_mobilenetv3_fp32.onnx".into(),
            Model::Rvm { fp16: true, .. } => "rvm_mobilenetv3_fp16.onnx".into(),
            Model::RvmStatic { width, height } => {
                format!("rvm_mobilenetv3_fp16_{width}x{height}_static.onnx")
            }
            Model::MediaPipe { landscape: false } => "selfie_segmenter.onnx".into(),
            Model::MediaPipe { landscape: true } => "selfie_segmenter_landscape.onnx".into(),
        }
    }

    fn label(self) -> String {
        match self {
            Model::Rvm { fp16, ratio, width } => format!(
                "RVM {} {}x{} r={ratio}",
                if fp16 { "fp16" } else { "fp32" },
                width,
                width * 9 / 16
            ),
            Model::RvmStatic { width, height } => format!("RVM fp16 {width}x{height} static"),
            Model::MediaPipe { landscape: false } => "MediaPipe 256x256".into(),
            Model::MediaPipe { landscape: true } => "MediaPipe 144x256".into(),
        }
    }
}

fn all_cases() -> Vec<Case> {
    let models = [
        Model::Rvm {
            fp16: false,
            ratio: 0.25,
            width: FRAME_WIDTH,
        },
        Model::Rvm {
            fp16: false,
            ratio: 0.4,
            width: FRAME_WIDTH,
        },
        Model::Rvm {
            fp16: true,
            ratio: 0.25,
            width: FRAME_WIDTH,
        },
        Model::Rvm {
            fp16: true,
            ratio: 0.4,
            width: FRAME_WIDTH,
        },
        Model::Rvm {
            fp16: false,
            ratio: 0.5,
            width: FRAME_WIDTH / 2,
        },
        Model::Rvm {
            fp16: true,
            ratio: 0.5,
            width: FRAME_WIDTH / 2,
        },
        Model::MediaPipe { landscape: false },
        Model::MediaPipe { landscape: true },
    ];
    let static_models = STATIC_RESOLUTIONS
        .iter()
        .map(|&(width, height)| Model::RvmStatic { width, height });
    let models: Vec<Model> = models.into_iter().chain(static_models).collect();
    let only_static = env::var("BENCH_STATIC_ONLY").is_ok_and(|v| v == "1");
    [Device::DirectMl, Device::Cpu]
        .into_iter()
        .flat_map(|device| models.iter().map(move |&model| Case { model, device }))
        .filter(|case| !only_static || matches!(case.model, Model::RvmStatic { .. }))
        .filter(|case| {
            !matches!(
                (case.device, case.model),
                (Device::Cpu, Model::RvmStatic { width, .. }) if width > FRAME_WIDTH
            )
        })
        .collect()
}

struct Measurement {
    all_on_gpu: String,
    session_ms: f64,
    mean_ms: f64,
    p95_ms: f64,
    cpu_ms_per_frame: f64,
    working_set_mb: f64,
    private_mb: f64,
    vram_mb: f64,
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let models_dir = repo_root().join("models");
    let frames = env::var("BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_FRAMES);
    match args.get(1).map(String::as_str) {
        Some("--case") => {
            let index: usize = args.get(2).context("missing case index")?.parse()?;
            let case = *all_cases().get(index).context("case index out of range")?;
            let m = run_case(&models_dir, case, frames)?;
            println!(
                "RESULT\t{}\t{:.0}\t{:.2}\t{:.2}\t{:.2}\t{:.0}\t{:.0}\t{:.0}",
                m.all_on_gpu,
                m.session_ms,
                m.mean_ms,
                m.p95_ms,
                m.cpu_ms_per_frame,
                m.working_set_mb,
                m.private_mb,
                m.vram_mb
            );
            Ok(())
        }
        Some("--inspect") => inspect(&models_dir),
        _ => run_all(frames),
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn run_all(frames: usize) -> Result<()> {
    let exe = env::current_exe()?;
    let mut table = String::from(
        "| Model | EP | Wszystko na GPU | Sesja [ms] | Średnio [ms] | p95 [ms] | CPU/klatkę [ms] | Working set [MB] | Private [MB] | VRAM [MB] |\n|---|---|---|---|---|---|---|---|---|---|\n",
    );
    println!(
        "adapter 0: {}",
        adapter_description().unwrap_or_else(|e| e.to_string())
    );
    println!(
        "frames per case: {frames}, spinning: {}",
        env::var("BENCH_SPIN").unwrap_or_else(|_| "default".into())
    );
    let only_device = env::var("BENCH_DEVICE").ok();
    for (index, case) in all_cases().into_iter().enumerate() {
        if only_device
            .as_deref()
            .is_some_and(|d| !format!("{:?}", case.device).eq_ignore_ascii_case(d))
        {
            continue;
        }
        let output = Command::new(&exe)
            .args(["--case", &index.to_string()])
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let device = match case.device {
            Device::DirectMl => "DirectML",
            Device::Cpu => "CPU",
        };
        let row = match stdout.lines().find_map(|l| l.strip_prefix("RESULT\t")) {
            Some(result) => format!(
                "| {} | {device} | {} |\n",
                case.model.label(),
                result.replace('\t', " | ")
            ),
            None => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let reason = stdout
                    .lines()
                    .chain(stderr.lines())
                    .rfind(|l| !l.trim().is_empty())
                    .unwrap_or("unknown");
                format!(
                    "| {} | {device} | błąd: {} |||||||\n",
                    case.model.label(),
                    reason.replace('|', "/")
                )
            }
        };
        print!("{row}");
        for line in stdout.lines().filter(|l| l.starts_with("CPU_NODES")) {
            println!("  {line}");
        }
        table.push_str(&row);
    }
    let results_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("results");
    fs::create_dir_all(&results_dir)?;
    let suffix = env::var("BENCH_SPIN")
        .map(|v| format!("-spin{v}"))
        .unwrap_or_default();
    fs::write(results_dir.join(format!("results{suffix}.md")), &table)?;
    println!("\n{table}");
    Ok(())
}

fn inspect(models_dir: &Path) -> Result<()> {
    println!("adapter 0: {}", adapter_description()?);
    for name in [
        "rvm_mobilenetv3_fp32.onnx",
        "rvm_mobilenetv3_fp16.onnx",
        "selfie_segmenter.onnx",
        "selfie_segmenter_landscape.onnx",
    ] {
        let session = Session::builder()
            .map_err(ort_err)?
            .commit_from_file(models_dir.join(name))
            .map_err(ort_err)?;
        println!("{name}");
        for input in session.inputs() {
            println!("  in  {} {:?}", input.name(), input.dtype());
        }
        for output in session.outputs() {
            println!("  out {} {:?}", output.name(), output.dtype());
        }
    }
    Ok(())
}

fn ort_err<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow!("{e}")
}

fn base_builder(device: Device, log_sink: Arc<Mutex<Vec<String>>>) -> Result<SessionBuilder> {
    let builder = Session::builder()
        .map_err(ort_err)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort_err)?
        .with_log_level(LogLevel::Verbose)
        .map_err(ort_err)?
        .with_logger(Arc::new(
            move |_level, _category, _id, _location, message: &str| {
                let is_placement = message.contains("placed on")
                    || message.starts_with(' ') && message.contains('(');
                if is_placement && let Ok(mut lines) = log_sink.lock() {
                    lines.push(message.to_owned());
                }
            },
        ))
        .map_err(ort_err)?;
    let spinning = env::var("BENCH_SPIN").map(|v| v != "0").unwrap_or(true);
    let builder = builder
        .with_intra_op_spinning(spinning)
        .map_err(ort_err)?
        .with_inter_op_spinning(spinning)
        .map_err(ort_err)?;
    Ok(match device {
        Device::DirectMl => builder
            .with_memory_pattern(false)
            .map_err(ort_err)?
            .with_parallel_execution(false)
            .map_err(ort_err)?
            .with_execution_providers([ep::DirectML::default()
                .with_device_id(0)
                .build()
                .error_on_failure()])
            .map_err(ort_err)?,
        Device::Cpu => builder,
    })
}

fn build_session(models_dir: &Path, case: Case) -> Result<(Session, String, Vec<String>)> {
    let path = models_dir.join(case.model.file_name());
    let logs = Arc::new(Mutex::new(Vec::new()));
    match case.device {
        Device::Cpu => {
            let session = base_builder(case.device, logs.clone())?
                .commit_from_file(&path)
                .map_err(ort_err)?;
            Ok((session, "n/d".into(), Vec::new()))
        }
        Device::DirectMl => {
            let strict = base_builder(case.device, logs.clone())?
                .with_disable_cpu_fallback()
                .map_err(ort_err)?
                .commit_from_file(&path);
            if let Ok(session) = strict {
                return Ok((session, "tak".into(), Vec::new()));
            }
            let logs = Arc::new(Mutex::new(Vec::new()));
            let session = base_builder(case.device, logs.clone())?
                .commit_from_file(&path)
                .map_err(ort_err)?;
            let lines = logs.lock().map(|l| l.clone()).unwrap_or_default();
            let cpu_nodes = cpu_placed_nodes(&lines);
            Ok((
                session,
                format!("nie ({} węzłów na CPU)", cpu_nodes.len()),
                cpu_nodes,
            ))
        }
    }
}

fn cpu_placed_nodes(lines: &[String]) -> Vec<String> {
    let mut on_cpu = false;
    let mut nodes = Vec::new();
    for line in lines.iter().flat_map(|l| l.lines()) {
        if line.contains("placed on") {
            on_cpu = line.contains("CPUExecutionProvider");
        } else if on_cpu && !line.trim().is_empty() {
            nodes.push(line.trim().to_owned());
        }
    }
    nodes
}

fn tensor_of(shape: Vec<i64>, fp16: bool, value_at: impl Fn(usize) -> f32) -> Result<DynValue> {
    let len = shape.iter().product::<i64>() as usize;
    Ok(if fp16 {
        let data: Vec<f16> = (0..len).map(|i| f16::from_f32(value_at(i))).collect();
        Tensor::from_array((shape, data))
            .map_err(ort_err)?
            .into_dyn()
    } else {
        let data: Vec<f32> = (0..len).map(value_at).collect();
        Tensor::from_array((shape, data))
            .map_err(ort_err)?
            .into_dyn()
    })
}

fn synthetic_pixel(
    width: usize,
    height: usize,
    channels: usize,
    planar: bool,
) -> impl Fn(usize) -> f32 {
    move |i| {
        let (c, y, x) = if planar {
            (i / (width * height), (i / width) % height, i % width)
        } else {
            (
                i % channels,
                (i / (channels * width)) % height,
                (i / channels) % width,
            )
        };
        let base = match c {
            0 => x as f32 / width as f32,
            1 => y as f32 / height as f32,
            _ => ((x / 64 + y / 64) % 2) as f32,
        };
        base.clamp(0.0, 1.0)
    }
}

fn input_is_fp16(session: &Session, name: &str) -> bool {
    session
        .inputs()
        .iter()
        .find(|i| i.name() == name)
        .and_then(|i| i.dtype().tensor_type())
        == Some(TensorElementType::Float16)
}

fn run_case(models_dir: &Path, case: Case, frames: usize) -> Result<Measurement> {
    let session_start = Instant::now();
    let (mut session, all_on_gpu, cpu_nodes) = build_session(models_dir, case)?;
    let session_ms = session_start.elapsed().as_secs_f64() * 1000.0;
    for node in &cpu_nodes {
        println!("CPU_NODES {node}");
    }

    let mut timings = Vec::with_capacity(frames);
    let cpu_before;
    match case.model {
        Model::Rvm { .. } | Model::RvmStatic { .. } => {
            let fp16 = input_is_fp16(&session, "src");
            let (width, height, ratio) = match case.model {
                Model::Rvm { width, ratio, .. } => {
                    (width, width * FRAME_HEIGHT / FRAME_WIDTH, Some(ratio))
                }
                Model::RvmStatic { width, height } => (width, height, None),
                Model::MediaPipe { .. } => unreachable!(),
            };
            let state_shapes = ["r1i", "r2i", "r3i", "r4i"]
                .iter()
                .map(|name| {
                    let input = session
                        .inputs()
                        .iter()
                        .find(|i| i.name() == *name)
                        .context("missing recurrent input")?;
                    let shape = input
                        .dtype()
                        .tensor_shape()
                        .context("state is not a tensor")?;
                    Ok(shape.iter().map(|&d| d.max(1)).collect::<Vec<i64>>())
                })
                .collect::<Result<Vec<_>>>()?;
            let (w, h) = (width as usize, height as usize);
            let src = tensor_of(
                vec![1, 3, height, width],
                fp16,
                synthetic_pixel(w, h, 3, true),
            )?;
            let ratio_tensor: Option<DynValue> = match ratio {
                Some(r) => Some(
                    Tensor::from_array((vec![1i64], vec![r]))
                        .map_err(ort_err)?
                        .into_dyn(),
                ),
                None => None,
            };
            let mut states = state_shapes
                .iter()
                .map(|shape| tensor_of(shape.clone(), fp16, |_| 0.0))
                .collect::<Result<Vec<_>>>()?;
            let run_options = RunOptions::new().map_err(ort_err)?.with_outputs(
                OutputSelector::no_default()
                    .with("pha")
                    .with("r1o")
                    .with("r2o")
                    .with("r3o")
                    .with("r4o"),
            );

            let step = |session: &mut Session, states: &mut Vec<DynValue>| -> Result<Duration> {
                let start = Instant::now();
                let mut inputs = ort::inputs![
                    "src" => &src,
                    "r1i" => &states[0],
                    "r2i" => &states[1],
                    "r3i" => &states[2],
                    "r4i" => &states[3],
                ];
                if let Some(ratio_tensor) = &ratio_tensor {
                    inputs.push(("downsample_ratio".into(), ratio_tensor.into()));
                }
                let mut outputs = session
                    .run_with_options(inputs, &run_options)
                    .map_err(ort_err)?;
                let pha = outputs.remove("pha").context("missing pha")?;
                let next = ["r1o", "r2o", "r3o", "r4o"]
                    .iter()
                    .map(|n| outputs.remove(*n).context("missing state"))
                    .collect::<Result<Vec<_>>>()?;
                drop(outputs);
                touch_output(&pha, fp16)?;
                *states = next;
                Ok(start.elapsed())
            };

            for _ in 0..WARMUP_FRAMES {
                step(&mut session, &mut states)?;
            }
            cpu_before = process_cpu_time();
            for _ in 0..frames {
                timings.push(step(&mut session, &mut states)?);
            }
        }
        Model::MediaPipe { landscape } => {
            let (w, h) = if landscape {
                (256usize, 144usize)
            } else {
                (256, 256)
            };
            let input_name = session.inputs()[0].name().to_owned();
            let input = tensor_of(
                vec![1, h as i64, w as i64, 3],
                false,
                synthetic_pixel(w, h, 3, false),
            )?;
            let step = |session: &mut Session| -> Result<Duration> {
                let start = Instant::now();
                let outputs = session
                    .run(ort::inputs![input_name.as_str() => &input])
                    .map_err(ort_err)?;
                touch_output(&outputs[0], false)?;
                Ok(start.elapsed())
            };
            for _ in 0..WARMUP_FRAMES {
                step(&mut session)?;
            }
            cpu_before = process_cpu_time();
            for _ in 0..frames {
                timings.push(step(&mut session)?);
            }
        }
    }
    let cpu_ms_per_frame = (process_cpu_time() - cpu_before).as_secs_f64() * 1000.0 / frames as f64;
    let (working_set, private) = process_memory()?;
    let vram = match case.device {
        Device::DirectMl => process_vram()?,
        Device::Cpu => 0,
    };

    let mut ms: Vec<f64> = timings.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let mean_ms = ms.iter().sum::<f64>() / ms.len() as f64;
    let p95_ms = ms[((ms.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)];
    if mean_ms.is_nan() {
        bail!("no frames measured");
    }

    Ok(Measurement {
        all_on_gpu,
        session_ms,
        mean_ms,
        p95_ms,
        cpu_ms_per_frame,
        working_set_mb: working_set as f64 / 1_048_576.0,
        private_mb: private as f64 / 1_048_576.0,
        vram_mb: vram as f64 / 1_048_576.0,
    })
}

fn touch_output(value: &DynValue, fp16: bool) -> Result<()> {
    if fp16 {
        let (_, data) = value.try_extract_tensor::<f16>().map_err(ort_err)?;
        std::hint::black_box(data[data.len() / 2].to_f32());
    } else {
        let (_, data) = value.try_extract_tensor::<f32>().map_err(ort_err)?;
        std::hint::black_box(data[data.len() / 2]);
    }
    Ok(())
}

fn filetime_to_duration(ft: FILETIME) -> Duration {
    let ticks = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    Duration::from_nanos(ticks * 100)
}

fn process_cpu_time() -> Duration {
    let (mut creation, mut exit, mut kernel, mut user) = Default::default();
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok.is_err() {
        return Duration::ZERO;
    }
    filetime_to_duration(kernel) + filetime_to_duration(user)
}

fn process_memory() -> Result<(usize, usize)> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    }
    .ok()?;
    Ok((counters.WorkingSetSize, counters.PrivateUsage))
}

fn adapter3() -> Result<IDXGIAdapter3> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    Ok(unsafe { factory.EnumAdapters1(0)? }.cast()?)
}

fn adapter_description() -> Result<String> {
    let desc = unsafe { adapter3()?.GetDesc2()? };
    let len = desc
        .Description
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(desc.Description.len());
    Ok(format!(
        "{} ({} MB VRAM)",
        String::from_utf16_lossy(&desc.Description[..len]),
        desc.DedicatedVideoMemory / 1_048_576
    ))
}

fn process_vram() -> Result<u64> {
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    unsafe { adapter3()?.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)? };
    Ok(info.CurrentUsage)
}
