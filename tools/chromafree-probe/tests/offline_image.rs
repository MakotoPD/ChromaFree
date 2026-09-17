use std::path::PathBuf;
use std::process::Command;

const COLOR: [u8; 3] = [40, 120, 200];
const EXPECTED_BT709_LUMA: u8 = 110;

fn build_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/vcam/Release")
}

struct OfflineImage(PathBuf);

impl Drop for OfflineImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn samples(format: &str) -> Vec<(u8, u8)> {
    let dir = build_dir();
    let output = Command::new(dir.join("vcam-source-smoke.exe"))
        .arg(dir.join("vcam-source.dll"))
        .arg(format)
        .arg("1.5")
        .output()
        .expect("build vcam-source first: cmake --build build/vcam --config Release");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with("sample"))
        .map(|line| {
            let field = |name: &str| -> u8 {
                line.split_whitespace()
                    .find_map(|part| part.strip_prefix(name))
                    .and_then(|value| value.parse().ok())
                    .unwrap()
            };
            (field("b0="), field("b3="))
        })
        .collect()
}

#[test]
#[ignore]
fn virtual_camera_shows_the_offline_image_when_the_app_is_not_running() {
    let path = build_dir().join("offline.png");
    let image = image::RgbImage::from_pixel(320, 180, image::Rgb(COLOR));
    image.save(&path).unwrap();
    let _cleanup = OfflineImage(path);

    let nv12 = samples("nv12");
    assert!(nv12.len() > 30);
    assert!(
        nv12.iter().all(|(luma, _)| luma.abs_diff(EXPECTED_BT709_LUMA) <= 3),
        "{:?}",
        &nv12[..3]
    );

    let argb = samples("argb32");
    assert!(argb.len() > 30);
    assert!(
        argb.iter().all(|&(blue, alpha)| blue == COLOR[2] && alpha == 255),
        "{:?}",
        &argb[..3]
    );
}
