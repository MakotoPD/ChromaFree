use windows::Win32::Graphics::Direct3D12::ID3D12PipelineState;

use crate::device::GpuDevice;
use crate::error::GpuError;

const FRAME_SAMPLING: &str = include_str!("../shaders/frame_sampling.hlsli");
const INCLUDE_LINE: &str = "#include \"frame_sampling.hlsli\"";

pub(crate) struct Shaders {
    pub orient_scale: ID3D12PipelineState,
    pub preprocess_half: ID3D12PipelineState,
    pub preprocess_float: ID3D12PipelineState,
    pub mask_from_half: ID3D12PipelineState,
    pub mask_from_float: ID3D12PipelineState,
    pub mask_from_upload: ID3D12PipelineState,
    pub clear: ID3D12PipelineState,
    pub blur_to_half: ID3D12PipelineState,
    pub blur_downsample: ID3D12PipelineState,
    pub blur_resize: ID3D12PipelineState,
    pub blur_tent: ID3D12PipelineState,
    pub blur_from_half: ID3D12PipelineState,
    pub composite_nv12: ID3D12PipelineState,
    pub composite_bgra: ID3D12PipelineState,
}

fn expand(source: &str) -> String {
    source.replace(INCLUDE_LINE, FRAME_SAMPLING)
}

impl Shaders {
    pub fn compile(device: &GpuDevice) -> Result<Self, GpuError> {
        let preprocess = expand(include_str!("../shaders/preprocess.hlsl"));
        let mask = include_str!("../shaders/mask.hlsl");
        let blur = expand(include_str!("../shaders/blur.hlsl"));
        let composite = expand(include_str!("../shaders/composite.hlsl"));
        Ok(Self {
            orient_scale: device.compile(include_str!("../shaders/orient_scale.hlsl"), "main")?,
            preprocess_half: device.compile(&preprocess, "planar_half")?,
            preprocess_float: device.compile(&preprocess, "interleaved_float")?,
            mask_from_half: device.compile(mask, "from_half")?,
            mask_from_float: device.compile(mask, "from_float")?,
            mask_from_upload: device.compile(mask, "from_upload")?,
            clear: device.compile(include_str!("../shaders/clear.hlsl"), "main")?,
            blur_to_half: device.compile(&blur, "to_half")?,
            blur_downsample: device.compile(&blur, "downsample")?,
            blur_resize: device.compile(&blur, "resize")?,
            blur_tent: device.compile(&blur, "tent")?,
            blur_from_half: device.compile(&blur, "from_half")?,
            composite_nv12: device.compile(&composite, "nv12")?,
            composite_bgra: device.compile(&composite, "bgra")?,
        })
    }
}
