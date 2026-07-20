// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

fn main() {
    // lbug bundles httplib (including SSLClient) in extension_installer.cpp.
    // When linking the codenexus binary, the linker pulls in this object file,
    // which references OpenSSL symbols (SSL_ctrl, SSL_CTX_*, TLS_client_method,
    // etc.). Link system OpenSSL (libssl + libcrypto) unconditionally to
    // satisfy these symbols for any binary target.
    //
    // zstd-sys is no longer in the dependency tree (inklog 0.1.10+ uses gzip
    // fallback via flate2 when `compression` feature is not enabled). The
    // .cargo/config.toml zstd env vars are retained for future use but are
    // currently inactive.
    println!("cargo:rustc-link-lib=ssl");
    println!("cargo:rustc-link-lib=crypto");

    // lbug's prebuilt static lib (base_csv_reader.cpp.o) references __cpu_model,
    // a GCC runtime symbol that lives in libgcc.a (static), not libgcc_s.so
    // (dynamic). rustc's default link line passes -lgcc_s but not -lgcc, so
    // strict linkers like mold fail with "undefined symbol: __cpu_model" on the
    // codenexus-verify binary. Probe libgcc.a's directory via the C compiler
    // and link it statically on Linux/gcc targets (macOS/Windows clang doesn't
    // emit __cpu_model).
    #[cfg(target_os = "linux")]
    {
        let libgcc_dir = std::process::Command::new("cc")
            .arg("-print-file-name=libgcc.a")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                std::path::Path::new(s.trim())
                    .parent()
                    .map(|p| p.to_path_buf())
            })
            .filter(|d| !d.as_os_str().is_empty());
        if let Some(dir) = libgcc_dir {
            println!("cargo:rustc-link-search=native={}", dir.display());
        }
        println!("cargo:rustc-link-lib=static=gcc");
    }
}
