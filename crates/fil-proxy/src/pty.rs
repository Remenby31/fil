use crate::ipc::{self, DaemonConnection, ProxyRegistration};
use anyhow::{Context, Result};
use fil_protocol::ipc::{DaemonMessage, ProxyMessage};
use nix::libc;
use nix::pty::{OpenptyResult, openpty};
use nix::sys::signal::{self, Signal};
use nix::sys::wait::{self, WaitPidFlag, WaitStatus};
use nix::unistd::{self, ForkResult, Pid};
use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

const DAEMON_RECONNECT_INTERVAL: Duration = Duration::from_secs(1);

pub struct PtyProcess {
    pub master_fd: i32,
    pub child_pid: i32,
}

pub fn detect_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

fn get_window_size() -> libc::winsize {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if ws.ws_col == 0 {
        ws.ws_col = 80;
        ws.ws_row = 24;
    }
    ws
}

pub fn current_window_dimensions() -> (u32, u32) {
    let ws = get_window_size();
    (u32::from(ws.ws_col), u32::from(ws.ws_row))
}

pub fn spawn_pty(shell: &str) -> Result<PtyProcess> {
    let ws = get_window_size();

    let OpenptyResult { master, slave } = openpty(None, None).context("failed to open PTY pair")?;

    unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, &ws) };

    match unsafe { unistd::fork() }.context("failed to fork")? {
        ForkResult::Child => {
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

            let shell_cstr =
                CString::new(shell).unwrap_or_else(|_| CString::new("/bin/zsh").unwrap());
            let shell_basename = shell.rsplit('/').next().unwrap_or(shell);
            let argv0 = CString::new(format!("-{shell_basename}"))
                .unwrap_or_else(|_| CString::new("-zsh").unwrap());

            unistd::execvp(&shell_cstr, &[&argv0]).context("failed to exec shell")?;

            unreachable!()
        }
        ForkResult::Parent { child } => {
            drop(slave);
            let master_raw = master.as_raw_fd();
            std::mem::forget(master);

            Ok(PtyProcess {
                master_fd: master_raw,
                child_pid: child.as_raw(),
            })
        }
    }
}

