use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{self, Command},
};

use sha2::{Digest, Sha256};

const RELEASE: &str = "r25.1";
const VERSION: &str = "2025.1 patch 3042095";

struct Distribution {
    platform: &'static str,
    archive: &'static str,
    sha256: &'static str,
}

fn main() {
    println!("cargo:rerun-if-env-changed=P4API_CACHE_DIR");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/native/p4_bridge.h");
    println!("cargo:rerun-if-changed=src/native/p4_bridge.cc");

    let target = env::var("TARGET").expect("Cargo always sets TARGET");
    let distribution = distribution(&target);
    let root = provision_p4api(&target, &distribution);
    let include = root.join("include");
    let libraries = root.join("lib");

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
        println!("cargo:rustc-link-lib=static=libp4script_cstub");

        link_openssl();

        for library in [
            "advapi32", "bcrypt", "crypt32", "iphlpapi", "kernel32", "oldnames", "ole32",
            "shell32", "user32", "ws2_32",
        ] {
            println!("cargo:rustc-link-lib={library}");
        }
    } else {
        println!("cargo:rustc-link-lib=static=client");
        println!("cargo:rustc-link-lib=static=rpc");
        println!("cargo:rustc-link-lib=static=supp");
        println!("cargo:rustc-link-lib=static=p4script_cstub");

        link_openssl();

        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=dl");
        println!("cargo:rustc-link-lib=rt");
    }
}

fn distribution(target: &str) -> Distribution {
    match target {
        "x86_64-unknown-linux-gnu" => Distribution {
            platform: "bin.linux26x86_64",
            archive: "p4api-glibc2.12-openssl3.5.tgz",
            sha256: "b3840d7e4b889e480929134703d2215409f8a86e8e206158fb6651734086805d",
        },
        "x86_64-pc-windows-msvc" => Distribution {
            platform: "bin.ntx64",
            archive: "p4api_vs2022_dyn_openssl3.5.zip",
            sha256: "b05db557dc5dd8d3b3e316632afb457bb1c4bdf4b696ffc14e5df78acc743a57",
        },
        _ => panic!("the native Perforce provider does not support target {target}"),
    }
}

fn provision_p4api(target: &str, distribution: &Distribution) -> PathBuf {
    let cache_entry = p4api_cache_dir().join(target).join(distribution.sha256);
    let root = cache_entry.join("root");
    let marker = cache_entry.join("complete");

    if fs::read_to_string(&marker).is_ok_and(|value| value.trim() == distribution.sha256)
        && validate_p4api(&root).is_ok()
    {
        return root;
    }

    fs::create_dir_all(&cache_entry).unwrap_or_else(|error| {
        panic!(
            "cannot create P4API cache directory {}: {error}",
            cache_entry.display()
        )
    });

    let archive = cache_entry.join(distribution.archive);
    download_p4api(&archive, distribution);
    extract_p4api(&archive, &root);

    validate_p4api(&root).unwrap_or_else(|error| panic!("{error}"));
    fs::write(&marker, format!("{}\n", distribution.sha256)).unwrap_or_else(|error| {
        panic!(
            "cannot complete P4API cache entry {}: {error}",
            cache_entry.display()
        )
    });
    let _ = fs::remove_file(archive);

    root
}

fn p4api_cache_dir() -> PathBuf {
    if let Some(path) = env::var_os("P4API_CACHE_DIR") {
        return PathBuf::from(path);
    }

    let base = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("OUT_DIR").expect("Cargo always sets OUT_DIR"))
        });

    base.join("yori").join("p4api")
}

