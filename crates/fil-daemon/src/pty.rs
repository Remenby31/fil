use anyhow::{Context, Result};
use nix::libc;
use nix::pty::{openpty, OpenptyResult};
use nix::sys::signal::{self, Signal};
use nix::sys::wait::{self, WaitPidFlag, WaitStatus};
use nix::unistd::{self, ForkResult, Pid};
use std::ffi::CString;
use std::os::fd::AsRawFd;

pub struct PtyProcess {
    pub master_fd: i32,
    pub child_pid: i32,
}

pub fn detect_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

fn get_window_size() -> libc::winsize {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    unsafe { libc::ioctl(libc::STDIN_FILENO, u64::from(libc::TIOCGWINSZ), &mut ws) };
    if ws.ws_col == 0 {
        ws.ws_col = 80;
        ws.ws_row = 24;
    }
    ws
}

pub fn spawn_pty(shell: &str) -> Result<PtyProcess> {
    let ws = get_window_size();

    let OpenptyResult { master, slave } = openpty(None, None)
        .context("failed to open PTY pair")?;

    // Set window size on slave
    unsafe { libc::ioctl(slave.as_raw_fd(), u64::from(libc::TIOCSWINSZ), &ws) };

    match unsafe { unistd::fork() }.context("failed to fork")? {
        ForkResult::Child => {
            // Close master in child
            drop(master);

            unistd::setsid().ok();

            let slave_raw = slave.as_raw_fd();
            unsafe { libc::ioctl(slave_raw, u64::from(libc::TIOCSCTTY), 0) };

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
            let master_raw = master.as_raw_fd();
            std::mem::forget(master); // we manage the fd manually

            Ok(PtyProcess {
                master_fd: master_raw,
                child_pid: child.as_raw(),
            })
        }
    }
}

/// Synchronous poll-based proxy loop.
/// This is the gold standard approach used by script(1) and tmux.
/// No async, no non-blocking flags on stdin/stdout.
pub fn proxy_loop_sync(pty: &PtyProcess) -> Result<i32> {
    let master_fd = pty.master_fd;
    let child_pid = Pid::from_raw(pty.child_pid);

    // Install SIGWINCH handler via self-pipe trick
    let (sigwinch_read, sigwinch_write) = nix::unistd::pipe()
        .context("failed to create signal pipe")?;

    unsafe {
        SIGWINCH_PIPE = sigwinch_write.as_raw_fd();
        signal::sigaction(
            Signal::SIGWINCH,
            &signal::SigAction::new(
                signal::SigHandler::Handler(sigwinch_handler),
                signal::SaFlags::SA_RESTART,
                signal::SigSet::empty(),
            ),
        ).ok();
    }

    let mut pollfds = [
        libc::pollfd { fd: libc::STDIN_FILENO, events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: master_fd, events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: sigwinch_read.as_raw_fd(), events: libc::POLLIN, revents: 0 },
    ];

    let mut buf = [0u8; 16384];

    loop {
        let ret = unsafe { libc::poll(pollfds.as_mut_ptr(), 3, -1) };

        if ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }

        // SIGWINCH — propagate resize
        if pollfds[2].revents & libc::POLLIN != 0 {
            let mut discard = [0u8; 1];
            unsafe { libc::read(sigwinch_read.as_raw_fd(), discard.as_mut_ptr() as *mut _, 1) };
            let ws = get_window_size();
            unsafe { libc::ioctl(master_fd, u64::from(libc::TIOCSWINSZ), &ws) };
            // Send SIGWINCH to child process group
            signal::kill(child_pid, Signal::SIGWINCH).ok();
        }

        // stdin → master (user input)
        if pollfds[0].revents & libc::POLLIN != 0 {
            let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n <= 0 {
                break;
            }
            write_all(master_fd, &buf[..n as usize]);
        }

        // master → stdout (shell output)
        if pollfds[1].revents & libc::POLLIN != 0 {
            let n = unsafe { libc::read(master_fd, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n <= 0 {
                break;
            }
            write_all(libc::STDOUT_FILENO, &buf[..n as usize]);
        }

        // Check for hangup on master (child exited)
        if pollfds[1].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            // Drain remaining output
            loop {
                let n = unsafe { libc::read(master_fd, buf.as_mut_ptr() as *mut _, buf.len()) };
                if n <= 0 { break; }
                write_all(libc::STDOUT_FILENO, &buf[..n as usize]);
            }
            break;
        }

        // Check for hangup on stdin
        if pollfds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            break;
        }
    }

    // Clean up signal pipe
    unsafe { SIGWINCH_PIPE = -1; }

    // Close master fd
    unsafe { libc::close(master_fd); }

    // Reap child
    let exit_code = match wait::waitpid(child_pid, Some(WaitPidFlag::WNOHANG)) {
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

    Ok(exit_code)
}

fn write_all(fd: i32, data: &[u8]) {
    let mut written = 0;
    while written < data.len() {
        let n = unsafe {
            libc::write(fd, data[written..].as_ptr() as *const _, data.len() - written)
        };
        if n <= 0 { break; }
        written += n as usize;
    }
}

// Signal handler for SIGWINCH — writes to self-pipe
static mut SIGWINCH_PIPE: i32 = -1;

extern "C" fn sigwinch_handler(_sig: libc::c_int) {
    unsafe {
        if SIGWINCH_PIPE >= 0 {
            let byte: u8 = 1;
            libc::write(SIGWINCH_PIPE, &byte as *const u8 as *const _, 1);
        }
    }
}
