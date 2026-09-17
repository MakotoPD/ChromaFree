use std::path::{Path, PathBuf};

const PRODUCT_NAME: &str = "ChromaFree";
const FILE_DESCRIPTION: &str = "ChromaFree virtual camera";
const COPYRIGHT_YEAR: &str = "2026";

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn rc_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn resource_script(manifest_dir: &Path) -> String {
    let version = env("CARGO_PKG_VERSION");
    let numeric = format!(
        "{},{},{},0",
        env("CARGO_PKG_VERSION_MAJOR"),
        env("CARGO_PKG_VERSION_MINOR"),
        env("CARGO_PKG_VERSION_PATCH")
    );
    let author = env("CARGO_PKG_AUTHORS");
    let license = env("CARGO_PKG_LICENSE");
    let icon = rc_path(&manifest_dir.join("assets").join("chromafree.ico"));
    let manifest = rc_path(&manifest_dir.join("chromafree.manifest"));
    [
        "#include <winver.h>".to_owned(),
        format!("1 ICON \"{icon}\""),
        format!("1 24 \"{manifest}\""),
        "1 VERSIONINFO".to_owned(),
        format!("FILEVERSION {numeric}"),
        format!("PRODUCTVERSION {numeric}"),
        "FILEOS VOS_NT_WINDOWS32".to_owned(),
        "FILETYPE VFT_APP".to_owned(),
        "BEGIN".to_owned(),
        "BLOCK \"StringFileInfo\"".to_owned(),
        "BEGIN".to_owned(),
        "BLOCK \"040904B0\"".to_owned(),
        "BEGIN".to_owned(),
        format!("VALUE \"CompanyName\", \"{author}\""),
        format!("VALUE \"FileDescription\", \"{FILE_DESCRIPTION}\""),
        format!("VALUE \"FileVersion\", \"{version}\""),
        "VALUE \"InternalName\", \"chromafree\"".to_owned(),
        format!("VALUE \"LegalCopyright\", \"Copyright (C) {COPYRIGHT_YEAR} {author}. {license}\""),
        "VALUE \"OriginalFilename\", \"chromafree.exe\"".to_owned(),
        format!("VALUE \"ProductName\", \"{PRODUCT_NAME}\""),
        format!("VALUE \"ProductVersion\", \"{version}\""),
        "END".to_owned(),
        "END".to_owned(),
        "BLOCK \"VarFileInfo\"".to_owned(),
        "BEGIN".to_owned(),
        "VALUE \"Translation\", 0x409, 1200".to_owned(),
        "END".to_owned(),
        "END".to_owned(),
    ]
    .join("\n")
}

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg=/DELAYLOAD:DirectML.dll");
        println!("cargo:rustc-link-lib=delayimp");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
        println!("cargo:rerun-if-changed=assets/chromafree.ico");
        println!("cargo:rerun-if-changed=chromafree.manifest");
        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-changed=Cargo.toml");
        println!("cargo:rerun-if-changed=../../Cargo.toml");
        let manifest_dir = PathBuf::from(env("CARGO_MANIFEST_DIR"));
        let script = PathBuf::from(env("OUT_DIR")).join("chromafree.rc");
        std::fs::write(&script, resource_script(&manifest_dir)).expect("writing chromafree.rc");
        embed_resource::compile(&script, embed_resource::NONE)
            .manifest_optional()
            .expect("compiling chromafree.rc");
    }
    println!("cargo:rerun-if-changed=translations");
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/main.slint", config).expect("compiling ui/main.slint");
}
