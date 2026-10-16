//! Helpers the harness tests share.

use std::io;
use std::path::Path;

/// Make `link` show the directory `target`: a symlink, which Windows only allows with a privilege
/// (Developer Mode or an elevated token), so there a failed link becomes a recursive copy.
pub(super) fn link_dir(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).or_else(|_| copy_dir(target, link))
    }
}

#[cfg(windows)]
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}
