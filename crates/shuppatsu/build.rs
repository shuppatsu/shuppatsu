//! Windows (GNU toolchain) only: compiles `shuppatsu.rc` (the exe icon,
//! `assets/logo/gen/shuppatsu.ico`) with MinGW's `windres` and links the resource object into
//! the executable. Skipped with a warning when `windres` is missing or fails; the game then
//! has no exe icon but its window icon is unaffected. Other targets do nothing.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    if var("CARGO_CFG_TARGET_OS") != "windows" {
        return;
    }
    if var("CARGO_CFG_TARGET_ENV") != "gnu" {
        println!("cargo:warning=no exe icon: only the windows-gnu build embeds it (windres)");
        return;
    }
    let manifest = PathBuf::from(var("CARGO_MANIFEST_DIR"));
    let rc = manifest.join("shuppatsu.rc");
    let icons = manifest.join("../../assets/logo/gen");
    println!("cargo:rerun-if-changed={}", rc.display());
    println!(
        "cargo:rerun-if-changed={}",
        icons.join("shuppatsu.ico").display()
    );
    println!("cargo:rerun-if-env-changed=WINDRES");
    // The dev shell's cross binutils are prefixed by the target: x86_64-w64-mingw32-windres.
    let windres = std::env::var("WINDRES")
        .unwrap_or_else(|_| format!("{}-w64-mingw32-windres", var("CARGO_CFG_TARGET_ARCH")));
    let obj = PathBuf::from(var("OUT_DIR")).join("shuppatsu-rc.o");
    let status = Command::new(&windres)
        .arg("--input")
        .arg(&rc)
        .arg("--input-format=rc")
        .arg("--output-format=coff")
        .arg("--include-dir")
        .arg(&icons)
        .arg("--output")
        .arg(&obj)
        .status();
    match status {
        Ok(s) if s.success() => {
            // Every linked target of this package (the game and its test binary, which
            // checks the resource).
            println!("cargo:rustc-link-arg={}", obj.display());
        }
        Ok(s) => println!("cargo:warning=no exe icon: {windres} failed ({s})"),
        Err(e) => println!("cargo:warning=no exe icon: cannot run {windres}: {e}"),
    }
}
