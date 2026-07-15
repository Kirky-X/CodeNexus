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
}
