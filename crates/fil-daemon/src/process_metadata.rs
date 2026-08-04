use std::ffi::CStr;
use std::mem::{size_of, size_of_val, zeroed};
use std::os::fd::AsRawFd;
#[cfg(target_os = "macos")]
use std::ptr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessMetadata {
    pub created_at: i64,
    pub cwd: String,
    pub command: String,
}

pub fn peer_pid(stream: &tokio::net::UnixStream) -> Option<i32> {
    #[cfg(target_os = "macos")]
    unsafe {
        let mut pid: libc::pid_t = 0;
        let mut len = size_of::<libc::pid_t>() as libc::socklen_t;
        let result = libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            &mut pid as *mut _ as *mut libc::c_void,
            &mut len,
        );
        (result == 0 && pid > 0).then_some(pid)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = stream;
        None
    }
}

pub fn snapshot(peer_pid: i32, fallback_cwd: &str, fallback_shell: &str) -> ProcessMetadata {
    #[cfg(target_os = "macos")]
    {
        let proxy_info = bsd_info(peer_pid);
        let created_at = proxy_info
            .as_ref()
            .and_then(|info| i64::try_from(info.pbi_start_tvsec).ok())
            .filter(|timestamp| *timestamp > 0)
            .unwrap_or_else(|| chrono::Utc::now().timestamp());

        let children = child_pids(peer_pid);
        let shell_pid = children
            .iter()
            .copied()
            .find(|pid| is_shell_name(&process_name(*pid)))
            .or_else(|| children.first().copied());

        let Some(shell_pid) = shell_pid else {
            return ProcessMetadata {
                created_at,
                cwd: fallback_cwd.to_string(),
                command: shell_basename(fallback_shell),
            };
        };

        let shell_info = bsd_info(shell_pid);
        let foreground_group = shell_info.as_ref().map_or(0, |info| info.e_tpgid as i32);
        let descendants = descendant_pids(shell_pid);
        let active_pid = descendants
            .iter()
            .copied()
            .find(|pid| *pid == foreground_group)
            .or_else(|| {
                descendants.iter().copied().find(|pid| {
                    bsd_info(*pid).is_some_and(|info| info.pbi_pgid as i32 == foreground_group)
                })
            })
            .filter(|_| {
                shell_info.as_ref().is_some_and(|info| {
                    foreground_group > 0 && foreground_group != info.pbi_pgid as i32
                })
            });

        let metadata_pid = active_pid.unwrap_or(shell_pid);
        let cwd = process_cwd(metadata_pid)
            .or_else(|| process_cwd(shell_pid))
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| fallback_cwd.to_string());
        let command = process_command(metadata_pid);

        ProcessMetadata {
            created_at,
            cwd,
            command: if command.is_empty() {
                shell_basename(fallback_shell)
            } else {
                command
            },
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = peer_pid;
        ProcessMetadata {
            created_at: chrono::Utc::now().timestamp(),
            cwd: fallback_cwd.to_string(),
            command: shell_basename(fallback_shell),
        }
    }
}

fn shell_basename(shell: &str) -> String {
    shell
        .rsplit('/')
        .next()
        .unwrap_or(shell)
        .trim_start_matches('-')
        .to_string()
}

fn is_shell_name(name: &str) -> bool {
    matches!(
        name.trim_start_matches('-'),
        "zsh" | "bash" | "fish" | "sh" | "dash"
    )
}

fn normalize_command(name: &str) -> String {
    let executable = name
        .split_whitespace()
        .next()
        .unwrap_or(name)
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .trim_start_matches('-');
    let lowercase = executable.to_ascii_lowercase();
    if lowercase == "codex" || lowercase.starts_with("codex-") {
        "codex".to_string()
    } else if lowercase == "claude" || lowercase.starts_with("claude-") {
        "claude".to_string()
    } else {
        executable.to_string()
    }
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: i32) -> Option<libc::proc_bsdinfo> {
    unsafe {
        let mut info: libc::proc_bsdinfo = zeroed();
        let size = size_of::<libc::proc_bsdinfo>();
        let written = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size as i32,
        );
        (written as usize == size).then_some(info)
    }
}

#[cfg(target_os = "macos")]
fn child_pids(parent: i32) -> Vec<i32> {
    unsafe {
        let mut pids = [0 as libc::pid_t; 256];
        let returned = libc::proc_listchildpids(
            parent,
            pids.as_mut_ptr() as *mut libc::c_void,
            size_of_val(&pids) as i32,
        );
        if returned <= 0 {
            return Vec::new();
        }
        pids.into_iter().filter(|pid| *pid > 0).collect()
    }
}

#[cfg(target_os = "macos")]
fn descendant_pids(root: i32) -> Vec<i32> {
    let mut result = Vec::new();
    let mut pending = child_pids(root);
    while let Some(pid) = pending.pop() {
        if result.len() >= 256 {
            break;
        }
        pending.extend(child_pids(pid));
        result.push(pid);
    }
    result
}

#[cfg(target_os = "macos")]
fn process_name(pid: i32) -> String {
    unsafe {
        let mut buffer = [0 as libc::c_char; 256];
        let written = libc::proc_name(
            pid,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len() as u32,
        );
        if written <= 0 {
            return String::new();
        }
        CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(target_os = "macos")]
fn process_command(pid: i32) -> String {
    process_argv0(pid)
        .map(|value| normalize_command(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| normalize_command(&process_name(pid)))
}

#[cfg(target_os = "macos")]
fn process_argv0(pid: i32) -> Option<String> {
    unsafe {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size = 0usize;
        if libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            ptr::null_mut(),
            &mut size,
            ptr::null_mut(),
            0,
        ) != 0
            || size <= size_of::<libc::c_int>()
        {
            return None;
        }

        let mut buffer = vec![0u8; size];
        if libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            buffer.as_mut_ptr() as *mut libc::c_void,
            &mut size,
            ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        buffer.truncate(size);

        let mut index = size_of::<libc::c_int>();
        while index < buffer.len() && buffer[index] != 0 {
            index += 1;
        }
        while index < buffer.len() && buffer[index] == 0 {
            index += 1;
        }
        let start = index;
        while index < buffer.len() && buffer[index] != 0 {
            index += 1;
        }
        (start < index).then(|| String::from_utf8_lossy(&buffer[start..index]).into_owned())
    }
}

#[cfg(target_os = "macos")]
fn process_cwd(pid: i32) -> Option<String> {
    unsafe {
        let mut info: libc::proc_vnodepathinfo = zeroed();
        let size = size_of::<libc::proc_vnodepathinfo>();
        let written = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size as i32,
        );
        if written as usize != size {
            return None;
        }
        Some(
            CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_names_are_normalized() {
        assert_eq!(shell_basename("/bin/zsh"), "zsh");
        assert_eq!(shell_basename("-bash"), "bash");
        assert!(is_shell_name("zsh"));
        assert!(!is_shell_name("codex"));
        assert_eq!(normalize_command("codex-aarch64-apple-darwin"), "codex");
        assert_eq!(normalize_command("/usr/local/bin/claude-code"), "claude");
    }
}
