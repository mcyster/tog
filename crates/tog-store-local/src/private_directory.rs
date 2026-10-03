use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(crate) fn create(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}
