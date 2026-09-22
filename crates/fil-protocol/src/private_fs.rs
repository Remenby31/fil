//! Owner-only storage for credentials and local IPC. Never follow a final symlink.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub fn directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private directory must be owned and not a symlink",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

pub fn open(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    validate(&file, true)?;
    Ok(file)
}

pub fn read(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    validate(&file, false)?;
    Ok(file)
}

/// A historical shared-path log may belong to another installation/user. Do
/// not let unrelated files, links or FIFOs disable an otherwise private daemon.
pub fn harden_owned_legacy_log(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_file()
        && metadata.nlink() == 1
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.permissions().mode() & 0o077 != 0
    {
        read(path)?;
    }
    Ok(())
}

fn validate(file: &File, writable: bool) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private file must be singly linked and owned",
        ));
    }
    let mode = if writable {
        0o600
    } else {
        metadata.permissions().mode() & 0o600
    };
    if metadata.permissions().mode() & 0o777 != mode {
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = open(path)?;
    file.set_len(0)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_files_and_legacy_symlinks_do_not_hang_or_modify_their_target() {
        let root = std::env::temp_dir().join(format!("fil-special-{}", uuid::Uuid::new_v4()));
        directory(&root).unwrap();
        let fifo = root.join("fifo");
        let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        harden_owned_legacy_log(&fifo).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read(&fifo).is_err());
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(1))
                .expect("FIFO read must not block")
        );
        let target = root.join("other");
        fs::write(&target, b"unchanged").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let link = root.join("legacy-log");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        harden_owned_legacy_log(&link).unwrap();
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
        harden_owned_legacy_log(&target).unwrap();
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn private_storage_repairs_modes_preserves_data_and_rejects_links() {
        let root = std::env::temp_dir().join(format!("fil-private-{}", uuid::Uuid::new_v4()));
        directory(&root).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let path = root.join("key");
        write(&path, b"synthetic-key").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        open(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"synthetic-key");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let link = root.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(write(&link, b"must-not-overwrite").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"synthetic-key");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        read(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o400
        );
        fs::remove_dir_all(root).unwrap();
    }
}
