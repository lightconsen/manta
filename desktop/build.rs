fn main() {
    tauri_build::build();
    // Android: the cdylib's unwind tables (from cc-compiled C deps) reference
    // `__gxx_personality_v0`, which lives in libc++. Linking `-lc++_shared`
    // records it in DT_NEEDED so tauri-cli bundles libc++_shared.so into the
    // APK and the Android linker resolves the symbol at dlopen time.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-arg=-lc++_shared");
    }
}
