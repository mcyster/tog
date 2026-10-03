use std::io;
use std::path::PathBuf;

pub fn data_directory() -> io::Result<PathBuf> {
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
