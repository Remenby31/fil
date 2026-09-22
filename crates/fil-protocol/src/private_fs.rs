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
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private file must be singly linked and owned",
        ));
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
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
        fs::remove_dir_all(root).unwrap();
    }
}
