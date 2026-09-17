use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use chromafree_capture::{CaptureFormat, Encoding};
use chromafree_core::{
    BackgroundEffect, FrameSize, InferenceDevice, MaskParams, ModelVariant, Orientation, PipelineSettings, Rgb,
    RgbImage, Rotation, SegmentationMethod, SegmentationSettings, VariantPreference,
};
use serde::{Deserialize, Serialize};

pub const GREEN_SCREEN: &str = "#00B140";
pub const BLUE_SCREEN: &str = "#0047BB";
const CPU_INFERENCE_THREADS: usize = 2;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub camera: CameraConfig,
    pub output: OutputConfig,
    pub orientation: OrientationConfig,
    pub effect: EffectConfig,
    pub segmentation: SegmentationConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraConfig {
    pub symbolic_link: Option<String>,
    pub name: Option<String>,
    pub format: Option<FormatConfig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatConfig {
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub encoding: EncodingConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EncodingConfig {
    Nv12,
    Yuy2,
    Mjpeg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 30,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OrientationConfig {
    pub rotation: u16,
    pub mirror_horizontal: bool,
    pub mirror_vertical: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectMode {
    Passthrough,
    #[default]
    Blur,
    Color,
    Image,
    Transparent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectConfig {
    pub mode: EffectMode,
    pub blur_strength: f32,
    pub color: String,
    pub image_path: Option<PathBuf>,
}

impl Default for EffectConfig {
    fn default() -> Self {
        Self {
            mode: EffectMode::default(),
            blur_strength: 0.5,
            color: GREEN_SCREEN.to_owned(),
            image_path: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MethodConfig {
    #[default]
    Rvm,
    MediaPipe,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceConfig {
    #[default]
    Gpu,
    Cpu,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SegmentationConfig {
    pub method: MethodConfig,
    pub quality: String,
    pub device: DeviceConfig,
    pub edge_low: Option<f32>,
    pub edge_high: Option<f32>,
    pub temporal_smoothing: Option<f32>,
}

impl Default for SegmentationConfig {
    fn default() -> Self {
        Self {
            method: MethodConfig::default(),
            quality: "auto".to_owned(),
            device: DeviceConfig::default(),
            edge_low: None,
            edge_high: None,
            temporal_smoothing: None,
        }
    }
}

impl FormatConfig {
    pub fn from_capture(format: &CaptureFormat) -> Option<Self> {
        let encoding = match format.encoding {
            Encoding::Nv12 => EncodingConfig::Nv12,
            Encoding::Yuy2 => EncodingConfig::Yuy2,
            Encoding::Mjpeg => EncodingConfig::Mjpeg,
            Encoding::Other(_) => return None,
        };
        Some(Self {
            width: format.width,
            height: format.height,
            fps_numerator: format.fps_numerator,
            fps_denominator: format.fps_denominator,
            encoding,
        })
    }

    pub fn to_capture(self) -> CaptureFormat {
        CaptureFormat {
            width: self.width,
            height: self.height,
            fps_numerator: self.fps_numerator,
            fps_denominator: self.fps_denominator,
            encoding: match self.encoding {
                EncodingConfig::Nv12 => Encoding::Nv12,
                EncodingConfig::Yuy2 => Encoding::Yuy2,
                EncodingConfig::Mjpeg => Encoding::Mjpeg,
            },
        }
    }
}

pub fn parse_color(text: &str) -> Result<Rgb> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("color {text} must look like #RRGGBB");
    }
    let value = u32::from_str_radix(hex, 16)?;
    Ok(Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    })
}

pub fn format_color(color: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b)
}

pub fn parse_quality(text: &str) -> Result<VariantPreference> {
    if text.eq_ignore_ascii_case("auto") {
        return Ok(VariantPreference::Auto);
    }
    let (width, height) = text
        .split_once('x')
        .with_context(|| format!("quality {text} must be auto or WIDTHxHEIGHT"))?;
    Ok(VariantPreference::Fixed(ModelVariant::new(
        width.parse()?,
        height.parse()?,
    )))
}

impl Config {
    pub fn output_size(&self) -> Result<FrameSize> {
        Ok(FrameSize::new(self.output.width, self.output.height)?)
    }

    pub fn orientation(&self) -> Orientation {
        Orientation {
            rotation: match self.orientation.rotation % 360 {
                90 => Rotation::Clockwise90,
                180 => Rotation::Clockwise180,
                270 => Rotation::Clockwise270,
                _ => Rotation::None,
            },
            mirror_horizontal: self.orientation.mirror_horizontal,
            mirror_vertical: self.orientation.mirror_vertical,
        }
    }

    pub fn default_mask_params(method: MethodConfig) -> MaskParams {
        match method {
            MethodConfig::Rvm => MaskParams::RECURRENT_MODEL,
            MethodConfig::MediaPipe => MaskParams::MEMORYLESS_MODEL,
        }
    }

    pub fn mask_params(&self) -> Option<MaskParams> {
        let segmentation = &self.segmentation;
        if segmentation.edge_low.is_none()
            && segmentation.edge_high.is_none()
            && segmentation.temporal_smoothing.is_none()
        {
            return None;
        }
        let defaults = Self::default_mask_params(segmentation.method);
        Some(MaskParams {
            temporal_smoothing: segmentation
                .temporal_smoothing
                .unwrap_or(defaults.temporal_smoothing)
                .clamp(0.0, 0.95),
            edge_low: segmentation.edge_low.unwrap_or(defaults.edge_low).clamp(0.0, 1.0),
            edge_high: segmentation.edge_high.unwrap_or(defaults.edge_high).clamp(0.0, 1.0),
        })
    }

    pub fn pipeline_settings(&self, background: Option<Arc<RgbImage>>) -> Result<PipelineSettings> {
        let color = parse_color(&self.effect.color)?;
        let effect = match (self.effect.mode, background) {
            (EffectMode::Passthrough, _) => BackgroundEffect::Passthrough,
            (EffectMode::Blur, _) => BackgroundEffect::Blur {
                strength: self.effect.blur_strength.clamp(0.0, 1.0),
            },
            (EffectMode::Color, _) | (EffectMode::Image, None) => BackgroundEffect::Color(color),
            (EffectMode::Image, Some(image)) => BackgroundEffect::Image(image),
            (EffectMode::Transparent, _) => BackgroundEffect::Transparent { fallback: color },
        };
        let mut settings = PipelineSettings::new(self.output_size()?);
        settings.orientation = self.orientation();
        settings.effect = effect;
        settings.segmentation = SegmentationSettings {
            method: match self.segmentation.method {
                MethodConfig::Rvm => SegmentationMethod::Rvm,
                MethodConfig::MediaPipe => SegmentationMethod::MediaPipe,
            },
            variant: parse_quality(&self.segmentation.quality)?,
            device: match self.segmentation.device {
                DeviceConfig::Gpu => InferenceDevice::DirectMl { adapter_index: 0 },
                DeviceConfig::Cpu => InferenceDevice::Cpu {
                    threads: CPU_INFERENCE_THREADS,
                },
            },
            mask: self.mask_params(),
        };
        Ok(settings)
    }

    pub fn default_path() -> Result<PathBuf> {
        let app_data = std::env::var_os("APPDATA").context("APPDATA is not set")?;
        Ok(PathBuf::from(app_data).join("ChromaFree").join("config.toml"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", temporary.display()))?;
        std::fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("chromafree-config-test-{}", std::process::id()))
            .join(name)
    }

    #[test]
    fn missing_file_gives_defaults_that_map_to_pipeline_settings() {
        let config = Config::load(&temp_path("missing.toml")).unwrap();
        assert_eq!(config, Config::default());
        let settings = config.pipeline_settings(None).unwrap();
        assert_eq!(settings.output, FrameSize::new(1280, 720).unwrap());
        assert_eq!(settings.effect, BackgroundEffect::Blur { strength: 0.5 });
        assert_eq!(settings.segmentation.variant, VariantPreference::Auto);
        assert_eq!(settings.segmentation.mask, None);
    }

    #[test]
    fn config_round_trips_through_disk() {
        let path = temp_path("round_trip.toml");
        let mut config = Config::default();
        config.camera.symbolic_link = Some(r"\\?\usb#vid_3564".to_owned());
        config.camera.format = Some(FormatConfig {
            width: 1920,
            height: 1080,
            fps_numerator: 30,
            fps_denominator: 1,
            encoding: EncodingConfig::Mjpeg,
        });
        config.effect.mode = EffectMode::Image;
        config.effect.image_path = Some(PathBuf::from(r"C:\background.png"));
        config.segmentation.method = MethodConfig::MediaPipe;
        config.segmentation.quality = "960x540".to_owned();
        config.segmentation.edge_low = Some(0.2);
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn partial_file_keeps_defaults_for_missing_fields() {
        let config: Config = toml::from_str("[effect]\nmode = \"transparent\"\ncolor = \"#0047BB\"\n").unwrap();
        assert_eq!(config.output, OutputConfig::default());
        let settings = config.pipeline_settings(None).unwrap();
        assert_eq!(
            settings.effect,
            BackgroundEffect::Transparent {
                fallback: Rgb { r: 0, g: 71, b: 187 }
            }
        );
    }

    #[test]
    fn values_map_to_core_types() {
        let mut config = Config::default();
        config.orientation.rotation = 270;
        config.orientation.mirror_horizontal = true;
        config.segmentation.device = DeviceConfig::Cpu;
        config.segmentation.quality = "1280x720".to_owned();
        config.segmentation.method = MethodConfig::MediaPipe;
        config.segmentation.edge_high = Some(0.9);
        config.effect.mode = EffectMode::Image;
        let settings = config.pipeline_settings(None).unwrap();
        assert_eq!(settings.orientation.rotation, Rotation::Clockwise270);
        assert!(settings.orientation.mirror_horizontal);
        assert!(matches!(settings.segmentation.device, InferenceDevice::Cpu { .. }));
        assert_eq!(
            settings.segmentation.variant,
            VariantPreference::Fixed(ModelVariant::new(1280, 720))
        );
        let mask = settings.segmentation.mask.unwrap();
        assert_eq!(mask.edge_high, 0.9);
        assert_eq!(mask.edge_low, MaskParams::MEMORYLESS_MODEL.edge_low);
        assert!(matches!(settings.effect, BackgroundEffect::Color(_)));
    }

    #[test]
    fn invalid_values_are_rejected() {
        assert!(parse_color("green").is_err());
        assert!(parse_quality("big").is_err());
        assert_eq!(format_color(parse_color("#00b140").unwrap()), GREEN_SCREEN);
        let mut config = Config::default();
        config.output.width = 1281;
        assert!(config.pipeline_settings(None).is_err());
    }
}
