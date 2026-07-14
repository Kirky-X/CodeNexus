// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

fn main() {
    // lbug bundles httplib (including SSLClient) in extension_installer.cpp.
    // When linking the codenexus binary, the linker pulls in this object file,
    // which references OpenSSL symbols (SSL_ctrl, SSL_CTX_*, TLS_client_method,
    // etc.). Link system OpenSSL (libssl + libcrypto) unconditionally to
    // satisfy these symbols for any binary target.
    //
    // The zstd duplicate-symbol issue (only when `inklog` feature is enabled)
    // is handled via .cargo/config.toml (ZSTD_SYS_USE_PKG_CONFIG=1 +
    // PKG_CONFIG_PATH=build-support).
    println!("cargo:rustc-link-lib=ssl");
    println!("cargo:rustc-link-lib=crypto");
}
