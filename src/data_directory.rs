use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(crate) fn from_environment() -> io::Result<PathBuf> {
    if let Some(configured_directory) = std::env::var_os("TOG_DATA_DIR") {
        Ok(PathBuf::from(configured_directory))
    } else if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        Ok(PathBuf::from(data_home).join("tog"))
    } else if let Some(home_directory) = std::env::var_os("HOME") {
        Ok(PathBuf::from(home_directory).join(".local/share/tog"))
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "TOG_DATA_DIR, XDG_DATA_HOME, or HOME must be set",
        ))
    }
}

pub(crate) fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}
