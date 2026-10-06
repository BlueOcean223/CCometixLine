use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Write `contents` to `path` through a temporary file next to it, so that a process
/// reading the file meanwhile gets the old or the new contents, never part of them.
/// Creates the directory if needed.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);
    let result = fs::write(&temp, contents).and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
