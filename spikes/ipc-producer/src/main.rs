use std::{
    io::{self, BufRead},
    mem::{offset_of, size_of},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HLOCAL, LocalFree},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        System::{
            Memory::{
                CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS,
                MapViewOfFile, OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
            },
            Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
            Threading::{CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent},
        },
    },
    core::HSTRING,
};

const MAGIC: u32 = 0x4D41_4347;
const PROTOCOL_VERSION: u32 = 1;
const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
const FRAME_RATE: u32 = 30;
const NV12_BYTES: usize = WIDTH * HEIGHT * 3 / 2;
const HEADER_BYTES: usize = 64;
const SECTION_BYTES: usize = HEADER_BYTES + NV12_BYTES;
const CONSUMER_ACTIVE: u32 = 1;
const PRODUCER_ACTIVE: u32 = 2;
const CONSUMER_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_INTERVAL: Duration = Duration::from_millis(500);
const STATS_INTERVAL: Duration = Duration::from_secs(5);

const APP_PROBE_GLOBAL_NAME: &str = r"Global\chromafree-spike-app-probe";
const APP_SECTION_NAME: &str = r"Local\chromafree-spike-app-section";
const APP_EVENT_NAME: &str = r"Local\chromafree-spike-app-event";
const DLL_SECTION_NAME: &str = r"Global\chromafree-spike-dll-section";
const DLL_EVENT_NAME: &str = r"Global\chromafree-spike-dll-event";
const SHARED_OBJECT_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;LS)(A;;GA;;;IU)";

#[repr(C)]
struct Header {
    magic: u32,
    version: u32,
    width: u32,
    height: u32,
    sequence: AtomicU64,
    frame_number: u64,
    frame_qpc: i64,
    flags: AtomicU32,
    reserved: u32,
    producer_heartbeat_qpc: AtomicI64,
    consumer_heartbeat_qpc: AtomicI64,
}

const _: () = {
    assert!(size_of::<Header>() == HEADER_BYTES);
    assert!(offset_of!(Header, sequence) == 16);
    assert!(offset_of!(Header, flags) == 40);
    assert!(offset_of!(Header, consumer_heartbeat_qpc) == 56);
};

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct Channel {
    label: &'static str,
    _section: OwnedHandle,
    event: Option<OwnedHandle>,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    frames_written: u64,
}

impl Channel {
    fn header(&self) -> &Header {
        unsafe { &*(self.view.Value as *const Header) }
    }

    fn header_mut(&mut self) -> &mut Header {
        unsafe { &mut *(self.view.Value as *mut Header) }
    }

    fn write_frame(&mut self, frame: &[u8], frame_number: u64, qpc: i64) {
        let data = unsafe { (self.view.Value as *mut u8).add(HEADER_BYTES) };
        let header = self.header_mut();
        header.sequence.fetch_add(1, Ordering::AcqRel);
        unsafe { ptr::copy_nonoverlapping(frame.as_ptr(), data, NV12_BYTES) };
        header.frame_number = frame_number;
        header.frame_qpc = qpc;
        header.sequence.fetch_add(1, Ordering::AcqRel);
        header.producer_heartbeat_qpc.store(qpc, Ordering::Relaxed);
        header.flags.fetch_or(PRODUCER_ACTIVE, Ordering::Relaxed);
        if let Some(event) = &self.event {
            let _ = unsafe { SetEvent(event.0) };
        }
        self.frames_written += 1;
    }

    fn consumer_active(&self, now: i64) -> bool {
        let header = self.header();
        header.flags.load(Ordering::Relaxed) & CONSUMER_ACTIVE != 0
            && qpc_to_duration(now - header.consumer_heartbeat_qpc.load(Ordering::Relaxed))
                < CONSUMER_TIMEOUT
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.header()
            .flags
            .fetch_and(!PRODUCER_ACTIVE, Ordering::Relaxed);
        let _ = unsafe { UnmapViewOfFile(self.view) };
    }
}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> Result<Self> {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from(sddl),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )?
        };
        Ok(Self(descriptor))
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0.0,
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

