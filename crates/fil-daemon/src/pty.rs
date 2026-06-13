use anyhow::{Context, Result};
use nix::libc;
use nix::pty::{openpty, OpenptyResult};
use nix::sys::signal::{self, Signal};
use nix::sys::wait::{self, WaitPidFlag, WaitStatus};
use nix::unistd::{self, ForkResult, Pid};
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use tokio::io::unix::AsyncFd;
use tokio::signal::unix::{signal as tokio_signal, SignalKind};
use tracing::debug;

pub struct PtyProcess {
    pub master_fd: OwnedFd,
    pub child_pid: i32,
}

pub fn detect_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

fn get_window_size() -> libc::winsize {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::ioctl(libc::STDIN_FILENO, u64::from(libc::TIOCGWINSZ), &mut ws) };
    if ret != 0 || ws.ws_col == 0 {
        ws.ws_col = 80;
        ws.ws_row = 24;
    }
    ws
}

fn set_window_size(fd: i32, ws: &libc::winsize) {
    unsafe {
        libc::ioctl(fd, u64::from(libc::TIOCSWINSZ), ws);
    }
}

pub fn spawn_pty(shell: &str) -> Result<PtyProcess> {
    let ws = get_window_size();

    let OpenptyResult { master, slave } = openpty(None, None)
        .context("failed to open PTY pair")?;

    set_window_size(slave.as_raw_fd(), &ws);

    match unsafe { unistd::fork() }.context("failed to fork")? {
        ForkResult::Child => {
            drop(master);

            unistd::setsid().ok();

            unsafe {
                libc::ioctl(slave.as_raw_fd(), u64::from(libc::TIOCSCTTY), 0);
            }

            let slave_raw = slave.as_raw_fd();
            unistd::dup2(slave_raw, libc::STDIN_FILENO).ok();
            unistd::dup2(slave_raw, libc::STDOUT_FILENO).ok();
            unistd::dup2(slave_raw, libc::STDERR_FILENO).ok();

            if slave_raw > 2 {
                drop(slave);
            }

            if std::env::var("TERM").is_err() {
                unsafe { std::env::set_var("TERM", "xterm-256color") };
            }

            let shell_cstr = CString::new(shell)
                .unwrap_or_else(|_| CString::new("/bin/zsh").unwrap());
            let shell_basename = shell.rsplit('/').next().unwrap_or(shell);
            let argv0 = CString::new(format!("-{shell_basename}"))
                .unwrap_or_else(|_| CString::new("-zsh").unwrap());

            unistd::execvp(&shell_cstr, &[&argv0])
                .context("failed to exec shell")?;

            unreachable!()
        }
        ForkResult::Parent { child } => {
            drop(slave);

            Ok(PtyProcess {
                master_fd: master,
                child_pid: child.as_raw(),
            })
        }
    }
}

pub async fn proxy_loop(pty: PtyProcess) -> Result<i32> {
    let master_raw_fd = pty.master_fd.as_raw_fd();
    let child_pid = Pid::from_raw(pty.child_pid);

    // Set master fd to non-blocking for async I/O
    let flags = unsafe { libc::fcntl(master_raw_fd, libc::F_GETFL) };
    unsafe { libc::fcntl(master_raw_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };

    // Set stdin to non-blocking
    let stdin_flags = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_GETFL) };
    unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, stdin_flags | libc::O_NONBLOCK) };

    let master_async = AsyncFd::new(pty.master_fd)?;
    let stdin_async = AsyncFd::new(unsafe { OwnedFd::from_raw_fd(libc::STDIN_FILENO) })?;

    let mut sigwinch = tokio_signal(SignalKind::window_change())?;
    let mut sigchld = tokio_signal(SignalKind::child())?;

    let exit_code: i32;

    loop {
        tokio::select! {
            // stdin → PTY master (user input)
            readable = stdin_async.readable() => {
                let mut guard = readable?;
                match guard.try_io(|fd| {
                    let mut buf = [0u8; 8192];
                    let raw = fd.as_raw_fd();
                    let n = unsafe { libc::read(raw, buf.as_mut_ptr() as *mut _, buf.len()) };
                    if n < 0 {
                        Err(std::io::Error::last_os_error())
                    } else if n == 0 {
                        Ok(0usize)
                    } else {
                        let master_raw = master_async.as_raw_fd();
                        let written = unsafe { libc::write(master_raw, buf.as_ptr() as *const _, n as usize) };
                        if written < 0 {
                            Err(std::io::Error::last_os_error())
                        } else {
                            Ok(n as usize)
                        }
                    }
                }) {
                    Ok(Ok(0)) => {
                        debug!("stdin EOF");
                        break;
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Ok(Err(e)) => {
                        debug!(error = %e, "stdin read error");
                        break;
                    }
                    Err(_would_block) => {}
                }
            }

            // PTY master → stdout (shell output)
            readable = master_async.readable() => {
                let mut guard = readable?;
                match guard.try_io(|fd| {
                    let mut buf = [0u8; 8192];
                    let raw = fd.as_raw_fd();
                    let n = unsafe { libc::read(raw, buf.as_mut_ptr() as *mut _, buf.len()) };
                    if n < 0 {
                        Err(std::io::Error::last_os_error())
                    } else if n == 0 {
                        Ok(0usize)
                    } else {
                        let written = unsafe { libc::write(libc::STDOUT_FILENO, buf.as_ptr() as *const _, n as usize) };
                        if written < 0 {
                            Err(std::io::Error::last_os_error())
                        } else {
                            Ok(n as usize)
                        }
                    }
                }) {
                    Ok(Ok(0)) => {
                        debug!("PTY master EOF");
                        break;
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Ok(Err(e)) if e.raw_os_error() == Some(libc::EIO) => {
                        // EIO means the slave side was closed (child exited)
                        debug!("PTY master EIO — child exited");
                        break;
                    }
                    Ok(Err(e)) => {
                        debug!(error = %e, "PTY master read error");
                        break;
                    }
                    Err(_would_block) => {}
                }
            }

            _ = sigwinch.recv() => {
                let ws = get_window_size();
                set_window_size(master_async.as_raw_fd(), &ws);
                debug!(cols = ws.ws_col, rows = ws.ws_row, "propagated resize");
            }

            _ = sigchld.recv() => {
                debug!("received SIGCHLD");
            }
        }
    }

    // Reap the child process
    exit_code = match wait::waitpid(child_pid, Some(WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::Exited(_, code)) => code,
        Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
        _ => {
            signal::kill(child_pid, Signal::SIGHUP).ok();
            match wait::waitpid(child_pid, None) {
                Ok(WaitStatus::Exited(_, code)) => code,
                Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
                _ => 1,
            }
        }
    };

    // Don't close stdin — it's not ours
    let stdin_fd = stdin_async.into_inner();
    std::mem::forget(stdin_fd);

    Ok(exit_code)
}
