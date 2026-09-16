use std::mem::{offset_of, size_of};
use std::path::PathBuf;

use bgcam_ipc::layout::*;

fn wide_string_constants() -> String {
    [
        ("BGCAM_SECTION_NAME", BGCAM_SECTION_NAME),
        ("BGCAM_FRAME_READY_EVENT_NAME", BGCAM_FRAME_READY_EVENT_NAME),
        ("BGCAM_CONSUMER_CHANGED_EVENT_NAME", BGCAM_CONSUMER_CHANGED_EVENT_NAME),
    ]
    .iter()
    .map(|(name, value)| {
        format!(
            "
#define {name} L\"{value}\"
"
        )
    })
    .collect()
}

fn generated_header() -> String {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = cbindgen::Config {
        language: cbindgen::Language::C,
        pragma_once: true,
        after_includes: Some(wide_string_constants()),
        no_includes: true,
        sys_includes: vec!["stdint.h".into()],
        documentation: false,
        style: cbindgen::Style::Type,
        export: cbindgen::ExportConfig {
            include: vec!["BgcamFrameHeader".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    let mut output = Vec::new();
    cbindgen::Builder::new()
        .with_config(config)
        .with_src(manifest.join("src/layout.rs"))
        .generate()
        .expect("cbindgen failed")
        .write(&mut output);
    String::from_utf8(output).expect("header is not UTF-8")
}

#[test]
fn committed_c_header_matches_rust_layout() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("include/bgcam_ipc.h");
    let generated = generated_header();
    if std::env::var("BGCAM_UPDATE_HEADER").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert_eq!(
        committed, generated,
        "run with BGCAM_UPDATE_HEADER=1 to regenerate include/bgcam_ipc.h"
    );
    assert!(
        !generated.contains("/*") && !generated.contains("//"),
        "generated header contains comments"
    );
}

#[test]
fn header_layout_is_frozen() {
    assert_eq!(size_of::<BgcamFrameHeader>(), BGCAM_HEADER_SIZE as usize);
    let offsets = [
        (offset_of!(BgcamFrameHeader, magic), 0),
        (offset_of!(BgcamFrameHeader, version), 4),
        (offset_of!(BgcamFrameHeader, header_size), 8),
        (offset_of!(BgcamFrameHeader, capacity), 12),
        (offset_of!(BgcamFrameHeader, output_width), 16),
        (offset_of!(BgcamFrameHeader, output_height), 20),
        (offset_of!(BgcamFrameHeader, output_fps_numerator), 24),
        (offset_of!(BgcamFrameHeader, output_fps_denominator), 28),
        (offset_of!(BgcamFrameHeader, frame_width), 32),
        (offset_of!(BgcamFrameHeader, frame_height), 36),
        (offset_of!(BgcamFrameHeader, frame_format), 40),
        (offset_of!(BgcamFrameHeader, frame_size), 44),
        (offset_of!(BgcamFrameHeader, producer_flags), 48),
        (offset_of!(BgcamFrameHeader, consumer_flags), 52),
        (offset_of!(BgcamFrameHeader, consumer_format), 56),
        (offset_of!(BgcamFrameHeader, consumer_count), 60),
        (offset_of!(BgcamFrameHeader, sequence), 64),
        (offset_of!(BgcamFrameHeader, frame_number), 72),
        (offset_of!(BgcamFrameHeader, frame_qpc), 80),
        (offset_of!(BgcamFrameHeader, producer_heartbeat_qpc), 88),
        (offset_of!(BgcamFrameHeader, consumer_heartbeat_qpc), 96),
        (offset_of!(BgcamFrameHeader, qpc_frequency), 104),
        (offset_of!(BgcamFrameHeader, reserved), 112),
    ];
    for (index, (actual, expected)) in offsets.iter().enumerate() {
        assert_eq!(actual, expected, "field #{index}");
    }
    assert_eq!(
        BGCAM_PROTOCOL_VERSION, 1,
        "bump the protocol version together with any layout change"
    );
}