fn qpc_now() -> i64 {
    let mut value = 0;
    let _ = unsafe { QueryPerformanceCounter(&mut value) };
    value
}

fn qpc_to_duration(ticks: i64) -> Duration {
    static FREQUENCY: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut value = 1;
        let _ = unsafe { QueryPerformanceFrequency(&mut value) };
        value
    });
    Duration::from_secs_f64(ticks.max(0) as f64 / frequency as f64)
}

fn map_view(section: &OwnedHandle) -> Result<MEMORY_MAPPED_VIEW_ADDRESS> {
    let view = unsafe {
        MapViewOfFile(
            section.0,
            FILE_MAP_READ | FILE_MAP_WRITE,
            0,
            0,
            SECTION_BYTES,
        )
    };
    if view.Value.is_null() {
        return Err(windows::core::Error::from_thread()).context("MapViewOfFile");
    }
    Ok(view)
}

fn probe_global_creation() {
    let result = unsafe {
        CreateFileMappingW(
            HANDLE::default(),
            None,
            PAGE_READWRITE,
            0,
            4096,
            &HSTRING::from(APP_PROBE_GLOBAL_NAME),
        )
    };
    match result {
        Ok(handle) => {
            println!(
                "[A] create {APP_PROBE_GLOBAL_NAME} without SeCreateGlobalPrivilege: SUCCEEDED (unexpected)"
            );
            drop(OwnedHandle(handle));
        }
        Err(error) => println!("[A] create {APP_PROBE_GLOBAL_NAME}: {error}"),
    }
}

fn create_app_channel(descriptor: &SecurityDescriptor) -> Result<Channel> {
    let attributes = descriptor.attributes();
    let section = unsafe {
        CreateFileMappingW(
            HANDLE::default(),
            Some(&attributes),
            PAGE_READWRITE,
            0,
            SECTION_BYTES as u32,
            &HSTRING::from(APP_SECTION_NAME),
        )
    }
    .context("[E] CreateFileMappingW")?;
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let section = OwnedHandle(section);
    let event = unsafe {
        CreateEventW(
            Some(&attributes),
            false,
            false,
            &HSTRING::from(APP_EVENT_NAME),
        )
    }
    .context("[E] CreateEventW")?;
    let view = map_view(&section)?;
    let mut channel = Channel {
        label: "E",
        _section: section,
        event: Some(OwnedHandle(event)),
        view,
        frames_written: 0,
    };
    let header = channel.header_mut();
    header.magic = MAGIC;
    header.version = PROTOCOL_VERSION;
    header.width = WIDTH as u32;
    header.height = HEIGHT as u32;
    println!("[E] created {APP_SECTION_NAME} and {APP_EVENT_NAME} (already existed: {existed})");
    Ok(channel)
}

fn open_dll_channel() -> windows::core::Result<Channel> {
    let section = OwnedHandle(unsafe {
        OpenFileMappingW(
            (FILE_MAP_READ | FILE_MAP_WRITE).0,
            false,
            &HSTRING::from(DLL_SECTION_NAME),
        )?
    });
    let view = unsafe {
        MapViewOfFile(
            section.0,
            FILE_MAP_READ | FILE_MAP_WRITE,
            0,
            0,
            SECTION_BYTES,
        )
    };
    if view.Value.is_null() {
        return Err(windows::core::Error::from_thread());
    }
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(DLL_EVENT_NAME)) };
    if let Err(error) = &event {
        println!("[C] open {DLL_EVENT_NAME}: {error}");
    }
    Ok(Channel {
        label: "C",
        _section: section,
        event: event.ok().map(OwnedHandle),
        view,
        frames_written: 0,
    })
}