/// Synchronous poll-based proxy loop with optional daemon IPC.
/// The daemon connection is non-blocking and strictly best-effort.
/// If None, the proxy runs in offline mode (local terminal only).
pub fn proxy_loop_sync(
    pty: &PtyProcess,
    mut daemon: Option<DaemonConnection>,
    registration: Option<&ProxyRegistration>,
) -> Result<i32> {
    let master_fd = pty.master_fd;

    write_all(libc::STDOUT_FILENO, b"\x1b[5 q");
    let child_pid = Pid::from_raw(pty.child_pid);

    let (signal_read, signal_write) =
        nix::unistd::pipe().context("failed to create signal pipe")?;
    set_nonblocking(signal_read.as_raw_fd()).context("failed to configure signal pipe")?;
    set_nonblocking(signal_write.as_raw_fd()).context("failed to configure signal pipe")?;

    SIGNAL_PIPE.store(signal_write.as_raw_fd(), Ordering::SeqCst);
    RESIZE_PENDING.store(false, Ordering::SeqCst);
    TERMINATION_SIGNAL.store(0, Ordering::SeqCst);

    for watched_signal in [
        Signal::SIGWINCH,
        Signal::SIGINT,
        Signal::SIGTERM,
        Signal::SIGHUP,
        Signal::SIGQUIT,
    ] {
        unsafe {
            signal::sigaction(
                watched_signal,
                &signal::SigAction::new(
                    signal::SigHandler::Handler(signal_handler),
                    signal::SaFlags::SA_RESTART,
                    signal::SigSet::empty(),
                ),
            )
        }
        .with_context(|| format!("failed to install {watched_signal:?} handler"))?;
    }

    let mut pollfds = [
        libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: master_fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: signal_read.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: -1,
            events: libc::POLLIN,
            revents: 0,
        },
    ];

    let mut buf = [0u8; 16384];
    let mut remote_client_attached = false;
    let mut local_size = get_window_size();
    let mut requested_termination = 0;
    let mut next_daemon_reconnect = Instant::now();

    'proxy: loop {
        if daemon.is_none()
            && let Some(registration) = registration
            && Instant::now() >= next_daemon_reconnect
        {
            daemon = ipc::connect_and_register(
                registration,
                u32::from(local_size.ws_col),
                u32::from(local_size.ws_row),
            );
            next_daemon_reconnect = Instant::now() + DAEMON_RECONNECT_INTERVAL;
        }

        pollfds[3].fd = daemon.as_ref().map_or(-1, DaemonConnection::fd);
        pollfds[3].events = libc::POLLIN;
        if daemon.as_ref().is_some_and(DaemonConnection::wants_write) {
            pollfds[3].events |= libc::POLLOUT;
        }

        let timeout = daemon_poll_timeout(&daemon, registration, next_daemon_reconnect);
        let ret =
            unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as libc::nfds_t, timeout) };

        if ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                if let Some(signal) = take_termination_signal() {
                    requested_termination = signal;
                    break;
                }
                continue;
            }
            break;
        }

        // Signals use a non-blocking self-pipe; atomics preserve events even if
        // the pipe is already full.
        if pollfds[2].revents & libc::POLLIN != 0 {
            drain_fd(signal_read.as_raw_fd());
        }

        if RESIZE_PENDING.swap(false, Ordering::SeqCst) {
            let ws = get_window_size();
            local_size = ws;

            if !remote_client_attached {
                apply_window_size(master_fd, ws);
            }

            let message = ProxyMessage::Resize {
                cols: ws.ws_col,
                rows: ws.ws_row,
            };
            if !queue_daemon(&mut daemon, &message) {
                disconnect_daemon(
                    &mut daemon,
                    &mut remote_client_attached,
                    master_fd,
                    local_size,
                );
            }
        }

        if let Some(signal) = take_termination_signal() {
            requested_termination = signal;
            break 'proxy;
        }

        // Flush/read daemon IPC before accepting more PTY output. Both socket
        // paths are non-blocking, so daemon backpressure cannot hold the PTY.
        let daemon_events = pollfds[3].revents;
        if daemon_events & libc::POLLOUT != 0 {
            let flush_failed = daemon
                .as_mut()
                .is_some_and(|connection| connection.flush().is_err());
            if flush_failed {
                disconnect_daemon(
                    &mut daemon,
                    &mut remote_client_attached,
                    master_fd,
                    local_size,
                );
            }
        }

        if daemon_events & libc::POLLIN != 0 {
            let read_result = daemon.as_mut().map(DaemonConnection::read_available);
            match read_result {
                Some(Ok(messages)) => {
                    for message in messages {
                        match message {
                            DaemonMessage::Input(data) => write_all(master_fd, &data),
                            DaemonMessage::Resize { cols, rows } => {
                                if let Some(ws) = window_size_from_dimensions(cols, rows) {
                                    apply_window_size(master_fd, ws);
                                }
                            }
                            DaemonMessage::ClientAttached => {
                                remote_client_attached = true;
                            }
                            DaemonMessage::ClientDetached => {
                                // Mirror the guard in disconnect_daemon(): only
                                // restore the local size if a remote client had
                                // actually taken it over. An unsolicited detach
                                // must never SIGWINCH the user's shell.
                                if remote_client_attached {
                                    remote_client_attached = false;
                                    apply_window_size(master_fd, local_size);
                                }
                            }
                        }
                    }
                }
                Some(Err(_)) => disconnect_daemon(
                    &mut daemon,
                    &mut remote_client_attached,
                    master_fd,
                    local_size,
                ),
                None => {}
            }
        }

        if daemon_events & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            disconnect_daemon(
                &mut daemon,
                &mut remote_client_attached,
                master_fd,
                local_size,
            );
        }

        // stdin → master (user input)
        if pollfds[0].revents & libc::POLLIN != 0 {
            let n =
                unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n <= 0 {
                break;
            }
            write_all(master_fd, &buf[..n as usize]);
        }

        // master → stdout (shell output) + tee to daemon
        if pollfds[1].revents & libc::POLLIN != 0 {
            let n = unsafe { libc::read(master_fd, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n <= 0 {
                break;
            }
            let data = &buf[..n as usize];
            write_all(libc::STDOUT_FILENO, data);

            let message = ProxyMessage::Output(data.to_vec());
            if !queue_daemon(&mut daemon, &message) {
                disconnect_daemon(
                    &mut daemon,
                    &mut remote_client_attached,
                    master_fd,
                    local_size,
                );
            }
        }

        // Check for hangup on master (child exited)
        if pollfds[1].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            loop {
                let n = unsafe { libc::read(master_fd, buf.as_mut_ptr() as *mut _, buf.len()) };
                if n <= 0 {
                    break;
                }
                write_all(libc::STDOUT_FILENO, &buf[..n as usize]);
            }
            break;
        }

        if pollfds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            break;
        }
    }

    if let Some(connection) = daemon.as_mut() {
        let _ = connection.queue(&ProxyMessage::Destroyed { exit_code: 0 });
        let _ = connection.flush();
    }
    drop(daemon);

    // Clean up
    SIGNAL_PIPE.store(-1, Ordering::SeqCst);
    unsafe {
        libc::close(master_fd);
    }

    let child_exit_code = match wait::waitpid(child_pid, Some(WaitPidFlag::WNOHANG)) {
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

    if requested_termination != 0 {
        Ok(128 + requested_termination)
    } else {
        Ok(child_exit_code)
    }
}

