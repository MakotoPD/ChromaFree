use std::path::PathBuf;
use std::sync::Arc;

use chromafree_core::segmentation::AlphaMask;
use chromafree_core::{
    BackgroundEffect, BgraFrame, ColorMatrix, FrameSize, MaskParams, MediaPipeVariant, Nv12Frame, OnnxModelProvider,
    Orientation, OutputFormat, PipelineOutput, PipelineSettings, Rgb, RgbImage, Rotation, SegmentationMethod,
    VariantPreference,
};
use windows::Win32::Graphics::Direct3D12::{ID3D12PipelineState, ID3D12Resource};

use crate::device::{GpuDevice, MappedBuffer, Pass, ROOT_CONSTANTS, groups};
use crate::error::GpuError;
use crate::inference::{Element, GpuModel};
use crate::shaders::Shaders;

const MIN_LUMA_LEVELS: f32 = 1.0;
const MAX_LUMA_LEVELS: f32 = 6.0;
const COMPOSITE_COLOR: u32 = 0;
const COMPOSITE_IMAGE: u32 = 1;
const COMPOSITE_BLUR: u32 = 2;
const COMPOSITE_PASSTHROUGH: u32 = 3;

#[derive(Clone, Copy, PartialEq)]
struct ModelKey {
    method: SegmentationMethod,
    variant: VariantPreference,
    frame: FrameSize,
}

struct BlurLevel {
    width: u32,
    height: u32,
    data: ID3D12Resource,
    scratch: ID3D12Resource,
}

struct BlurChain {
    strength: f32,
    levels: Vec<BlurLevel>,
    fractional: Option<BlurLevel>,
    background: ID3D12Resource,
}

struct ImageUpload {
    source: Arc<RgbImage>,
    matrix: ColorMatrix,
    buffer: MappedBuffer,
}

struct MaskBuffers {
    size: FrameSize,
    history: ID3D12Resource,
    mask: ID3D12Resource,
    upload: MappedBuffer,
    history_valid: bool,
}

struct CameraUpload {
    size: FrameSize,
    buffer: MappedBuffer,
}

pub struct GpuPipeline {
    device: GpuDevice,
    shaders: Shaders,
    models: OnnxModelProvider,
    models_dir: PathBuf,
    settings: PipelineSettings,
    camera: Option<CameraUpload>,
    frame: ID3D12Resource,
    nv12_output: ID3D12Resource,
    nv12_readback: MappedBuffer,
    nv12: Nv12Frame,
    bgra_output: ID3D12Resource,
    bgra_readback: MappedBuffer,
    bgra: BgraFrame,
    placeholder: ID3D12Resource,
    placeholder_upload: MappedBuffer,
    model: Option<(ModelKey, GpuModel)>,
    mask: Option<MaskBuffers>,
    blur: Option<BlurChain>,
    image: Option<ImageUpload>,
}

fn float(value: f32) -> u32 {
    value.to_bits()
}

fn frame_bytes(size: FrameSize) -> usize {
    (size.luma_len() + size.chroma_len()) * 4
}

fn orientation_map(orientation: Orientation, source: FrameSize, target: FrameSize) -> [f32; 6] {
    let oriented = orientation.output_size(source);
    let (ow, oh) = (f64::from(oriented.width()), f64::from(oriented.height()));
    let (tw, th) = (f64::from(target.width()), f64::from(target.height()));
    let scale = (tw / ow).max(th / oh);
    let (rw, rh) = (tw / scale, th / scale);
    let (rx, ry) = ((ow - rw) / 2.0, (oh - rh) / 2.0);
    let (sw, sh) = (f64::from(source.width()), f64::from(source.height()));
    let map = |ox: f64, oy: f64| {
        let (qx, qy) = (rx + ox * rw / tw, ry + oy * rh / th);
        let (mut sx, mut sy) = match orientation.rotation {
            Rotation::None => (qx, qy),
            Rotation::Clockwise90 => (qy, sh - qx),
            Rotation::Clockwise180 => (sw - qx, sh - qy),
            Rotation::Clockwise270 => (sw - qy, qx),
        };
        if orientation.mirror_horizontal {
            sx = sw - sx;
        }
        if orientation.mirror_vertical {
            sy = sh - sy;
        }
        (sx, sy)
    };
    let (x0, y0) = map(0.0, 0.0);
    let (x1, y1) = map(1.0, 0.0);
    let (x2, y2) = map(0.0, 1.0);
    [
        (x1 - x0) as f32,
        (x2 - x0) as f32,
        x0 as f32,
        (y1 - y0) as f32,
        (y2 - y0) as f32,
        y0 as f32,
    ]
}

