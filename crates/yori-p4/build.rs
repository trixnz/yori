use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=P4API_ROOT");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/native/p4_bridge.h");
    println!("cargo:rerun-if-changed=src/native/p4_bridge.cc");

    let root = env::var_os("P4API_ROOT").map_or_else(
        || {
            panic!(
                "P4API_ROOT is not set; run through ./scripts/check or provision the pinned P4API with ./scripts/fetch-p4api"
            )
        },
        PathBuf::from,
    );
    let include = root.join("include");
    let libraries = root.join("lib");

    assert!(
        include.join("p4/clientapi.h").is_file(),
        "P4API_ROOT does not contain include/p4/clientapi.h: {}",
        root.display()
    );

    let version_path = root.join("sample/Version");
    println!("cargo:rerun-if-changed={}", version_path.display());
    let version = fs::read_to_string(&version_path).unwrap_or_else(|error| {
        panic!(
            "cannot read pinned P4API version from {}: {error}",
            version_path.display()
        )
    });
    assert!(
        version.contains("RELEASE = 2025 1 ;") && version.contains("PATCHLEVEL = 3042095 ;"),
        "P4API_ROOT is not P4API 2025.1 patch 3042095: {}",
        root.display()
    );

    let target = env::var("TARGET").expect("Cargo always sets TARGET");
    assert!(
        matches!(
            target.as_str(),
            "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu" | "x86_64-pc-windows-msvc"
        ),
        "the native Perforce provider does not support target {target}"
    );

    let mut bridge = cxx_build::bridge("src/lib.rs");
    bridge
        .file("src/native/p4_bridge.cc")
        .include("src/native")
        .include(include)
        .std("c++17")
        .opt_level(1)
        .warnings(true);

    if target.contains("windows") {
        bridge
            .define("OS_NT", None)
            .define("CASE_INSENSITIVE", None);
        bridge.static_crt(false);
    } else {
        bridge.define("OS_LINUX", None);
    }

    bridge.compile("yori-p4-bridge");

    println!("cargo:rustc-link-search=native={}", libraries.display());

    if target.contains("windows") {
        println!("cargo:rustc-link-lib=static=libclient");
        println!("cargo:rustc-link-lib=static=librpc");
        println!("cargo:rustc-link-lib=static=libsupp");
        println!("cargo:rustc-link-lib=libssl");
        println!("cargo:rustc-link-lib=libcrypto");

        for library in [
            "advapi32", "bcrypt", "crypt32", "iphlpapi", "kernel32", "oldnames", "user32", "ws2_32",
        ] {
            println!("cargo:rustc-link-lib={library}");
        }
    } else {
        println!("cargo:rustc-link-lib=static=client");
        println!("cargo:rustc-link-lib=static=rpc");
        println!("cargo:rustc-link-lib=static=supp");
        println!("cargo:rustc-link-lib=ssl");
        println!("cargo:rustc-link-lib=crypto");
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=dl");
        println!("cargo:rustc-link-lib=rt");
    }
}
