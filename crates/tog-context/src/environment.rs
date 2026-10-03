use std::path::{Path, PathBuf};

mod from_process;

pub struct Environment {
    data_directory: PathBuf,
}

impl Environment {
    pub fn data_directory(&self) -> &Path {
        &self.data_directory
    }
}
