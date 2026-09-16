use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use bgcam_ipc::layout::BGCAM_HEADER_SIZE;
use bgcam_ipc::{IpcError, PixelFormat, SharedRegion};

struct Memory {
    _words: Vec<u64>,
    region: SharedRegion,
}

fn memory(capacity: usize) -> Arc<Memory> {
    let len = (BGCAM_HEADER_SIZE as usize + capacity).div_ceil(8);
    let mut words = vec![0u64; len];
    let region = unsafe { SharedRegion::from_raw(words.as_mut_ptr().cast::<u8>(), len * 8) }.unwrap();
    region.initialize(1_000_000);
    Arc::new(Memory { _words: words, region })
}

const SIZES: [(u32, u32); 3] = [(64, 36), (128, 72), (32, 18)];

#[test]
fn concurrent_readers_never_observe_torn_frames() {
    let (largest_width, largest_height) = SIZES[1];
    let memory = memory(PixelFormat::Bgra.frame_size(largest_width, largest_height));
    let done = Arc::new(AtomicBool::new(false));

    let readers: Vec<_> = (0..4)
        .map(|_| {
            let memory = Arc::clone(&memory);
            let done = Arc::clone(&done);
            thread::spawn(move || {
                let mut out = vec![0u8; memory.region.capacity()];
                let (mut frames, mut contended, mut last) = (0u64, 0u64, 0u64);
                while !done.load(Ordering::Relaxed) {
                    match memory.region.read_frame(&mut out) {
                        Ok(Some(info)) => {
                            let expected = (info.frame_number % 251) as u8;
                            let (width, height) = SIZES[(info.frame_number % 3) as usize];
                            let format = if info.frame_number % 2 == 0 {
                                PixelFormat::Nv12
                            } else {
                                PixelFormat::Bgra
                            };
                            assert_eq!((info.width, info.height, info.format), (width, height, format));
                            assert!(
                                out[..info.size].iter().all(|&b| b == expected),
                                "torn frame {}",
                                info.frame_number
                            );
                            assert!(info.frame_number >= last);
                            last = info.frame_number;
                            frames += 1;
                        }
                        Ok(None) => {}
                        Err(IpcError::Contended) => contended += 1,
                        Err(error) => panic!("{error}"),
                    }
                }
                (frames, contended)
            })
        })
        .collect();

    let mut pixels = vec![0u8; memory.region.capacity()];
    for frame_number in 1..=20_000u64 {
        let (width, height) = SIZES[(frame_number % 3) as usize];
        let format = if frame_number % 2 == 0 {
            PixelFormat::Nv12
        } else {
            PixelFormat::Bgra
        };
        let size = format.frame_size(width, height);
        pixels[..size].fill((frame_number % 251) as u8);
        memory
            .region
            .write_frame(format, width, height, frame_number, frame_number as i64, &pixels)
            .unwrap();
        if frame_number % 64 == 0 {
            thread::yield_now();
        }
    }
    done.store(true, Ordering::Relaxed);

    for reader in readers {
        let (frames, contended) = reader.join().unwrap();
        assert!(frames > 0, "reader never completed a read ({contended} contended)");
    }
}
