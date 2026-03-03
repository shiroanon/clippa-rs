use std::fs;
use std::path::PathBuf;

pub fn get_archive_dir() -> PathBuf {
    let mut path = dirs::data_dir().expect("No home directory found.");
    path.push("clippa");
    let _ = fs::create_dir_all(&path);
    path
}

pub fn get_archive_path(domain: &str) -> PathBuf {
    let mut path = get_archive_dir();
    path.push(format!("archive_{}.txt", domain));
    path
}
