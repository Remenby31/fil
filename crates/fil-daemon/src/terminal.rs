use anyhow::Result;
use nix::sys::termios::{self, LocalFlags, SetArg, Termios};
use std::os::fd::BorrowedFd;
use tracing::debug;

/// RAII guard that puts stdin into raw mode and restores it on drop.
pub struct RawModeGuard {
    original: Termios,
}

impl RawModeGuard {
    pub fn new() -> Result<Self> {
        let stdin_fd = unsafe { BorrowedFd::borrow_raw(libc::STDIN_FILENO) };
        let original = termios::tcgetattr(stdin_fd)?;

        let mut raw = original.clone();

        // Disable canonical mode and echo — let the PTY handle everything
        termios::cfmakeraw(&mut raw);

        // But keep signal generation off — we want Ctrl+C to go to the PTY,
        // not kill fil itself
        raw.local_flags.remove(LocalFlags::ISIG);

        termios::tcsetattr(stdin_fd, SetArg::TCSANOW, &raw)?;
        debug!("terminal set to raw mode");

        Ok(Self { original })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let stdin_fd = unsafe { BorrowedFd::borrow_raw(libc::STDIN_FILENO) };
        if let Err(e) = termios::tcsetattr(stdin_fd, SetArg::TCSANOW, &self.original) {
            eprintln!("fil: failed to restore terminal: {e}");
        } else {
            debug!("terminal restored");
        }
    }
}
