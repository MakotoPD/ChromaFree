fn main() {
    let _mf = chromafree_capture::MediaFoundation::startup().unwrap();
    for camera in chromafree_capture::list_all_cameras().unwrap() {
        println!("{} | {}", camera.name, camera.symbolic_link);
    }
}