fn daemon_poll_timeout(
    daemon: &Option<DaemonConnection>,
    registration: Option<&ProxyRegistration>,
    next_reconnect: Instant,
) -> i32 {
    if daemon.is_some() || registration.is_none() {
        return -1;
    }

    let millis = next_reconnect
        .saturating_duration_since(Instant::now())
        .as_millis();
    i32::try_from(millis).unwrap_or(i32::MAX)
}

fn write_all(fd: i32, data: &[u8]) {
    let mut written = 0;
    while written < data.len() {
        let n = unsafe {
            libc::write(
                fd,
                data[written..].as_ptr() as *const _,
                data.len() - written,
            )
        };
        if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        if n <= 0 {
            break;
        }
        written += n as usize;
    }
}

fn window_size_from_dimensions(cols: u16, rows: u16) -> Option<libc::winsize> {
    if cols == 0 || rows == 0 {
        return None;
    }
    Some(libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    })
}

fn apply_window_size(master_fd: i32, ws: libc::winsize) {
    // A no-op resize is still a SIGWINCH, and an inner ssh/tmux/TUI redraws on
    // every one of them. Skip when the PTY already has these dimensions.
    if current_window_size(master_fd)
        .is_some_and(|cur| cur.ws_row == ws.ws_row && cur.ws_col == ws.ws_col)
    {
        return;
    }
    // TIOCSWINSZ already makes the kernel raise SIGWINCH on the tty's
    // foreground process group when the dimensions actually change. Sending it
    // again by hand delivered two signals per resize, so an inner ssh/tmux
    // reflowed twice. The kernel's own signal also targets the foreground job
    // rather than only the shell, which is what we want.
    unsafe { libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws) };
}

fn current_window_size(master_fd: i32) -> Option<libc::winsize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(master_fd, libc::TIOCGWINSZ, &mut ws) };
    (rc == 0).then_some(ws)
}

fn queue_daemon(daemon: &mut Option<DaemonConnection>, message: &ProxyMessage) -> bool {
    match daemon.as_mut() {
        Some(connection) => connection.queue(message).is_ok(),
        None => true,
    }
}

fn disconnect_daemon(
    daemon: &mut Option<DaemonConnection>,
    remote_client_attached: &mut bool,
    master_fd: i32,
    local_size: libc::winsize,
) {
    *daemon = None;
    if *remote_client_attached {
        *remote_client_attached = false;
        apply_window_size(master_fd, local_size);
    }
}

fn set_nonblocking(fd: i32) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn drain_fd(fd: i32) {
    let mut buf = [0u8; 64];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n > 0 {
            continue;
        }
        if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        break;
    }
}

fn take_termination_signal() -> Option<i32> {
    match TERMINATION_SIGNAL.swap(0, Ordering::SeqCst) {
        0 => None,
        signal => Some(signal),
    }
}

static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);
static RESIZE_PENDING: AtomicBool = AtomicBool::new(false);
static TERMINATION_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn signal_handler(signal: libc::c_int) {
    if signal == libc::SIGWINCH {
        RESIZE_PENDING.store(true, Ordering::SeqCst);
    } else {
        let _ = TERMINATION_SIGNAL.compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst);
    }

    let fd = SIGNAL_PIPE.load(Ordering::SeqCst);
    if fd >= 0 {
        unsafe {
            let byte: u8 = 1;
            libc::write(fd, &byte as *const u8 as *const _, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_proxy_wakes_for_daemon_reconnect() {
        let registration = ProxyRegistration::new(
            "session".to_string(),
            "/bin/zsh".to_string(),
            "/tmp".to_string(),
        );
        let timeout = daemon_poll_timeout(
            &None,
            Some(&registration),
            Instant::now() + Duration::from_millis(100),
        );

        assert!((0..=100).contains(&timeout));
        assert_eq!(daemon_poll_timeout(&None, None, Instant::now()), -1);
    }
}
