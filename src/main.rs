//! CodeNexus binary entry point.
//!
//! Dispatches to the CLI implementation defined in [`codenexus::cli`].

fn main() {
    // The CLI dispatch will be wired up in a later task. For now we print the
    // crate name and version so the binary is runnable.
    println!("codenexus {}", codenexus::version());
}
