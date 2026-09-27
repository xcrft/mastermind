//! Cooperative foreground cancellation. Signal handlers only set an atomic;
//! normal Rust unwinding owns processor termination and lease release.

use super::Error;
use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub(super) fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

#[cfg(unix)]
extern "C" fn interrupt(_signal: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

pub(super) struct Cancellation {
    #[cfg(unix)]
    previous: [libc::sigaction; 2],
}

impl Cancellation {
    #[cfg(unix)]
    pub(super) fn install() -> Result<Self, Error> {
        // SAFETY: zeroed sigaction structs are initialized through libc before
        // registration. The handler only writes a lock-free atomic boolean.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            let mut previous: [libc::sigaction; 2] = std::mem::zeroed();
            action.sa_sigaction = interrupt as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            INTERRUPTED.store(false, Ordering::Relaxed);
            if libc::sigaction(libc::SIGINT, &action, &mut previous[0]) != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if libc::sigaction(libc::SIGTERM, &action, &mut previous[1]) != 0 {
                let error = std::io::Error::last_os_error();
                libc::sigaction(libc::SIGINT, &previous[0], std::ptr::null_mut());
                return Err(error.into());
            }
            Ok(Self { previous })
        }
    }

    #[cfg(not(unix))]
    pub(super) fn install() -> Result<Self, Error> {
        Err("continuous semantic processing requires Unix process supervision".into())
    }
}

impl Drop for Cancellation {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: restore the handlers saved by this foreground CLI command.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.previous[0], std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &self.previous[1], std::ptr::null_mut());
        }
        INTERRUPTED.store(false, Ordering::Relaxed);
    }
}
