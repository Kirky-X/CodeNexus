// Copyright (c) 2026 Kirky.X
// Licensed under the terms of the LICENSE file at /home/kirky/projects/CodeNexus/LICENSE
//
// build.rs — link native OpenSSL (libssl + libcrypto) required by lbug's C++
// httplib::SSLClient extension. lbug 0.17.1's build script compiles the C++
// but does not emit cargo:rustc-link-lib directives for OpenSSL, so the final
// binary fails at link time with undefined SSL_* / X509_* symbols.

fn main() {
    // Prefer pkg-config for proper include/lib path resolution.
    if pkg_config::probe_library("openssl").is_ok() {
        return;
    }

    // Fallback: emit manual link directives (assumes system-installed OpenSSL
    // in the default linker search path, which is the case on Ubuntu/Debian
    // with libssl-dev installed).
    println!("cargo:rustc-link-lib=ssl");
    println!("cargo:rustc-link-lib=crypto");
}
