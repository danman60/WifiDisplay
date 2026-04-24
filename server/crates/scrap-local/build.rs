fn main() {
    // Fixed for cross-compile: check CARGO_CFG_TARGET_OS (set by cargo for the
    // TARGET platform) instead of cfg!(windows), which reports the HOST.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        println!("cargo:rustc-cfg=dxgi");
    } else if target_os == "macos" {
        println!("cargo:rustc-cfg=quartz");
    } else {
        println!("cargo:rustc-cfg=x11");
    }
}