fn download_p4api(destination: &Path, distribution: &Distribution) {
    if destination.is_file() {
        let actual = file_sha256(destination);

        if actual == distribution.sha256 {
            return;
        }

        fs::remove_file(destination).unwrap_or_else(|error| {
            panic!(
                "cannot remove invalid P4API archive {}: {error}",
                destination.display()
            )
        });
    }

    let url = format!(
        "https://ftp.perforce.com/perforce/{RELEASE}/{}/{}",
        distribution.platform, distribution.archive
    );
    let partial = destination.with_extension(format!("partial-{}", process::id()));
    let _ = fs::remove_file(&partial);

    println!("cargo:warning=downloading pinned P4API from {url}");
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--retry",
            "3",
            "--silent",
            "--show-error",
        ])
        .arg("--output")
        .arg(&partial)
        .arg(&url)
        .status()
        .unwrap_or_else(|error| {
            panic!("cannot run curl to download P4API: {error}; install curl and retry")
        });

    if !status.success() {
        let _ = fs::remove_file(&partial);
        panic!("curl failed to download P4API from {url}");
    }

    let actual = file_sha256(&partial);
    if actual != distribution.sha256 {
        let _ = fs::remove_file(&partial);
        panic!(
            "P4API archive checksum mismatch for {url}: expected {}, got {actual}",
            distribution.sha256
        );
    }

    fs::rename(&partial, destination).unwrap_or_else(|error| {
        panic!(
            "cannot finish P4API download {}: {error}",
            destination.display()
        )
    });
}

fn extract_p4api(archive: &Path, root: &Path) {
    let cache_entry = root
        .parent()
        .expect("P4API root always has a cache entry parent");
    let staging = cache_entry.join(format!("extracting-{}", process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).unwrap_or_else(|error| {
        panic!(
            "cannot create P4API extraction directory {}: {error}",
            staging.display()
        )
    });

    let status = Command::new("tar")
        .arg("--extract")
        .arg("--file")
        .arg(archive)
        .arg("--directory")
        .arg(&staging)
        .status()
        .unwrap_or_else(|error| {
            panic!("cannot run tar to extract P4API: {error}; install tar and retry")
        });

    if !status.success() {
        let _ = fs::remove_dir_all(&staging);
        panic!("tar failed to extract P4API archive {}", archive.display());
    }

    let extracted = find_p4api_root(&staging).unwrap_or_else(|| {
        panic!(
            "P4API archive did not contain include/p4/clientapi.h under {}",
            staging.display()
        )
    });
    let _ = fs::remove_dir_all(root);

    if extracted == staging {
        fs::rename(&staging, root)
            .unwrap_or_else(|error| panic!("cannot install extracted P4API: {error}"));
    } else {
        fs::rename(&extracted, root)
            .unwrap_or_else(|error| panic!("cannot install extracted P4API: {error}"));
        let _ = fs::remove_dir_all(&staging);
    }
}

fn find_p4api_root(directory: &Path) -> Option<PathBuf> {
    if directory.join("include/p4/clientapi.h").is_file() {
        return Some(directory.to_path_buf());
    }

    fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_dir() && path.join("include/p4/clientapi.h").is_file())
}

fn validate_p4api(root: &Path) -> Result<(), String> {
    if !root.join("include/p4/clientapi.h").is_file() {
        return Err(format!(
            "P4API cache does not contain include/p4/clientapi.h: {}",
            root.display()
        ));
    }

    let version_path = root.join("sample/Version");
    let version = fs::read_to_string(&version_path).map_err(|error| {
        format!(
            "cannot read pinned P4API version from {}: {error}",
            version_path.display()
        )
    })?;

    if !version.contains("RELEASE = 2025 1 ;") || !version.contains("PATCHLEVEL = 3042095 ;") {
        return Err(format!("P4API cache is not {VERSION}: {}", root.display()));
    }

    Ok(())
}

fn file_sha256(path: &Path) -> String {
    let mut file = fs::File::open(path)
        .unwrap_or_else(|error| panic!("cannot open {} for verification: {error}", path.display()));
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8 * 1024];

    loop {
        let read = file
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        if read == 0 {
            break;
        }

        hasher.update(&buffer[..read]);
    }

    format!("{:x}", hasher.finalize())
}

fn link_openssl() {
    let openssl = openssl_src::Build::new().build();
    println!(
        "cargo:rustc-link-search=native={}",
        openssl.lib_dir().display()
    );

    for library in openssl.libs() {
        println!("cargo:rustc-link-lib=static={library}");
    }
}
