use anyhow::{Result, anyhow};
use chromafree_capture::{
    CameraDevice, CameraReader, CaptureFormat, Encoding, FrameSource, camera_formats, list_cameras,
};

use crate::config::CameraConfig;

const AUTO_MAX_PIXELS: u32 = 1920 * 1080;

pub struct OpenedCamera {
    pub source: Box<dyn FrameSource>,
    pub name: String,
    pub format: CaptureFormat,
}

#[derive(Debug, thiserror::Error)]
pub enum CameraOpenError {
    #[error("no camera is connected")]
    NoCamera,
    #[error("camera {0} is not connected")]
    Missing(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub trait CameraProvider: Send + 'static {
    fn open(&mut self, camera: &CameraConfig, output_fps: u32) -> Result<OpenedCamera, CameraOpenError>;
}

fn encoding_preference(encoding: Encoding) -> u8 {
    match encoding {
        Encoding::Nv12 => 3,
        Encoding::Mjpeg => 2,
        Encoding::Yuy2 => 1,
        Encoding::Other(_) => 0,
    }
}

pub fn rank_formats(formats: &[CaptureFormat], output_fps: u32) -> Vec<CaptureFormat> {
    let mut ranked: Vec<CaptureFormat> = formats.iter().filter(|format| format.is_decodable()).copied().collect();
    let key = |format: &CaptureFormat| {
        let fast_enough = format.fps() + 0.5 >= f64::from(output_fps);
        let fits = format.width * format.height <= AUTO_MAX_PIXELS;
        (
            fast_enough && fits,
            format.width * format.height,
            encoding_preference(format.encoding),
            std::cmp::Reverse(format.fps_numerator / format.fps_denominator.max(1)),
        )
    };
    ranked.sort_by_key(|format| std::cmp::Reverse(key(format)));
    ranked
}

pub struct MediaFoundationCameras;

impl MediaFoundationCameras {
    fn find(camera: &CameraConfig) -> Result<CameraDevice, CameraOpenError> {
        let cameras = list_cameras().map_err(anyhow::Error::from)?;
        match &camera.symbolic_link {
            Some(link) => cameras
                .into_iter()
                .find(|device| &device.symbolic_link == link)
                .ok_or_else(|| CameraOpenError::Missing(camera.name.clone().unwrap_or_else(|| link.clone()))),
            None => cameras.into_iter().next().ok_or(CameraOpenError::NoCamera),
        }
    }
}

impl CameraProvider for MediaFoundationCameras {
    fn open(&mut self, camera: &CameraConfig, output_fps: u32) -> Result<OpenedCamera, CameraOpenError> {
        let device = Self::find(camera)?;
        let formats = camera_formats(&device.symbolic_link).map_err(anyhow::Error::from)?;
        let candidates = match camera.format.map(|format| format.to_capture()) {
            Some(format) if formats.contains(&format) => vec![format],
            _ => rank_formats(&formats, output_fps),
        };
        let mut last_error = anyhow!("camera {} offers no usable format", device.name);
        for format in candidates {
            let started = CameraReader::open(&device.symbolic_link, &format).and_then(|mut reader| {
                reader.read()?;
                Ok(reader)
            });
            match started {
                Ok(reader) => {
                    tracing::info!(camera = %device.name, %format, "camera opened");
                    return Ok(OpenedCamera {
                        source: Box::new(reader),
                        name: device.name,
                        format,
                    });
                }
                Err(error) => {
                    tracing::warn!(camera = %device.name, %format, %error, "camera format failed to start");
                    last_error = error.into();
                }
            }
        }
        Err(CameraOpenError::Other(last_error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(width: u32, height: u32, fps: u32, encoding: Encoding) -> CaptureFormat {
        CaptureFormat {
            width,
            height,
            fps_numerator: fps,
            fps_denominator: 1,
            encoding,
        }
    }

    #[test]
    fn formats_are_ranked_by_size_then_native_encoding_then_lowest_sufficient_fps() {
        let formats = [
            format(3840, 2160, 30, Encoding::Mjpeg),
            format(1920, 1080, 15, Encoding::Nv12),
            format(1920, 1080, 30, Encoding::Mjpeg),
            format(1920, 1080, 60, Encoding::Nv12),
            format(1920, 1080, 30, Encoding::Nv12),
            format(1280, 720, 60, Encoding::Nv12),
            format(640, 480, 30, Encoding::Other(1)),
        ];
        let ranked = rank_formats(&formats, 30);
        assert_eq!(&ranked[..3], &[formats[4], formats[3], formats[2]]);
        assert_eq!(ranked.len(), 6);
        assert_eq!(rank_formats(&formats, 60)[0], formats[3]);
        assert_eq!(
            rank_formats(&[format(640, 480, 15, Encoding::Yuy2)], 30),
            vec![format(640, 480, 15, Encoding::Yuy2)]
        );
    }
}
