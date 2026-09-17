use std::sync::Arc;
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForSingleObject,
};
use windows::core::HSTRING;

use crate::error::IpcError;
use crate::layout::*;
use crate::region::{ConsumerState, FrameInfo, OutputMode, PixelFormat, SharedRegion};

pub fn qpc_now() -> i64 {
    let mut value = 0;
    let _ = unsafe { QueryPerformanceCounter(&mut value) };
    value
}

pub fn qpc_frequency() -> i64 {
    let mut value = 1;
    let _ = unsafe { QueryPerformanceFrequency(&mut value) };
    value
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectNames {
    pub section: String,
    pub frame_ready: String,
    pub consumer_changed: String,
}

impl ObjectNames {
    pub fn with_prefix(prefix: &str) -> Self {
        Self {
            section: format!("{prefix}{CHROMAFREE_SECTION_NAME}"),
            frame_ready: format!("{prefix}{CHROMAFREE_FRAME_READY_EVENT_NAME}"),
            consumer_changed: format!("{prefix}{CHROMAFREE_CONSUMER_CHANGED_EVENT_NAME}"),
        }
    }

    pub fn local() -> Self {
        Self::with_prefix("Local\\")
    }

    pub fn for_session(session_id: u32) -> Self {
        Self::with_prefix(&format!("Session\\{session_id}\\"))
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

struct MappedView(MEMORY_MAPPED_VIEW_ADDRESS);

impl Drop for MappedView {
    fn drop(&mut self) {
        let _ = unsafe { UnmapViewOfFile(self.0) };
    }
}

unsafe impl Send for MappedView {}
unsafe impl Sync for MappedView {}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

fn map(section: &OwnedHandle) -> Result<(MappedView, SharedRegion), IpcError> {
    let view = unsafe {
        MapViewOfFile(
            section.0,
            FILE_MAP_READ | FILE_MAP_WRITE,
            0,
            0,
            CHROMAFREE_SECTION_SIZE as usize,
        )
    };
    if view.Value.is_null() {
        return Err(windows::core::Error::from_thread().into());
    }
    let view = MappedView(view);
    let region = unsafe { SharedRegion::from_raw(view.0.Value.cast::<u8>(), CHROMAFREE_SECTION_SIZE as usize) }?;
    Ok((view, region))
}

fn wait(handle: &OwnedHandle, timeout: Duration) -> bool {
    let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    let result = unsafe { WaitForSingleObject(handle.0, millis) };
    result == WAIT_OBJECT_0
}

#[derive(Clone)]
pub struct ProducerWaker(Arc<OwnedHandle>);

impl ProducerWaker {
    pub fn wake(&self) {
        let _ = unsafe { SetEvent(self.0.0) };
    }
}

pub struct ProducerChannel {
    _section: OwnedHandle,
    _view: MappedView,
    region: SharedRegion,
    frame_ready: OwnedHandle,
    consumer_changed: Arc<OwnedHandle>,
    frame_number: u64,
}

impl ProducerChannel {
    pub fn create(names: &ObjectNames) -> Result<Self, IpcError> {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from(CHROMAFREE_OBJECT_SDDL),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }?;
        let descriptor = SecurityDescriptor(descriptor);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0.0,
            bInheritHandle: false.into(),
        };
        let section = OwnedHandle(unsafe {
            CreateFileMappingW(
                HANDLE::default(),
                Some(&attributes),
                PAGE_READWRITE,
                0,
                CHROMAFREE_SECTION_SIZE,
                &HSTRING::from(names.section.as_str()),
            )
        }?);
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let frame_ready = OwnedHandle(unsafe {
            CreateEventW(
                Some(&attributes),
                false,
                false,
                &HSTRING::from(names.frame_ready.as_str()),
            )
        }?);
        let consumer_changed = OwnedHandle(unsafe {
            CreateEventW(
                Some(&attributes),
                false,
                false,
                &HSTRING::from(names.consumer_changed.as_str()),
            )
        }?);
        let (view, region) = map(&section)?;
        if existed {
            region.validate()?;
        } else {
            region.initialize(qpc_frequency());
        }
        Ok(Self {
            _section: section,
            _view: view,
            region,
            frame_ready,
            consumer_changed: Arc::new(consumer_changed),
            frame_number: 0,
        })
    }

    pub fn set_output_mode(&self, mode: OutputMode) -> Result<(), IpcError> {
        self.region.set_output_mode(mode)
    }

    pub fn publish(&mut self, format: PixelFormat, width: u32, height: u32, pixels: &[u8]) -> Result<(), IpcError> {
        self.publish_parts(format, width, height, &[pixels])
    }

    pub fn publish_parts(
        &mut self,
        format: PixelFormat,
        width: u32,
        height: u32,
        parts: &[&[u8]],
    ) -> Result<(), IpcError> {
        self.frame_number += 1;
        let qpc = qpc_now();
        self.region
            .write_frame_parts(format, width, height, self.frame_number, qpc, parts)?;
        self.region.set_producer_active(true, qpc);
        unsafe { SetEvent(self.frame_ready.0) }?;
        Ok(())
    }

    pub fn set_active(&self, active: bool) {
        self.region.set_producer_active(active, qpc_now());
    }

    pub fn consumer(&self) -> ConsumerState {
        self.region.consumer_state(qpc_now())
    }

    pub fn waker(&self) -> ProducerWaker {
        ProducerWaker(Arc::clone(&self.consumer_changed))
    }

    pub fn wait_for_consumer_change(&self, timeout: Duration) -> bool {
        wait(&self.consumer_changed, timeout)
    }
}

impl Drop for ProducerChannel {
    fn drop(&mut self) {
        self.region.set_producer_active(false, qpc_now());
    }
}

pub struct ReaderChannel {
    _section: OwnedHandle,
    _view: MappedView,
    region: SharedRegion,
    frame_ready: OwnedHandle,
    consumer_changed: OwnedHandle,
    consuming: bool,
}

impl ReaderChannel {
    pub fn open(names: &ObjectNames) -> Result<Self, IpcError> {
        let section = OwnedHandle(unsafe {
            OpenFileMappingW(
                (FILE_MAP_READ | FILE_MAP_WRITE).0,
                false,
                &HSTRING::from(names.section.as_str()),
            )
        }?);
        let (view, region) = map(&section)?;
        region.validate()?;
        let frame_ready = OwnedHandle(unsafe {
            OpenEventW(
                SYNCHRONIZATION_SYNCHRONIZE,
                false,
                &HSTRING::from(names.frame_ready.as_str()),
            )
        }?);
        let consumer_changed = OwnedHandle(unsafe {
            OpenEventW(
                EVENT_MODIFY_STATE,
                false,
                &HSTRING::from(names.consumer_changed.as_str()),
            )
        }?);
        Ok(Self {
            _section: section,
            _view: view,
            region,
            frame_ready,
            consumer_changed,
            consuming: false,
        })
    }

    pub fn output_mode(&self) -> Option<OutputMode> {
        self.region.output_mode()
    }

    pub fn start(&mut self, format: PixelFormat) -> Result<(), IpcError> {
        self.start_with_size(format, None)
    }

    pub fn start_with_size(&mut self, format: PixelFormat, size: Option<(u32, u32)>) -> Result<(), IpcError> {
        if !self.consuming {
            self.region.consumer_started(format, size, qpc_now());
            self.consuming = true;
        }
        unsafe { SetEvent(self.consumer_changed.0) }?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), IpcError> {
        if self.consuming {
            self.region.consumer_stopped();
            self.consuming = false;
            unsafe { SetEvent(self.consumer_changed.0) }?;
        }
        Ok(())
    }

    pub fn heartbeat(&self) {
        self.region.consumer_heartbeat(qpc_now());
    }

    pub fn producer_alive(&self) -> bool {
        self.region.producer_alive(qpc_now())
    }

    pub fn wait_for_frame(&self, timeout: Duration) -> bool {
        wait(&self.frame_ready, timeout)
    }

    pub fn read(&self, out: &mut [u8]) -> Result<Option<FrameInfo>, IpcError> {
        self.region.read_frame(out)
    }
}

impl Drop for ReaderChannel {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
