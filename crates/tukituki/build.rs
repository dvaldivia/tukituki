use std::process::Command;

fn main() {
    // Capture the rustc version that compiled this binary, so `tukituki
    // version` can report it the way the Go binary reports `runtime.Version()`.
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let version = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "rustc unknown".to_string());

    println!("cargo:rustc-env=TUKITUKI_RUSTC_VERSION={version}");
    // Re-run only if this build script changes; the rustc version is
    // baked at compile time and that's sufficient.
    println!("cargo:rerun-if-changed=build.rs");
}
