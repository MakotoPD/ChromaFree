use std::time::Duration;

use chromafree_core::{FrameSize, Nv12Frame};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFMediaBuffer, IMFSample, MF_LOW_LATENCY, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MF_VERSION,
    MFCreateAttributes, MFCreateMediaType, MFGetSystemTime, MFMediaType_Video, MFSTARTUP_FULL, MFShutdown, MFStartup,
    MFVideoFormat_NV12,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::Interface;

use crate::device::{CaptureFormat, Encoding, OpenedSource, native_types, open_source};
use crate::error::CaptureError;
use crate::source::FrameSource;

pub struct MediaFoundation(());

impl MediaFoundation {
    pub fn startup() -> Result<Self, CaptureError> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
        if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
            unsafe { CoUninitialize() };
            return Err(error.into());
        }
        Ok(Self(()))
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
        unsafe { CoUninitialize() };
    }
}

pub struct CameraReader {
    opened: OpenedSource,
    format: CaptureFormat,
    frame: Nv12Frame,
    timestamp: Option<i64>,
}

const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

impl CameraReader {
    pub fn open(symbolic_link: &str, format: &CaptureFormat) -> Result<Self, CaptureError> {
        let size = FrameSize::new(format.width, format.height)?;
        let mut attributes = None;
        unsafe { MFCreateAttributes(&mut attributes, 3) }?;
        let attributes = attributes.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
        unsafe {
            attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            attributes.SetUINT32(&MF_LOW_LATENCY, 1)?;
        }
        let opened = open_source(symbolic_link, Some(&attributes))?;

        let (index, _) = native_types(&opened.reader)?
            .into_iter()
            .find(|(_, candidate)| candidate == format)
            .ok_or_else(|| CaptureError::FormatNotAvailable(format.to_string()))?;
        let native = unsafe { opened.reader.GetNativeMediaType(STREAM, index) }?;
        unsafe { opened.reader.SetCurrentMediaType(STREAM, None, &native) }?;

        if format.encoding != Encoding::Nv12 {
            let output = unsafe { MFCreateMediaType() }?;
            unsafe {
                output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
                output.SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    (u64::from(format.width) << 32) | u64::from(format.height),
                )?;
                opened.reader.SetCurrentMediaType(STREAM, None, &output)?;
            }
        }
        unsafe { opened.reader.SetStreamSelection(STREAM, true) }?;

        Ok(Self {
            opened,
            format: *format,
            frame: Nv12Frame::new(size),
            timestamp: None,
        })
    }

    pub fn format(&self) -> &CaptureFormat {
        &self.format
    }

    fn copy_sample(&mut self, sample: &IMFSample) -> Result<(), CaptureError> {
        let buffer: IMFMediaBuffer = if unsafe { sample.GetBufferCount() }? == 1 {
            unsafe { sample.GetBufferByIndex(0) }?
        } else {
            unsafe { sample.ConvertToContiguousBuffer() }?
        };
        let size = self.frame.size();
        let width = size.width() as usize;
        let (luma_rows, chroma_rows) = (size.height() as usize, size.chroma_height() as usize);
        let (luma, chroma) = self.frame.planes_mut();

        if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer>() {
            let mut scanline0 = std::ptr::null_mut();
            let mut pitch = 0;
            unsafe { buffer2d.Lock2D(&mut scanline0, &mut pitch) }?;
            let pitch = pitch as isize;
            unsafe {
                for (row, target) in luma.chunks_exact_mut(width).enumerate() {
                    let line = scanline0.offset(pitch * row as isize);
                    std::ptr::copy_nonoverlapping(line, target.as_mut_ptr(), width);
                }
                for (row, target) in chroma.chunks_exact_mut(width).enumerate() {
                    let line = scanline0.offset(pitch * (luma_rows + row) as isize);
                    std::ptr::copy_nonoverlapping(line, target.as_mut_ptr(), width);
                }
                buffer2d.Unlock2D()?;
            }
            return Ok(());
        }

        let mut data = std::ptr::null_mut();
        let mut length = 0;
        unsafe { buffer.Lock(&mut data, None, Some(&mut length)) }?;
        let expected = width * (luma_rows + chroma_rows);
        let result = if (length as usize) < expected {
            Err(CaptureError::ShortBuffer {
                expected,
                actual: length as usize,
            })
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(data, expected) };
            luma.copy_from_slice(&bytes[..luma.len()]);
            chroma.copy_from_slice(&bytes[luma.len()..]);
            Ok(())
        };
        unsafe { buffer.Unlock() }?;
        result
    }
}

impl FrameSource for CameraReader {
    fn size(&self) -> FrameSize {
        self.frame.size()
    }

    fn read(&mut self) -> Result<&Nv12Frame, CaptureError> {
        loop {
            let mut flags = 0;
            let mut sample = None;
            let mut timestamp = 0;
            unsafe {
                self.opened.reader.ReadSample(
                    STREAM,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )
            }?;
            if flags & (MF_SOURCE_READERF_ERROR.0 | MF_SOURCE_READERF_ENDOFSTREAM.0) as u32 != 0 {
                return Err(CaptureError::StreamEnded);
            }
            if let Some(sample) = sample {
                self.copy_sample(&sample)?;
                self.timestamp = Some(timestamp);
                return Ok(&self.frame);
            }
        }
    }

    fn capture_age(&self) -> Option<Duration> {
        let age = unsafe { MFGetSystemTime() } - self.timestamp?;
        u64::try_from(age).ok().map(|ticks| Duration::from_nanos(ticks * 100))
    }
}