fn draw_pattern(frame: &mut [u8], index: u64) {
    let shift = (index * 4) as usize;
    let (luma, chroma) = frame.split_at_mut(WIDTH * HEIGHT);
    for (y, row) in luma.chunks_exact_mut(WIDTH).enumerate() {
        for (x, pixel) in row.iter_mut().enumerate() {
            let checker = ((x + shift) / 80 + y / 80).is_multiple_of(2);
            *pixel = if checker { 200 } else { 40 };
        }
    }
    let progress = (index as usize * 8) % WIDTH;
    for row in luma.chunks_exact_mut(WIDTH).take(24) {
        row[..progress].fill(235);
    }
    for (y, row) in chroma.chunks_exact_mut(WIDTH).enumerate() {
        let magenta = (y * 2 / 90).is_multiple_of(2);
        for pair in row.chunks_exact_mut(2) {
            pair[0] = if magenta { 180 } else { 60 };
            pair[1] = if magenta { 200 } else { 50 };
        }
    }
}

fn main() -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        thread::spawn(move || {
            let _ = io::stdin().lock().lines().next();
            stop.store(true, Ordering::Relaxed);
        });
    }

    println!("producer pid={} (press Enter to stop)", std::process::id());
    probe_global_creation();
    let descriptor = SecurityDescriptor::from_sddl(SHARED_OBJECT_SDDL)?;
    let mut app_channel = create_app_channel(&descriptor)?;
    let mut dll_channel: Option<Channel> = None;
    let mut last_open_error = String::new();

    let mut frame = vec![0u8; NV12_BYTES];
    let frame_interval = Duration::from_secs(1) / FRAME_RATE;
    let started = Instant::now();
    let mut next_frame = started;
    let mut next_probe = started;
    let mut next_stats = started + STATS_INTERVAL;
    let mut index = 0u64;
    let mut write_time = Duration::ZERO;

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if dll_channel.is_none() && now >= next_probe {
            next_probe = now + PROBE_INTERVAL;
            match open_dll_channel() {
                Ok(channel) => {
                    println!("[C] opened {DLL_SECTION_NAME} created by the DLL");
                    last_open_error.clear();
                    dll_channel = Some(channel);
                }
                Err(error) => {
                    let text = error.to_string();
                    if text != last_open_error {
                        println!("[C] open {DLL_SECTION_NAME}: {text}");
                        last_open_error = text;
                    }
                }
            }
        }

        draw_pattern(&mut frame, index);
        let qpc = qpc_now();
        let write_start = Instant::now();
        app_channel.write_frame(&frame, index, qpc);
        if let Some(channel) = dll_channel.as_mut() {
            channel.write_frame(&frame, index, qpc);
        }
        write_time += write_start.elapsed();
        index += 1;

        if dll_channel.as_ref().is_some_and(|c| {
            !c.consumer_active(qpc) && c.frames_written > u64::from(FRAME_RATE) * 3
        }) {
            println!("[C] consumer inactive for {CONSUMER_TIMEOUT:?}, releasing DLL section");
            dll_channel = None;
        }

        if now >= next_stats {
            next_stats = now + STATS_INTERVAL;
            let channels: Vec<String> = std::iter::once(&app_channel)
                .chain(dll_channel.as_ref())
                .map(|c| {
                    format!(
                        "[{}] frames={} consumer_active={}",
                        c.label,
                        c.frames_written,
                        c.consumer_active(qpc)
                    )
                })
                .collect();
            println!(
                "t={:.0}s frames={} write avg={:.3}ms {}",
                started.elapsed().as_secs_f64(),
                index,
                write_time.as_secs_f64() * 1000.0 / index as f64,
                channels.join(" ")
            );
        }

        next_frame += frame_interval;
        let sleep_for = next_frame.saturating_duration_since(Instant::now());
        if sleep_for.is_zero() {
            next_frame = Instant::now();
        } else {
            thread::sleep(sleep_for);
        }
    }

    println!("stopping");
    Ok(())
}
