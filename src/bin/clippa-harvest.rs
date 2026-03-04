use dirs;
use notify_rust::Notification;
use regex::Regex;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use tokio::signal;
use tokio::time::{Duration, sleep};
use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents};

/// Dispatches a system notification when a new link is secured.
fn alert_harvest(domain: &str) {
    let _ = Notification::new()
        .summary("Link Archived")
        .body(&format!("Stored under {}", domain))
        .show();
}

/// Sanitizes the domain for use as a filename.
fn get_clean_domain(url: &str, re: &Regex) -> Option<String> {
    re.captures(url)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().replace('.', "_"))
}

/// Resolves the storage path in ~/.local/share/clippa/
fn get_archive_path(domain: &str) -> PathBuf {
    let mut path = dirs::data_dir().expect("The system has no home. Persistence failed.");
    path.push("clippa");

    // Ensure the ritual site exists
    let _ = fs::create_dir_all(&path);

    path.push(format!("archive_{}.txt", domain));
    path
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut seen_urls: HashSet<String> = HashSet::new();

    // Pre-populate seen_urls from all existing archive files so we never
    // re-store a URL that was already saved in a previous session.
    let archive_dir = dirs::data_dir()
        .map(|mut p| { p.push("clippa"); p })
        .unwrap_or_default();
    if let Ok(entries) = std::fs::read_dir(&archive_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                for line in content.lines().filter(|l| !l.is_empty()) {
                    seen_urls.insert(line.to_string());
                }
            }
        }
    }

    // Patterns compiled once for the life of the process.
    let url_re = Regex::new(r"^https?://").unwrap();
    let domain_re = Regex::new(r"https?://(?:www\.)?([^/:]+)").unwrap();

    println!("Clippa-rs: Persistence initialized. Monitoring Niri seat.");
    println!("Storage: ~/.local/share/clippa/");

    loop {
        tokio::select! {
            // Primary logic: Poll the Wayland clipboard
            _ = async {
                let result = get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Text);

                if let Ok((mut reader, _)) = result {
                    let mut buffer = String::new();
                    if reader.read_to_string(&mut buffer).is_ok() {
                        let current_clip = buffer.trim();

                        // Check for URL structure and redundancy
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
                                    }
                                }
                            }
                        }
                    }
                }
                // Breathe. Prevents CPU exhaustion in the void.
                sleep(Duration::from_secs(1)).await;
            } => {},

            // Termination logic: Graceful exit on Ctrl+C or Systemd stop
            _ = signal::ctrl_c() => {
                println!("\nShutdown signal received. The archive is sealed.");
                break;
            }
        }
    }

    Ok(())
}
