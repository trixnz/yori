use std::{env, path::PathBuf};

fn main() {
    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory is set"))
        .join("../../assets/platform/windows/yori.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // openssl-src's static objects reference a build-only PDB which is not shipped.
    println!("cargo:rustc-link-arg=/IGNORE:4099");

    winresource::WindowsResource::new()
        .set_icon(&icon.to_string_lossy())
        .compile()
        .expect("failed to embed the Windows application icon");
}
