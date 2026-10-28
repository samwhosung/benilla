//! Stamps the commit this binary was built from, and gives the main thread the stack the app needs.

fn main() {
    benilla_buildstamp::emit();
    main_thread_stack();
}

/// The app runs on the main thread, where the windowing event loop must live, and needs more than
/// the 1 MB the MSVC linker reserves for it by default (2 MB on mingw): the login screen overflows
/// it. Linux and macOS give the main thread 8 MB, so Windows reserves the same.
fn main_thread_stack() {
    const STACK: u32 = 8 << 20;
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => println!("cargo:rustc-link-arg-bins=/STACK:{STACK}"),
        Ok("gnu") => println!("cargo:rustc-link-arg-bins=-Wl,--stack,{STACK}"),
        _ => {}
    }
}
