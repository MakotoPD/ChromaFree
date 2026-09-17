fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg=/DELAYLOAD:DirectML.dll");
        println!("cargo:rustc-link-lib=delayimp");
        println!("cargo:rerun-if-changed=assets/chromafree.ico");
        embed_resource::compile("chromafree.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("compiling chromafree.rc");
    }
    slint_build::compile("ui/main.slint").expect("compiling ui/main.slint");
}
