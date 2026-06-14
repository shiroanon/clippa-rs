use sha2::{Digest, Sha256};
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

pub fn get_screenshot_dir() -> PathBuf {
    let mut path = get_archive_dir();
    path.push("screenshots");
    let _ = fs::create_dir_all(&path);
    path
}

pub fn screenshot_path(url: &str) -> PathBuf {
    let hash = url_hash(url);
    let mut path = get_screenshot_dir();
    path.push(format!("{}.png", hash));
    path
}

pub fn url_hash(url: &str) -> String {
    let hash = Sha256::digest(url.as_bytes());
    hash[..8].iter().map(|b| format!("{:02x}", b)).collect()
}