fn inverse_constants(matrix: ColorMatrix) -> [u32; 4] {
    let k = matrix.inverse();
    [
        float(k.red_v as f32 / 256.0),
        float(k.green_u as f32 / 256.0),
        float(k.green_v as f32 / 256.0),
        float(k.blue_u as f32 / 256.0),
    ]
}

fn constants(values: &[u32]) -> [u32; ROOT_CONSTANTS] {
    let mut out = [0; ROOT_CONSTANTS];
    out[..values.len()].copy_from_slice(values);
    out
}

impl GpuPipeline {
    pub fn new(
        adapter_index: u32,
        models_dir: impl Into<PathBuf>,
        settings: PipelineSettings,
    ) -> Result<Self, GpuError> {
        let device = GpuDevice::new(adapter_index)?;
        let shaders = Shaders::compile(&device)?;
        let models_dir = models_dir.into();
        let output = settings.output;
        let placeholder = device.storage_buffer(16)?;
        let mut placeholder_upload = device.upload_buffer(16)?;
        placeholder_upload.bytes_mut().fill(0);
        Ok(Self {
            frame: device.storage_buffer(frame_bytes(output))?,
            nv12_output: device.storage_buffer(output.luma_len() + output.chroma_len())?,
            nv12_readback: device.readback_buffer(output.luma_len() + output.chroma_len())?,
            nv12: Nv12Frame::new(output),
            bgra_output: device.storage_buffer(output.luma_len() * 4)?,
            bgra_readback: device.readback_buffer(output.luma_len() * 4)?,
            bgra: BgraFrame::new(output),
            placeholder,
            placeholder_upload,
            models: OnnxModelProvider::new(models_dir.clone()),
            models_dir,
            device,
            shaders,
            settings,
            camera: None,
            model: None,
            mask: None,
            blur: None,
            image: None,
        })
    }

    pub fn adapter_name(&self) -> &str {
        self.device.adapter_name()
    }

    pub fn settings(&self) -> &PipelineSettings {
        &self.settings
    }

    pub fn active_model(&self) -> Option<&str> {
        self.model.as_ref().map(|(_, model)| model.name())
    }

    pub fn apply_settings(&mut self, settings: PipelineSettings) -> Result<(), GpuError> {
        if settings.output != self.settings.output {
            let output = settings.output;
            self.frame = self.device.storage_buffer(frame_bytes(output))?;
            self.nv12_output = self.device.storage_buffer(output.luma_len() + output.chroma_len())?;
            self.nv12_readback = self.device.readback_buffer(output.luma_len() + output.chroma_len())?;
            self.nv12 = Nv12Frame::new(output);
            self.bgra_output = self.device.storage_buffer(output.luma_len() * 4)?;
            self.bgra_readback = self.device.readback_buffer(output.luma_len() * 4)?;
            self.bgra = BgraFrame::new(output);
            self.model = None;
            self.mask = None;
            self.blur = None;
            self.image = None;
        }
        if settings.orientation != self.settings.orientation {
            self.invalidate_history();
        }
        if !settings.effect.needs_mask() {
            self.model = None;
            self.mask = None;
        }
        self.settings = settings;
        Ok(())
    }

    pub fn reset_temporal_state(&mut self) -> Result<(), GpuError> {
        self.invalidate_history();
        let Some((_, model)) = &self.model else {
            return Ok(());
        };
        self.device.begin(true)?;
        for state in model.state_buffers() {
            self.clear(state, u32::MAX / 4);
        }
        self.device.submit()?;
        self.device.wait()
    }

    fn invalidate_history(&mut self) {
        if let Some(mask) = &mut self.mask {
            mask.history_valid = false;
        }
    }

    fn clear(&self, target: &ID3D12Resource, elements: u32) {
        let count = elements.min(unsafe { target.GetDesc() }.Width as u32 / 4);
        self.device.dispatch(&Pass {
            pipeline: &self.shaders.clear,
            constants: constants(&[count]),
            shader_resource: self.placeholder_upload.resource(),
            unordered: [target, &self.placeholder, &self.placeholder, &self.placeholder],
            groups: (count.div_ceil(256), 1),
        });
    }

