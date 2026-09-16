pub const BGCAM_MAGIC: u32 = 0x4D41_4342;
pub const BGCAM_PROTOCOL_VERSION: u32 = 1;
pub const BGCAM_HEADER_SIZE: u32 = 128;
pub const BGCAM_MAX_WIDTH: u32 = 3840;
pub const BGCAM_MAX_HEIGHT: u32 = 2160;
pub const BGCAM_FRAME_CAPACITY: u32 = BGCAM_MAX_WIDTH * BGCAM_MAX_HEIGHT * 4;
pub const BGCAM_SECTION_SIZE: u32 = BGCAM_HEADER_SIZE + BGCAM_FRAME_CAPACITY;

pub const BGCAM_FORMAT_NONE: u32 = 0;
pub const BGCAM_FORMAT_NV12: u32 = 1;
pub const BGCAM_FORMAT_BGRA: u32 = 2;

pub const BGCAM_PRODUCER_ACTIVE: u32 = 1;
pub const BGCAM_CONSUMER_ACTIVE: u32 = 1;

pub const BGCAM_HEARTBEAT_TIMEOUT_MS: u32 = 2000;
pub const BGCAM_SEQLOCK_ATTEMPTS: u32 = 64;

pub const BGCAM_SECTION_NAME: &str = "bgcam-frames-v1";
pub const BGCAM_FRAME_READY_EVENT_NAME: &str = "bgcam-frame-ready-v1";
pub const BGCAM_CONSUMER_CHANGED_EVENT_NAME: &str = "bgcam-consumer-changed-v1";
pub const BGCAM_OBJECT_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;LS)(A;;GA;;;IU)";

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct BgcamFrameHeader {
    pub magic: u32,
    pub version: u32,
    pub header_size: u32,
    pub capacity: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub output_fps_numerator: u32,
    pub output_fps_denominator: u32,
    pub frame_width: u32,
    pub frame_height: u32,
    pub frame_format: u32,
    pub frame_size: u32,
    pub producer_flags: u32,
    pub consumer_flags: u32,
    pub consumer_format: u32,
    pub consumer_count: u32,
    pub sequence: u64,
    pub frame_number: u64,
    pub frame_qpc: i64,
    pub producer_heartbeat_qpc: i64,
    pub consumer_heartbeat_qpc: i64,
    pub qpc_frequency: i64,
    pub reserved: [u32; 4],
}
