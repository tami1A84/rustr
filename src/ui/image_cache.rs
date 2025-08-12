use std::fs;
use std::io::Write;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

fn get_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|mut path| {
        path.push("N"); // App-specific cache folder
        path.push("images");
        if !path.exists() {
            fs::create_dir_all(&path).ok();
        }
        path
    })
}

fn url_to_filename(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

pub fn load_from_disk(url: &str) -> Option<Vec<u8>> {
    if let Some(cache_dir) = get_cache_dir() {
        let filename = url_to_filename(url);
        let path = cache_dir.join(filename);
        if path.exists() {
            return fs::read(path).ok();
        }
    }
    None
}

pub fn save_to_disk(url: &str, data: &[u8]) {
    if let Some(cache_dir) = get_cache_dir() {
        let filename = url_to_filename(url);
        let path = cache_dir.join(filename);
        if let Ok(mut file) = fs::File::create(path) {
            file.write_all(data).ok();
        }
    }
}
