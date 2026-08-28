//! Ensures the embedded-asset folder exists so `cargo build` succeeds
//! before (or without) a web build. An executable built that way serves an
//! honest "interface not embedded" notice instead of failing to compile;
//! release packaging always builds `web/` first (see `.github/workflows/`).

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let web_build = std::path::Path::new(&manifest_dir).join("../../web/build");
    std::fs::create_dir_all(&web_build).expect("creating web/build placeholder directory");
    // Re-run when the built assets change so the embed stays current.
    println!("cargo::rerun-if-changed={}", web_build.display());
}
