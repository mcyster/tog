use std::io;
use std::path::PathBuf;

use super::Environment;

impl Environment {
    pub fn try_from_process() -> io::Result<Self> {
        if let Some(configured_directory) = std::env::var_os("TOG_DATA_DIR") {
            Ok(Self {
                data_directory: PathBuf::from(configured_directory),
            })
        } else if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
            Ok(Self {
                data_directory: PathBuf::from(data_home).join("tog"),
            })
        } else if let Some(home_directory) = std::env::var_os("HOME") {
            Ok(Self {
                data_directory: PathBuf::from(home_directory).join(".local/share/tog"),
            })
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "TOG_DATA_DIR, XDG_DATA_HOME, or HOME must be set",
            ))
        }
    }
}
