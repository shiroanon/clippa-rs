use dirs;
use image::GenericImageView;
use notify_rust::Notification;
use regex::Regex;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tokio::signal;
use tokio::time::{Duration, sleep};
use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents};

fn alert_harvest(domain: &str) {
    let _ = Notification::new()
        .summary("Link Archived")
        .body(&format!("Stored under {}", domain))
        .show();
}

fn get_clean_domain(url: &str, re: &Regex) -> Option<String> {
    re.captures(url)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().replace('.', "_"))
}

fn get_archive_path(domain: &str) -> PathBuf {
    let mut path = dirs::data_dir().expect("The system has no home. Persistence failed.");
    path.push("clippa");
    let _ = fs::create_dir_all(&path);
    path.push(format!("archive_{}.txt", domain));
    path
}

fn take_screenshot(url: &str) {
    let out_path = clippa_rs::screenshot_path(url);
    if out_path.exists() {
        return;
    }

    let tmp = {
        let mut p = out_path.parent().unwrap().to_path_buf();
        p.push(format!("{}.tmp.png", clippa_rs::url_hash(url)));
        p
    };

    let ok = Command::new("grim")
        .arg(&tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if ok && tmp.exists() {
        if let Ok(img) = image::open(&tmp) {
            let (w, h) = img.dimensions();
            let max_dim = 720u32;
            let (nw, nh) = if w > h {
                (max_dim, (h * max_dim / w).max(1))
            } else {
                ((w * max_dim / h).max(1), max_dim)
            };
            let resized = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
            let _ = resized.save(&out_path);
        }
        let _ = fs::remove_file(&tmp);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut seen_urls: HashSet<String> = HashSet::new();

    let archive_dir = dirs::data_dir()
        .map(|mut p| { p.push("clippa"); p })
        .unwrap_or_default();
    if let Ok(entries) = std::fs::read_dir(&archive_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.path().extension().is_some_and(|e| e == "txt") {
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    for line in content.lines().filter(|l| !l.is_empty()) {
                        seen_urls.insert(line.to_string());
                    }
                }
            }
        }
    }

    let url_re = Regex::new(r"^https?://").unwrap();
    let domain_re = Regex::new(r"https?://(?:www\.)?([^/:]+)").unwrap();

    println!("Clippa-rs: Persistence initialized. Monitoring Niri seat.");
    println!("Storage: ~/.local/share/clippa/");

    loop {
        tokio::select! {
            _ = async {
                let result = get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Text);

                if let Ok((mut reader, _)) = result {
                    let mut buffer = String::new();
                    if reader.read_to_string(&mut buffer).is_ok() {
                        let current_clip = buffer.trim();

                        if url_re.is_match(current_clip) && !seen_urls.contains(current_clip) {
                            if let Some(domain) = get_clean_domain(current_clip, &domain_re) {
                                let archive_file = get_archive_path(&domain);

                                if let Ok(mut f) = OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(archive_file)
                                {
                                    if writeln!(f, "{}", current_clip).is_ok() {
                                        alert_harvest(&domain);
                                        seen_urls.insert(current_clip.to_string());
                                        take_screenshot(current_clip);
                                    }
                                }
                            }
                        }
                    }
                }
                sleep(Duration::from_secs(1)).await;
            } => {},

            _ = signal::ctrl_c() => {
                println!("\nShutdown signal received. The archive is sealed.");
                break;
            }
        }
    }

    Ok(())
}
