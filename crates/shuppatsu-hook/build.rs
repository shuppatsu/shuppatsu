fn main() {
    // stdcall exports are decorated (`DirectInputCreateA@16`) by default; the game imports the
    // plain name, so strip the decoration from the DLL's export table.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,--kill-at");
        // The undecorated export names resolve to the decorated symbols; that is intended.
        println!("cargo:rustc-cdylib-link-arg=-Wl,--enable-stdcall-fixup");
    }
}