    pub fn process(&mut self, input: &Nv12Frame, format: OutputFormat) -> Result<PipelineOutput<'_>, GpuError> {
        self.process_inner(input, None, format)
    }

    pub fn process_with_alpha(
        &mut self,
        input: &Nv12Frame,
        alpha: &AlphaMask<'_>,
        format: OutputFormat,
    ) -> Result<PipelineOutput<'_>, GpuError> {
        self.process_inner(input, Some(alpha), format)
    }

    fn upload_camera(&mut self, input: &Nv12Frame) -> Result<(), GpuError> {
        let size = input.size();
        if self.camera.as_ref().is_none_or(|c| c.size != size) {
            self.camera = Some(CameraUpload {
                size,
                buffer: self.device.upload_buffer(size.luma_len() + size.chroma_len())?,
            });
            self.invalidate_history();
        }
        if let Some(camera) = &mut self.camera {
            let bytes = camera.buffer.bytes_mut();
            bytes[..size.luma_len()].copy_from_slice(input.luma());
            bytes[size.luma_len()..].copy_from_slice(input.chroma());
        }
        Ok(())
    }

    fn orient_and_scale(&self) -> Result<(), GpuError> {
        let camera = self
            .camera
            .as_ref()
            .ok_or_else(|| GpuError::Inference("camera frame not uploaded".into()))?;
        let output = self.settings.output;
        let map = orientation_map(self.settings.orientation, camera.size, output);
        for plane in 0..2u32 {
            let (w, h) = if plane == 0 {
                (output.width(), output.height())
            } else {
                (output.chroma_width(), output.chroma_height())
            };
            self.device.dispatch(&Pass {
                pipeline: &self.shaders.orient_scale,
                constants: constants(&[
                    camera.size.width(),
                    camera.size.height(),
                    output.width(),
                    output.height(),
                    plane,
                    0,
                    0,
                    0,
                    float(map[0]),
                    float(map[1]),
                    float(map[2]),
                    float(map[3]),
                    float(map[4]),
                    float(map[5]),
                ]),
                shader_resource: camera.buffer.resource(),
                unordered: [&self.frame, &self.placeholder, &self.placeholder, &self.placeholder],
                groups: groups(w, h),
            });
        }
        Ok(())
    }

    fn ensure_model(&mut self) -> Result<(), GpuError> {
        let output = self.settings.output;
        let segmentation = self.settings.segmentation;
        let key = ModelKey {
            method: segmentation.method,
            variant: segmentation.variant,
            frame: output,
        };
        if self.model.as_ref().is_some_and(|(k, _)| *k == key) {
            return Ok(());
        }
        self.model = None;
        let model = match segmentation.method {
            SegmentationMethod::Rvm => {
                let variant = self.models.resolve_rvm_variant(segmentation.variant, output)?;
                GpuModel::load_rvm(&self.device, &self.models_dir, variant)?
            }
            SegmentationMethod::MediaPipe => {
                GpuModel::load_mediapipe(&self.device, &self.models_dir, MediaPipeVariant::for_frame(output))?
            }
        };
        self.model = Some((key, model));
        self.mask = None;
        Ok(())
    }

    fn ensure_mask_buffers(&mut self, size: FrameSize) -> Result<(), GpuError> {
        if self.mask.as_ref().is_none_or(|m| m.size != size) {
            self.mask = Some(MaskBuffers {
                size,
                history: self.device.storage_buffer(size.luma_len() * 4)?,
                mask: self.device.storage_buffer(size.luma_len() * 4)?,
                upload: self.device.upload_buffer(size.luma_len() * 4)?,
                history_valid: false,
            });
        }
        Ok(())
    }

    fn mask_params(&self) -> MaskParams {
        self.settings.segmentation.mask.unwrap_or(match &self.model {
            Some((_, model)) if !model.is_recurrent() => MaskParams::MEMORYLESS_MODEL,
            _ => MaskParams::RECURRENT_MODEL,
        })
    }

    fn refine_mask(&mut self, pipeline: MaskSource) -> Result<FrameSize, GpuError> {
        let params = self.mask_params();
        let Some(mask) = &mut self.mask else {
            return Err(GpuError::Inference("mask buffers missing".into()));
        };
        let size = mask.size;
        let use_history = params.temporal_smoothing > 0.0;
        let keep = if mask.history_valid {
            params.temporal_smoothing.clamp(0.0, 0.98)
        } else {
            0.0
        };
        mask.history_valid = use_history;
        let (shader, shader_resource, half, float_alpha) = match pipeline {
            MaskSource::Upload => (
                &self.shaders.mask_from_upload,
                mask.upload.resource(),
                &self.placeholder,
                &self.placeholder,
            ),
            MaskSource::Model(Element::Half) => {
                let alpha = self.model.as_ref().map(|(_, m)| m.alpha()).unwrap_or(&self.placeholder);
                (
                    &self.shaders.mask_from_half,
                    self.placeholder_upload.resource(),
                    alpha,
                    &self.placeholder,
                )
            }
            MaskSource::Model(Element::Float) => {
                let alpha = self.model.as_ref().map(|(_, m)| m.alpha()).unwrap_or(&self.placeholder);
                (
                    &self.shaders.mask_from_float,
                    self.placeholder_upload.resource(),
                    &self.placeholder,
                    alpha,
                )
            }
        };
        self.device.dispatch(&Pass {
            pipeline: shader,
            constants: constants(&[
                size.width(),
                size.height(),
                u32::from(use_history),
                0,
                float(keep),
                float(params.edge_low),
                float(params.edge_high),
            ]),
            shader_resource,
            unordered: [half, &mask.history, &mask.mask, float_alpha],
            groups: groups(size.width(), size.height()),
        });
        Ok(size)
    }

    fn ensure_blur(&mut self, strength: f32) -> Result<(), GpuError> {
        let strength = strength.clamp(0.0, 1.0);
        if self.blur.as_ref().is_some_and(|b| b.strength == strength) {
            return Ok(());
        }
        let output = self.settings.output;
        let half = (output.chroma_width(), output.chroma_height());
        let levels = MIN_LUMA_LEVELS + strength * (MAX_LUMA_LEVELS - MIN_LUMA_LEVELS) - 1.0;
        let depth = levels.floor() as usize;
        let mut dimensions = vec![half];
        let (mut w, mut h) = half;
        for _ in 0..depth {
            if w < 4 || h < 4 {
                break;
            }
            w = w.div_ceil(2);
            h = h.div_ceil(2);
            dimensions.push((w, h));
        }
        let fraction = if dimensions.len() == depth + 1 {
            levels - depth as f32
        } else {
            0.0
        };
        let make = |(width, height): (u32, u32)| -> Result<BlurLevel, GpuError> {
            let bytes = width as usize * height as usize * 3 * 4;
            Ok(BlurLevel {
                width,
                height,
                data: self.device.storage_buffer(bytes)?,
                scratch: self.device.storage_buffer(bytes)?,
            })
        };
        let levels_buffers = dimensions.iter().copied().map(make).collect::<Result<Vec<_>, _>>()?;
        let (base_w, base_h) = *dimensions.last().unwrap_or(&half);
        let factor = 2f32.powf(fraction);
        let frac_dims = (
            ((base_w as f32 / factor).round() as u32).max(2),
            ((base_h as f32 / factor).round() as u32).max(2),
        );
        let fractional = if fraction > 0.01 && frac_dims != (base_w, base_h) {
            Some(make(frac_dims)?)
        } else {
            None
        };
        self.blur = Some(BlurChain {
            strength,
            levels: levels_buffers,
            fractional,
            background: self.device.storage_buffer(frame_bytes(output))?,
        });
        Ok(())
    }

    fn blur_pass(
        &self,
        pipeline: &ID3D12PipelineState,
        source: (&ID3D12Resource, u32, u32),
        target: (&ID3D12Resource, u32, u32),
    ) {
        let output = self.settings.output;
        self.device.dispatch(&Pass {
            pipeline,
            constants: constants(&[source.1, source.2, target.1, target.2, output.width(), output.height()]),
            shader_resource: self.placeholder_upload.resource(),
            unordered: [source.0, target.0, &self.placeholder, &self.placeholder],
            groups: groups(target.1, target.2),
        });
    }

    fn run_blur(&self) -> Result<(), GpuError> {
        let Some(chain) = &self.blur else {
            return Err(GpuError::Inference("blur chain missing".into()));
        };
        let output = self.settings.output;
        let half = &chain.levels[0];
        self.blur_pass(
            &self.shaders.blur_to_half,
            (&self.frame, output.width(), output.height()),
            (&half.data, half.width, half.height),
        );
        for pair in chain.levels.windows(2) {
            self.blur_pass(
                &self.shaders.blur_downsample,
                (&pair[0].data, pair[0].width, pair[0].height),
                (&pair[1].data, pair[1].width, pair[1].height),
            );
        }
        let deepest = &chain.levels[chain.levels.len() - 1];
        if let Some(fractional) = &chain.fractional {
            self.blur_pass(
                &self.shaders.blur_resize,
                (&deepest.data, deepest.width, deepest.height),
                (&fractional.data, fractional.width, fractional.height),
            );
            self.blur_pass(
                &self.shaders.blur_tent,
                (&fractional.data, fractional.width, fractional.height),
                (&fractional.scratch, fractional.width, fractional.height),
            );
            self.blur_pass(
                &self.shaders.blur_resize,
                (&fractional.scratch, fractional.width, fractional.height),
                (&deepest.data, deepest.width, deepest.height),
            );
        }
        self.blur_pass(
            &self.shaders.blur_tent,
            (&deepest.data, deepest.width, deepest.height),
            (&deepest.scratch, deepest.width, deepest.height),
        );
        for index in (1..chain.levels.len()).rev() {
            let child = &chain.levels[index];
            let parent = &chain.levels[index - 1];
            self.blur_pass(
                &self.shaders.blur_resize,
                (&child.scratch, child.width, child.height),
                (&parent.data, parent.width, parent.height),
            );
            self.blur_pass(
                &self.shaders.blur_tent,
                (&parent.data, parent.width, parent.height),
                (&parent.scratch, parent.width, parent.height),
            );
        }
        self.blur_pass(
            &self.shaders.blur_from_half,
            (&half.scratch, half.width, half.height),
            (&chain.background, output.width(), output.height()),
        );
        Ok(())
    }

    fn ensure_image(&mut self, source: &Arc<RgbImage>, matrix: ColorMatrix) -> Result<(), GpuError> {
        if self
            .image
            .as_ref()
            .is_some_and(|image| Arc::ptr_eq(&image.source, source) && image.matrix == matrix)
        {
            return Ok(());
        }
        let output = self.settings.output;
        let frame = source.to_nv12(output, matrix)?;
        let mut buffer = self.device.upload_buffer(frame_bytes(output))?;
        for (chunk, value) in buffer
            .bytes_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(frame.luma().iter().chain(frame.chroma()))
        {
            chunk.copy_from_slice(&f32::from(*value).to_le_bytes());
        }
        self.image = Some(ImageUpload {
            source: Arc::clone(source),
            matrix,
            buffer,
        });
        Ok(())
    }

    fn process_inner(
        &mut self,
        input: &Nv12Frame,
        external_alpha: Option<&AlphaMask<'_>>,
        format: OutputFormat,
    ) -> Result<PipelineOutput<'_>, GpuError> {
        let matrix = self.settings.matrix();
        let effect = self.settings.effect.clone();
        let output = self.settings.output;
        self.upload_camera(input)?;
        if let BackgroundEffect::Blur { strength } = effect {
            self.ensure_blur(strength)?;
        }
        if let BackgroundEffect::Image(source) = &effect {
            self.ensure_image(source, matrix)?;
        }

        self.device.begin(true)?;
        self.orient_and_scale()?;

        let mask_size = if !effect.needs_mask() {
            None
        } else if let Some(alpha) = external_alpha {
            let size = FrameSize::new(alpha.width, alpha.height)?;
            self.ensure_mask_buffers(size)?;
            if let Some(mask) = &mut self.mask {
                for (chunk, &value) in mask
                    .upload
                    .bytes_mut()
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(alpha.data)
                {
                    chunk.copy_from_slice(&(f32::from(value) / 255.0).to_le_bytes());
                }
            }
            Some(self.refine_mask(MaskSource::Upload)?)
        } else {
            self.device.submit()?;
            self.ensure_model()?;
            let (model_size, element) = match &self.model {
                Some((_, model)) => (model.size(), model.input_element()),
                None => return Err(GpuError::Inference("model missing".into())),
            };
            self.ensure_mask_buffers(model_size)?;
            self.device.begin(false)?;
            if let Some((_, model)) = &self.model {
                let pipeline = match element {
                    Element::Half => &self.shaders.preprocess_half,
                    Element::Float => &self.shaders.preprocess_float,
                };
                let (half_target, float_target) = match element {
                    Element::Half => (model.input(), &self.placeholder),
                    Element::Float => (&self.placeholder, model.input()),
                };
                let dispatch_width = match element {
                    Element::Half => model_size.width() / 2,
                    Element::Float => model_size.width(),
                };
                let k = inverse_constants(matrix);
                self.device.dispatch(&Pass {
                    pipeline,
                    constants: constants(&[
                        output.width(),
                        output.height(),
                        model_size.width(),
                        model_size.height(),
                        k[0],
                        k[1],
                        k[2],
                        k[3],
                    ]),
                    shader_resource: self.placeholder_upload.resource(),
                    unordered: [&self.frame, half_target, float_target, &self.placeholder],
                    groups: groups(dispatch_width, model_size.height()),
                });
            }
            self.device.submit()?;
            if let Some((_, model)) = &mut self.model {
                model.run()?;
            }
            self.device.begin(false)?;
            Some(self.refine_mask(MaskSource::Model(element))?)
        };

        let (mode, background_color) = match &effect {
            BackgroundEffect::Passthrough => (COMPOSITE_PASSTHROUGH, Rgb::new(0, 0, 0)),
            BackgroundEffect::Color(rgb) => (COMPOSITE_COLOR, *rgb),
            BackgroundEffect::Transparent { fallback } => (COMPOSITE_COLOR, *fallback),
            BackgroundEffect::Image(_) => (COMPOSITE_IMAGE, Rgb::new(0, 0, 0)),
            BackgroundEffect::Blur { .. } => (COMPOSITE_BLUR, Rgb::new(0, 0, 0)),
        };
        if mode == COMPOSITE_BLUR {
            self.run_blur()?;
        }
        let alpha_from_mask = matches!(effect, BackgroundEffect::Transparent { .. }) && format == OutputFormat::Bgra;
        let background = matrix.to_yuv(background_color);
        let k = inverse_constants(matrix);
        let (mask_width, mask_height) = mask_size.map_or((1, 1), |size| (size.width(), size.height()));
        let mask_buffer = self.mask.as_ref().map(|m| &m.mask).unwrap_or(&self.placeholder);
        let blur_buffer = self.blur.as_ref().map(|b| &b.background).unwrap_or(&self.placeholder);
        let image_buffer = self
            .image
            .as_ref()
            .filter(|_| mode == COMPOSITE_IMAGE)
            .map(|i| i.buffer.resource())
            .unwrap_or(self.placeholder_upload.resource());
        let (pipeline, target, dispatch) = match format {
            OutputFormat::Nv12 => (
                &self.shaders.composite_nv12,
                &self.nv12_output,
                groups(output.width() / 4, output.height() + output.height() / 2),
            ),
            OutputFormat::Bgra => (
                &self.shaders.composite_bgra,
                &self.bgra_output,
                groups(output.width(), output.height()),
            ),
        };
        self.device.dispatch(&Pass {
            pipeline,
            constants: constants(&[
                output.width(),
                output.height(),
                mask_width,
                mask_height,
                mode,
                u32::from(alpha_from_mask),
                0,
                0,
                float(f32::from(background.y)),
                float(f32::from(background.u)),
                float(f32::from(background.v)),
                k[0],
                k[1],
                k[2],
                k[3],
            ]),
            shader_resource: image_buffer,
            unordered: [&self.frame, mask_buffer, blur_buffer, target],
            groups: dispatch,
        });
        let readback = match format {
            OutputFormat::Nv12 => self.nv12_readback.resource().clone(),
            OutputFormat::Bgra => self.bgra_readback.resource().clone(),
        };
        self.device.copy_to_readback(target, &readback);
        self.device.submit()?;
        self.device.wait()?;

        match format {
            OutputFormat::Nv12 => {
                let bytes = self.nv12_readback.bytes();
                let (luma, chroma) = self.nv12.planes_mut();
                luma.copy_from_slice(&bytes[..output.luma_len()]);
                chroma.copy_from_slice(&bytes[output.luma_len()..output.luma_len() + output.chroma_len()]);
                Ok(PipelineOutput::Nv12(&self.nv12))
            }
            OutputFormat::Bgra => {
                self.bgra
                    .data_mut()
                    .copy_from_slice(&self.bgra_readback.bytes()[..output.luma_len() * 4]);
                Ok(PipelineOutput::Bgra(&self.bgra))
            }
        }
    }
}

enum MaskSource {
    Upload,
    Model(Element),
}
