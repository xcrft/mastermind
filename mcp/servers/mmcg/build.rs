use std::ffi::OsStr;

const WINDOWS_STACK_RESERVE_BYTES: usize = 8 * 1024 * 1024;

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_ENV");

    let target_os = std::env::var_os("CARGO_CFG_TARGET_OS");
    let target_env = std::env::var_os("CARGO_CFG_TARGET_ENV");
    if target_os.as_deref() == Some(OsStr::new("windows"))
        && target_env.as_deref() == Some(OsStr::new("msvc"))
    {
        println!("cargo:rustc-link-arg-bin=mmcg=/STACK:{WINDOWS_STACK_RESERVE_BYTES}");
    }
}
