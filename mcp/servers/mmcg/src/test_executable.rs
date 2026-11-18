//! Shared by the crate's unit tests and, through `#[path]`, the integration
//! tests.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes a script that a test will run, mode 0700, creating its directory.
///
/// Linux refuses to execute a file that any process holds open for writing.
/// While a test thread writes the file, another thread can fork a child that
/// keeps the descriptor until its own exec, and running the script then fails
/// with ETXTBSY. Writing from a short-lived process keeps the descriptor out of
/// the test process.
pub fn write_executable(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut writer = Command::new("/bin/sh")
        .args(["-c", "cat > \"$0\" && chmod 700 \"$0\""])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    writer
        .stdin
        .take()
        .unwrap()
        .write_all(contents.as_ref())
        .unwrap();
    assert!(
        writer.wait().unwrap().success(),
        "could not write {}",
        path.display()
    );
}
