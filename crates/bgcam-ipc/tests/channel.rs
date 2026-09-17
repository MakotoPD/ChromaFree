use std::time::Duration;

use bgcam_ipc::{ObjectNames, OutputMode, PixelFormat, ProducerChannel, ReaderChannel};

fn unique_names(test: &str) -> ObjectNames {
    ObjectNames::with_prefix(&format!(r"Local\bgcam-test-{}-{test}-", std::process::id()))
}

#[test]
fn reader_without_producer_fails_to_open() {
    assert!(ReaderChannel::open(&unique_names("absent")).is_err());
}

#[test]
fn producer_and_reader_exchange_frames_and_consumer_state() {
    let names = unique_names("roundtrip");
    let mut producer = ProducerChannel::create(&names).unwrap();
    let mode = OutputMode {
        width: 1280,
        height: 720,
        fps_numerator: 30,
        fps_denominator: 1,
    };
    producer.set_output_mode(mode).unwrap();

    let mut reader = ReaderChannel::open(&names).unwrap();
    assert_eq!(reader.output_mode(), Some(mode));
    assert!(!producer.consumer().active);
    assert!(!reader.producer_alive());

    reader.start(PixelFormat::Nv12).unwrap();
    assert!(producer.wait_for_consumer_change(Duration::from_secs(1)));
    let consumer = producer.consumer();
    assert!(consumer.active);
    assert_eq!(consumer.format, Some(PixelFormat::Nv12));
    assert_eq!(consumer.count, 1);

    let pixels: Vec<u8> = (0..PixelFormat::Nv12.frame_size(64, 36)).map(|i| i as u8).collect();
    producer.publish(PixelFormat::Nv12, 64, 36, &pixels).unwrap();
    assert!(reader.wait_for_frame(Duration::from_secs(1)));
    assert!(reader.producer_alive());
    let mut out = vec![0u8; pixels.len()];
    let info = reader.read(&mut out).unwrap().unwrap();
    assert_eq!((info.width, info.height, info.frame_number), (64, 36, 1));
    assert_eq!(out, pixels);

    assert!(!reader.wait_for_frame(Duration::from_millis(10)));
    drop(reader);
    assert!(producer.wait_for_consumer_change(Duration::from_secs(1)));
    assert!(!producer.consumer().active);

    let second = ReaderChannel::open(&names).unwrap();
    drop(producer);
    assert!(!second.producer_alive());
}

#[test]
fn waker_interrupts_consumer_wait_and_parts_are_concatenated() {
    let names = unique_names("parts");
    let mut producer = ProducerChannel::create(&names).unwrap();
    let waker = producer.waker();
    let waiter = std::thread::spawn(move || {
        waker.wake();
    });
    assert!(producer.wait_for_consumer_change(Duration::from_secs(1)));
    waiter.join().unwrap();

    let luma = [7u8; 16];
    let chroma = [9u8; 8];
    producer
        .publish_parts(PixelFormat::Nv12, 4, 4, &[&luma, &chroma])
        .unwrap();
    let reader = ReaderChannel::open(&names).unwrap();
    let mut out = [0u8; 24];
    reader.read(&mut out).unwrap().unwrap();
    assert_eq!(&out[..16], &luma);
    assert_eq!(&out[16..], &chroma);
}
