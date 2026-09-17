use std::fmt;

use chromafree_ipc::{VIRTUAL_CAMERA_CLSID, VIRTUAL_CAMERA_NAME};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFMediaSource, IMFMediaType, IMFSourceReader, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_E_NO_MORE_TYPES, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_SUBTYPE, MF_SOURCE_READER_FIRST_VIDEO_STREAM, MFCreateAttributes, MFCreateDeviceSource,
    MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFVideoFormat_MJPG, MFVideoFormat_NV12,
    MFVideoFormat_YUY2,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::{GUID, HSTRING, PWSTR};

use crate::error::CaptureError;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CameraDevice {
    pub name: String,
    pub symbolic_link: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Encoding {
    Nv12,
    Yuy2,
    Mjpeg,
    Other(u32),
}

impl Encoding {
    pub(crate) fn from_subtype(subtype: GUID) -> Self {
        match subtype {
            s if s == MFVideoFormat_NV12 => Self::Nv12,
            s if s == MFVideoFormat_YUY2 => Self::Yuy2,
            s if s == MFVideoFormat_MJPG => Self::Mjpeg,
            s => Self::Other(s.data1),
        }
    }
}

impl fmt::Display for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nv12 => f.write_str("NV12"),
            Self::Yuy2 => f.write_str("YUY2"),
            Self::Mjpeg => f.write_str("MJPEG"),
            Self::Other(fourcc) => {
                let text: String = fourcc
                    .to_le_bytes()
                    .iter()
                    .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
                    .collect();
                f.write_str(&text)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CaptureFormat {
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub encoding: Encoding,
}

impl CaptureFormat {
    pub fn fps(&self) -> f64 {
        f64::from(self.fps_numerator) / f64::from(self.fps_denominator.max(1))
    }

    pub(crate) fn from_media_type(media_type: &IMFMediaType) -> Result<Self, CaptureError> {
        let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }?;
        let size = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }?;
        let rate = unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) }.unwrap_or((30 << 32) | 1);
        Ok(Self {
            width: (size >> 32) as u32,
            height: size as u32,
            fps_numerator: (rate >> 32) as u32,
            fps_denominator: (rate as u32).max(1),
            encoding: Encoding::from_subtype(subtype),
        })
    }

    pub fn is_decodable(&self) -> bool {
        !matches!(self.encoding, Encoding::Other(_))
    }
}

impl fmt::Display for CaptureFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{} @ {:.2} fps {}",
            self.width,
            self.height,
            self.fps(),
            self.encoding
        )
    }
}

pub fn is_virtual_chromafree_camera(name: &str, symbolic_link: &str) -> bool {
    let clsid = VIRTUAL_CAMERA_CLSID.trim_matches(['{', '}']).to_ascii_lowercase();
    name == VIRTUAL_CAMERA_NAME || symbolic_link.to_ascii_lowercase().contains(&clsid)
}

fn capture_attributes(symbolic_link: Option<&str>) -> Result<IMFAttributes, CaptureError> {
    let mut attributes = None;
    unsafe { MFCreateAttributes(&mut attributes, 2) }?;
    let attributes = attributes.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
    unsafe {
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )
    }?;
    if let Some(link) = symbolic_link {
        unsafe {
            attributes.SetString(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                &HSTRING::from(link),
            )
        }?;
    }
    Ok(attributes)
}

fn string_attribute(activate: &IMFActivate, key: &GUID) -> Result<String, CaptureError> {
    let mut value = PWSTR::null();
    let mut length = 0;
    unsafe { activate.GetAllocatedString(key, &mut value, &mut length) }?;
    let text = unsafe { value.to_string() };
    unsafe { CoTaskMemFree(Some(value.0.cast())) };
    Ok(text.unwrap_or_default())
}

pub fn list_cameras() -> Result<Vec<CameraDevice>, CaptureError> {
    Ok(enumerate_devices()?
        .into_iter()
        .filter(|device| !is_virtual_chromafree_camera(&device.name, &device.symbolic_link))
        .collect())
}

pub fn chromafree_camera_present() -> Result<bool, CaptureError> {
    Ok(enumerate_devices()?
        .iter()
        .any(|device| is_virtual_chromafree_camera(&device.name, &device.symbolic_link)))
}

