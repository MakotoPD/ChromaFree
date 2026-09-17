use std::ptr::{NonNull, addr_of, addr_of_mut};
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};

use crate::error::IpcError;
use crate::layout::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    Nv12,
    Bgra,
}

impl PixelFormat {
    pub fn code(self) -> u32 {
        match self {
            Self::Nv12 => CHROMAFREE_FORMAT_NV12,
            Self::Bgra => CHROMAFREE_FORMAT_BGRA,
        }
    }

    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            CHROMAFREE_FORMAT_NV12 => Some(Self::Nv12),
            CHROMAFREE_FORMAT_BGRA => Some(Self::Bgra),
            _ => None,
        }
    }

    pub fn frame_size(self, width: u32, height: u32) -> usize {
        let pixels = width as usize * height as usize;
        match self {
            Self::Nv12 => pixels * 3 / 2,
            Self::Bgra => pixels * 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OutputMode {
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub size: usize,
    pub frame_number: u64,
    pub qpc: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsumerState {
    pub active: bool,
    pub format: Option<PixelFormat>,
    pub count: u32,
}

pub struct SharedRegion {
    header: NonNull<ChromaFreeFrameHeader>,
    data: NonNull<u8>,
    capacity: usize,
}

unsafe impl Send for SharedRegion {}
unsafe impl Sync for SharedRegion {}

fn validate_size(width: u32, height: u32) -> Result<(), IpcError> {
    let valid = width > 0
        && height > 0
        && width.is_multiple_of(2)
        && height.is_multiple_of(2)
        && width <= CHROMAFREE_MAX_WIDTH
        && height <= CHROMAFREE_MAX_HEIGHT;
    if valid {
        Ok(())
    } else {
        Err(IpcError::InvalidDescriptor { width, height })
    }
}

impl SharedRegion {
    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn from_raw(base: *mut u8, len: usize) -> Result<Self, IpcError> {
        let required = CHROMAFREE_HEADER_SIZE as usize + 1;
        if len < required {
            return Err(IpcError::RegionTooSmall { actual: len, required });
        }
        let header = NonNull::new(base.cast::<ChromaFreeFrameHeader>())
            .ok_or(IpcError::RegionTooSmall { actual: 0, required })?;
        let data = NonNull::new(unsafe { base.add(CHROMAFREE_HEADER_SIZE as usize) })
            .ok_or(IpcError::RegionTooSmall { actual: 0, required })?;
        Ok(Self {
            header,
            data,
            capacity: len - CHROMAFREE_HEADER_SIZE as usize,
        })
    }

    fn raw(&self) -> *mut ChromaFreeFrameHeader {
        self.header.as_ptr()
    }

    fn atomic_u32(&self, field: *mut u32) -> &AtomicU32 {
        unsafe { AtomicU32::from_ptr(field) }
    }

    fn atomic_i64(&self, field: *mut i64) -> &AtomicI64 {
        unsafe { AtomicI64::from_ptr(field) }
    }

    fn sequence(&self) -> &AtomicU64 {
        unsafe { AtomicU64::from_ptr(addr_of_mut!((*self.raw()).sequence)) }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn initialize(&self, qpc_frequency: i64) {
        let header = self.raw();
        unsafe {
            addr_of_mut!((*header).magic).write_volatile(CHROMAFREE_MAGIC);
            addr_of_mut!((*header).version).write_volatile(CHROMAFREE_PROTOCOL_VERSION);
            addr_of_mut!((*header).header_size).write_volatile(CHROMAFREE_HEADER_SIZE);
            addr_of_mut!((*header).capacity).write_volatile(self.capacity as u32);
            addr_of_mut!((*header).qpc_frequency).write_volatile(qpc_frequency);
        }
    }

    pub fn validate(&self) -> Result<(), IpcError> {
        let header = self.raw();
        let (magic, version) = unsafe {
            (
                addr_of!((*header).magic).read_volatile(),
                addr_of!((*header).version).read_volatile(),
            )
        };
        if magic == CHROMAFREE_MAGIC && version == CHROMAFREE_PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(IpcError::IncompatibleProtocol {
                magic,
                version,
                expected_magic: CHROMAFREE_MAGIC,
                expected_version: CHROMAFREE_PROTOCOL_VERSION,
            })
        }
    }

    pub fn set_output_mode(&self, mode: OutputMode) -> Result<(), IpcError> {
        validate_size(mode.width, mode.height)?;
        let header = self.raw();
        unsafe {
            addr_of_mut!((*header).output_width).write_volatile(mode.width);
            addr_of_mut!((*header).output_height).write_volatile(mode.height);
            addr_of_mut!((*header).output_fps_numerator).write_volatile(mode.fps_numerator.max(1));
            addr_of_mut!((*header).output_fps_denominator).write_volatile(mode.fps_denominator.max(1));
        }
        Ok(())
    }

    pub fn output_mode(&self) -> Option<OutputMode> {
        let header = self.raw();
        let mode = unsafe {
            OutputMode {
                width: addr_of!((*header).output_width).read_volatile(),
                height: addr_of!((*header).output_height).read_volatile(),
                fps_numerator: addr_of!((*header).output_fps_numerator).read_volatile(),
                fps_denominator: addr_of!((*header).output_fps_denominator).read_volatile(),
            }
        };
        let valid =
            validate_size(mode.width, mode.height).is_ok() && mode.fps_numerator > 0 && mode.fps_denominator > 0;
        valid.then_some(mode)
    }

    pub fn write_frame(
        &self,
        format: PixelFormat,
        width: u32,
        height: u32,
        frame_number: u64,
        qpc: i64,
        pixels: &[u8],
    ) -> Result<(), IpcError> {
        self.write_frame_parts(format, width, height, frame_number, qpc, &[pixels])
    }

    pub fn write_frame_parts(
        &self,
        format: PixelFormat,
        width: u32,
        height: u32,
        frame_number: u64,
        qpc: i64,
        parts: &[&[u8]],
    ) -> Result<(), IpcError> {
        validate_size(width, height)?;
        let size = format.frame_size(width, height);
        if size > self.capacity {
            return Err(IpcError::FrameTooLarge {
                size,
                capacity: self.capacity,
            });
        }
        let provided: usize = parts.iter().map(|part| part.len()).sum();
        if provided < size {
            return Err(IpcError::BufferTooSmall {
                needed: size,
                actual: provided,
            });
        }
        let header = self.raw();
        let sequence = self.sequence();
        let writing = sequence.load(Ordering::Acquire) | 1;
        sequence.store(writing, Ordering::Release);
        unsafe {
            let mut offset = 0;
            for part in parts {
                let count = part.len().min(size - offset);
                std::ptr::copy_nonoverlapping(part.as_ptr(), self.data.as_ptr().add(offset), count);
                offset += count;
            }
            addr_of_mut!((*header).frame_width).write_volatile(width);
            addr_of_mut!((*header).frame_height).write_volatile(height);
            addr_of_mut!((*header).frame_format).write_volatile(format.code());
            addr_of_mut!((*header).frame_size).write_volatile(size as u32);
            addr_of_mut!((*header).frame_number).write_volatile(frame_number);
            addr_of_mut!((*header).frame_qpc).write_volatile(qpc);
        }
        sequence.store(writing.wrapping_add(1), Ordering::Release);
        self.atomic_i64(unsafe { addr_of_mut!((*header).producer_heartbeat_qpc) })
            .store(qpc, Ordering::Relaxed);
        Ok(())
    }

    pub fn read_frame(&self, out: &mut [u8]) -> Result<Option<FrameInfo>, IpcError> {
        let header = self.raw();
        let sequence = self.sequence();
        for _ in 0..CHROMAFREE_SEQLOCK_ATTEMPTS {
            let before = sequence.load(Ordering::Acquire);
            if before == 0 {
                return Ok(None);
            }
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let (width, height, code, size, frame_number, qpc) = unsafe {
                (
                    addr_of!((*header).frame_width).read_volatile(),
                    addr_of!((*header).frame_height).read_volatile(),
                    addr_of!((*header).frame_format).read_volatile(),
                    addr_of!((*header).frame_size).read_volatile() as usize,
                    addr_of!((*header).frame_number).read_volatile(),
                    addr_of!((*header).frame_qpc).read_volatile(),
                )
            };
            let format = PixelFormat::from_code(code);
            let consistent = format.is_some_and(|f| f.frame_size(width, height) == size) && size <= self.capacity;
            if consistent && out.len() >= size {
                unsafe { std::ptr::copy_nonoverlapping(self.data.as_ptr(), out.as_mut_ptr(), size) };
            }
            if sequence.load(Ordering::Acquire) != before {
                continue;
            }
            let Some(format) = format.filter(|_| consistent) else {
                return Ok(None);
            };
            if out.len() < size {
                return Err(IpcError::BufferTooSmall {
                    needed: size,
                    actual: out.len(),
                });
            }
            return Ok(Some(FrameInfo {
                width,
                height,
                format,
                size,
                frame_number,
                qpc,
            }));
        }
        Err(IpcError::Contended)
    }

    pub fn set_producer_active(&self, active: bool, qpc: i64) {
        let header = self.raw();
        let flags = self.atomic_u32(unsafe { addr_of_mut!((*header).producer_flags) });
        if active {
            flags.fetch_or(CHROMAFREE_PRODUCER_ACTIVE, Ordering::AcqRel);
        } else {
            flags.fetch_and(!CHROMAFREE_PRODUCER_ACTIVE, Ordering::AcqRel);
        }
        self.atomic_i64(unsafe { addr_of_mut!((*header).producer_heartbeat_qpc) })
            .store(qpc, Ordering::Relaxed);
    }

    pub fn producer_alive(&self, now_qpc: i64) -> bool {
        let header = self.raw();
        let flags = self
            .atomic_u32(unsafe { addr_of_mut!((*header).producer_flags) })
            .load(Ordering::Acquire);
        let heartbeat = self
            .atomic_i64(unsafe { addr_of_mut!((*header).producer_heartbeat_qpc) })
            .load(Ordering::Relaxed);
        flags & CHROMAFREE_PRODUCER_ACTIVE != 0 && self.fresh(heartbeat, now_qpc)
    }

    fn fresh(&self, heartbeat: i64, now_qpc: i64) -> bool {
        let frequency = unsafe { addr_of!((*self.raw()).qpc_frequency).read_volatile() }.max(1);
        let timeout = frequency * i64::from(CHROMAFREE_HEARTBEAT_TIMEOUT_MS) / 1000;
        heartbeat != 0 && now_qpc.saturating_sub(heartbeat) <= timeout
    }

    pub fn consumer_started(&self, format: PixelFormat, qpc: i64) {
        let header = self.raw();
        self.atomic_u32(unsafe { addr_of_mut!((*header).consumer_format) })
            .store(format.code(), Ordering::Release);
        self.atomic_u32(unsafe { addr_of_mut!((*header).consumer_count) })
            .fetch_add(1, Ordering::AcqRel);
        self.atomic_u32(unsafe { addr_of_mut!((*header).consumer_flags) })
            .fetch_or(CHROMAFREE_CONSUMER_ACTIVE, Ordering::AcqRel);
        self.consumer_heartbeat(qpc);
    }

    pub fn consumer_stopped(&self) {
        let header = self.raw();
        let count = self.atomic_u32(unsafe { addr_of_mut!((*header).consumer_count) });
        let previous = count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| Some(c.saturating_sub(1)))
            .unwrap_or(0);
        if previous <= 1 {
            self.atomic_u32(unsafe { addr_of_mut!((*header).consumer_flags) })
                .fetch_and(!CHROMAFREE_CONSUMER_ACTIVE, Ordering::AcqRel);
        }
    }

    pub fn consumer_heartbeat(&self, qpc: i64) {
        self.atomic_i64(unsafe { addr_of_mut!((*self.raw()).consumer_heartbeat_qpc) })
            .store(qpc, Ordering::Relaxed);
    }

    pub fn consumer_state(&self, now_qpc: i64) -> ConsumerState {
        let header = self.raw();
        let flags = self
            .atomic_u32(unsafe { addr_of_mut!((*header).consumer_flags) })
            .load(Ordering::Acquire);
        let count = self
            .atomic_u32(unsafe { addr_of_mut!((*header).consumer_count) })
            .load(Ordering::Acquire);
        let heartbeat = self
            .atomic_i64(unsafe { addr_of_mut!((*header).consumer_heartbeat_qpc) })
            .load(Ordering::Relaxed);
        let format = PixelFormat::from_code(
            self.atomic_u32(unsafe { addr_of_mut!((*header).consumer_format) })
                .load(Ordering::Acquire),
        );
        let active = flags & CHROMAFREE_CONSUMER_ACTIVE != 0 && count > 0 && self.fresh(heartbeat, now_qpc);
        ConsumerState {
            active,
            format: format.filter(|_| active),
            count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(capacity: usize) -> (Vec<u64>, SharedRegion) {
        let words = (CHROMAFREE_HEADER_SIZE as usize + capacity).div_ceil(8);
        let mut memory = vec![0u64; words];
        let region = unsafe { SharedRegion::from_raw(memory.as_mut_ptr().cast::<u8>(), words * 8) }.unwrap();
        region.initialize(1_000_000);
        (memory, region)
    }

    #[test]
    fn empty_region_has_no_frame() {
        let (_memory, region) = region(64);
        region.validate().unwrap();
        assert_eq!(region.read_frame(&mut [0; 64]).unwrap(), None);
        assert_eq!(region.output_mode(), None);
    }

    #[test]
    fn written_frame_round_trips() {
        let (_memory, region) = region(64);
        let pixels: Vec<u8> = (0..24).collect();
        region.write_frame(PixelFormat::Nv12, 4, 4, 7, 1234, &pixels).unwrap();
        let mut out = [0u8; 64];
        let info = region.read_frame(&mut out).unwrap().unwrap();
        assert_eq!(info.size, 24);
        assert_eq!(
            (info.width, info.height, info.format, info.frame_number, info.qpc),
            (4, 4, PixelFormat::Nv12, 7, 1234)
        );
        assert_eq!(&out[..24], &pixels[..]);
    }

    #[test]
    fn oversized_frames_and_small_buffers_are_rejected() {
        let (_memory, region) = region(16);
        assert!(matches!(
            region.write_frame(PixelFormat::Bgra, 4, 4, 0, 0, &[0; 64]),
            Err(IpcError::FrameTooLarge { .. })
        ));
        region.write_frame(PixelFormat::Nv12, 2, 2, 1, 0, &[1; 6]).unwrap();
        assert!(matches!(
            region.read_frame(&mut [0; 4]),
            Err(IpcError::BufferTooSmall { needed: 6, .. })
        ));
        assert!(matches!(
            region.write_frame(PixelFormat::Nv12, 3, 2, 0, 0, &[0; 9]),
            Err(IpcError::InvalidDescriptor { .. })
        ));
    }

    #[test]
    fn output_mode_is_stored_separately_from_frames() {
        let (_memory, region) = region(64);
        let mode = OutputMode {
            width: 1280,
            height: 720,
            fps_numerator: 30,
            fps_denominator: 1,
        };
        region.set_output_mode(mode).unwrap();
        assert_eq!(region.output_mode(), Some(mode));
    }

    #[test]
    fn consumer_and_producer_liveness_follow_flags_and_heartbeats() {
        let (_memory, region) = region(64);
        assert!(!region.consumer_state(0).active);
        region.consumer_started(PixelFormat::Bgra, 5_000_000);
        region.consumer_started(PixelFormat::Nv12, 5_000_000);
        let state = region.consumer_state(5_500_000);
        assert!(state.active);
        assert_eq!((state.count, state.format), (2, Some(PixelFormat::Nv12)));
        assert!(!region.consumer_state(8_000_000).active);
        region.consumer_stopped();
        assert!(region.consumer_state(5_500_000).active);
        region.consumer_stopped();
        assert!(!region.consumer_state(5_500_000).active);
        region.consumer_stopped();
        assert_eq!(region.consumer_state(5_500_000).count, 0);

        region.set_producer_active(true, 10_000_000);
        assert!(region.producer_alive(11_000_000));
        assert!(!region.producer_alive(13_000_000));
        region.set_producer_active(false, 11_000_000);
        assert!(!region.producer_alive(11_000_000));
    }

    #[test]
    fn incompatible_memory_is_detected() {
        let mut memory = vec![0u64; 32];
        let region = unsafe { SharedRegion::from_raw(memory.as_mut_ptr().cast::<u8>(), 256) }.unwrap();
        assert!(matches!(region.validate(), Err(IpcError::IncompatibleProtocol { .. })));
    }
}
