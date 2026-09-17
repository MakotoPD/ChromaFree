use std::path::Path;

use anyhow::Result;
use bgcam_core::{
    CpuPipeline, InferenceDevice, Nv12Frame, OnnxModelProvider, OutputFormat, PipelineOutput, PipelineSettings,
};
use bgcam_gpu::GpuPipeline;

const CPU_FALLBACK_THREADS: usize = 2;

pub enum Processor {
    Gpu(Box<GpuPipeline>),
    Cpu(Box<CpuPipeline>),
}

impl Processor {
    pub fn create(models_dir: &Path, mut settings: PipelineSettings) -> Self {
        if let InferenceDevice::DirectMl { adapter_index } = settings.segmentation.device {
            match GpuPipeline::new(adapter_index, models_dir, settings.clone()) {
                Ok(pipeline) => {
                    tracing::info!(adapter = pipeline.adapter_name(), "GPU pipeline ready");
                    return Self::Gpu(Box::new(pipeline));
                }
                Err(error) => {
                    tracing::warn!(%error, "GPU pipeline unavailable, falling back to CPU");
                    settings.segmentation.device = InferenceDevice::Cpu {
                        threads: CPU_FALLBACK_THREADS,
                    };
                }
            }
        }
        Self::Cpu(Box::new(CpuPipeline::new(
            Box::new(OnnxModelProvider::new(models_dir)),
            settings,
        )))
    }

    pub fn runs_on(&self, device: InferenceDevice) -> bool {
        matches!(
            (self, device),
            (Self::Gpu(_), InferenceDevice::DirectMl { .. }) | (Self::Cpu(_), InferenceDevice::Cpu { .. })
        )
    }

    pub fn backend(&self) -> String {
        match self {
            Self::Gpu(pipeline) => format!("GPU: {}", pipeline.adapter_name()),
            Self::Cpu(_) => "CPU".to_owned(),
        }
    }

    pub fn settings(&self) -> &PipelineSettings {
        match self {
            Self::Gpu(pipeline) => pipeline.settings(),
            Self::Cpu(pipeline) => pipeline.settings(),
        }
    }

    pub fn active_model(&self) -> Option<&str> {
        match self {
            Self::Gpu(pipeline) => pipeline.active_model(),
            Self::Cpu(pipeline) => pipeline.active_model(),
        }
    }

    pub fn apply_settings(&mut self, settings: PipelineSettings) -> Result<()> {
        match self {
            Self::Gpu(pipeline) => pipeline.apply_settings(settings)?,
            Self::Cpu(pipeline) => pipeline.apply_settings(settings),
        }
        Ok(())
    }

    pub fn process(&mut self, input: &Nv12Frame, format: OutputFormat) -> Result<PipelineOutput<'_>> {
        Ok(match self {
            Self::Gpu(pipeline) => pipeline.process(input, format)?,
            Self::Cpu(pipeline) => pipeline.process(input, format)?,
        })
    }
}