fn enumerate_devices() -> Result<Vec<CameraDevice>, CaptureError> {
    let attributes = capture_attributes(None)?;
    let mut activates = std::ptr::null_mut();
    let mut count = 0;
    unsafe { MFEnumDeviceSources(&attributes, &mut activates, &mut count) }?;
    if activates.is_null() {
        return Ok(Vec::new());
    }
    let taken: Vec<Option<IMFActivate>> = unsafe { std::slice::from_raw_parts_mut(activates, count as usize) }
        .iter_mut()
        .map(Option::take)
        .collect();
    unsafe { CoTaskMemFree(Some(activates.cast())) };

    let mut cameras = Vec::with_capacity(taken.len());
    for activate in taken.into_iter().flatten() {
        let name = string_attribute(&activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)?;
        let symbolic_link = string_attribute(&activate, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK)?;
        cameras.push(CameraDevice { name, symbolic_link });
    }
    Ok(cameras)
}

pub(crate) struct OpenedSource {
    pub source: IMFMediaSource,
    pub reader: IMFSourceReader,
}

impl Drop for OpenedSource {
    fn drop(&mut self) {
        let _ = unsafe { self.source.Shutdown() };
    }
}

pub(crate) fn open_source(
    symbolic_link: &str,
    reader_attributes: Option<&IMFAttributes>,
) -> Result<OpenedSource, CaptureError> {
    let source = unsafe { MFCreateDeviceSource(&capture_attributes(Some(symbolic_link))?) }
        .map_err(|_| CaptureError::CameraNotFound(symbolic_link.to_owned()))?;
    let reader = unsafe { MFCreateSourceReaderFromMediaSource(&source, reader_attributes) };
    match reader {
        Ok(reader) => Ok(OpenedSource { source, reader }),
        Err(error) => {
            let _ = unsafe { source.Shutdown() };
            Err(error.into())
        }
    }
}

pub(crate) fn native_types(reader: &IMFSourceReader) -> Result<Vec<(u32, CaptureFormat)>, CaptureError> {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut types = Vec::new();
    for index in 0.. {
        match unsafe { reader.GetNativeMediaType(stream, index) } {
            Ok(media_type) => types.push((index, CaptureFormat::from_media_type(&media_type)?)),
            Err(error) if error.code() == MF_E_NO_MORE_TYPES => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(types)
}

pub fn camera_formats(symbolic_link: &str) -> Result<Vec<CaptureFormat>, CaptureError> {
    let opened = open_source(symbolic_link, None)?;
    let mut formats: Vec<CaptureFormat> = native_types(&opened.reader)?
        .into_iter()
        .map(|(_, format)| format)
        .filter(CaptureFormat::is_decodable)
        .collect();
    formats.sort_by(|a, b| {
        (b.width * b.height)
            .cmp(&(a.width * a.height))
            .then(b.fps().total_cmp(&a.fps()))
            .then(encoding_rank(a.encoding).cmp(&encoding_rank(b.encoding)))
    });
    formats.dedup();
    Ok(formats)
}

fn encoding_rank(encoding: Encoding) -> u8 {
    match encoding {
        Encoding::Nv12 => 0,
        Encoding::Mjpeg => 1,
        Encoding::Yuy2 => 2,
        Encoding::Other(_) => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_camera_is_recognised_by_name_or_clsid() {
        assert!(is_virtual_chromafree_camera(
            "ChromaFree",
            r"\\?\SWD#VCAMDEVAPI#something"
        ));
        assert!(is_virtual_chromafree_camera(
            "renamed",
            r"\\?\SWD#VCAMDEVAPI#{4525794b-703e-444b-a81d-2b278b2ad0e4}#x"
        ));
        assert!(!is_virtual_chromafree_camera(
            "USB Camera",
            r"\\?\USB#VID_046D&PID_085C#1"
        ));
    }

    #[test]
    fn encodings_are_named_after_fourcc() {
        assert_eq!(Encoding::from_subtype(MFVideoFormat_MJPG).to_string(), "MJPEG");
        assert_eq!(Encoding::Other(u32::from_le_bytes(*b"H264")).to_string(), "H264");
    }
}
