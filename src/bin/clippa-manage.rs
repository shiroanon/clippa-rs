use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use ratatui::{prelude::*, widgets::*};
use ratatui_image::{StatefulImage, picker::Picker};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Result, stdout},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};
use tokio::sync::mpsc;
use wl_clipboard_rs::copy::{MimeType, Options, Source};

#[derive(PartialEq, Clone)]
enum Mode {
    BrowsingFiles,
    BrowsingLinks,
    Searching,
    ViewingImage,
}

struct App {
    files: Vec<PathBuf>,
    file_state: ListState,
    selected_files: HashSet<usize>,
    file_link_counts: Vec<usize>,
    links: Vec<String>,
    link_state: ListState,
    selected_links: HashSet<usize>,
    mode: Mode,
    prev_mode: Mode,
    should_quit: bool,
    last_modified: Option<SystemTime>,
    link_titles: HashMap<String, String>,
    title_tx: mpsc::Sender<(String, String)>,
    title_rx: mpsc::Receiver<(String, String)>,
    search_query: String,
    search_results: Vec<(String, String)>,
    search_state: ListState,
    cached_image: Option<Box<dyn ratatui_image::protocol::StatefulProtocol>>,
    cached_image_url: Option<String>,
}

impl App {
    fn new() -> Self {
        let path = clippa_rs::get_archive_dir();
        let files: Vec<PathBuf> = fs::read_dir(path)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|entry| entry.path()))
                    .collect()
            })
            .unwrap_or_default();

        let file_link_counts = files.iter().map(|p| {
            fs::read_to_string(p)
                .map(|c| c.lines().filter(|l| !l.is_empty()).count())
                .unwrap_or(0)
        }).collect();

        let (tx, rx) = mpsc::channel(100);

        let mut app = Self {
            files,
            file_state: ListState::default(),
            selected_files: HashSet::new(),
            file_link_counts,
            links: Vec::new(),
            link_state: ListState::default(),
            selected_links: HashSet::new(),
            mode: Mode::BrowsingFiles,
            prev_mode: Mode::BrowsingFiles,
            should_quit: false,
            last_modified: None,
            link_titles: HashMap::new(),
            title_tx: tx,
            title_rx: rx,
            search_query: String::new(),
            search_results: Vec::new(),
            search_state: ListState::default(),
            cached_image: None,
            cached_image_url: None,
        };

        if !app.files.is_empty() {
            app.file_state.select(Some(0));
            app.load_links();
        }
        app
    }

    fn check_for_title_updates(&mut self) {
        while let Ok((url, title)) = self.title_rx.try_recv() {
            self.link_titles.insert(url, title);
        }
    }

    fn check_for_updates(&mut self) {
        if let Some(i) = self.file_state.selected() {
            let path = &self.files[i];
            if let Ok(metadata) = fs::metadata(path) {
                let modified = metadata.modified().ok();
                if modified != self.last_modified {
                    self.last_modified = modified;
                    self.load_links();
                }
            }
        }
    }

    fn load_links(&mut self) {
        if let Some(i) = self.file_state.selected() {
            if let Ok(content) = fs::read_to_string(&self.files[i]) {
                let current_sel = self.link_state.selected();
                self.links = content
                    .lines()
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .collect();

                self.file_link_counts[i] = self.links.len();
                self.link_state.select(Some(
                    current_sel
                        .filter(|&s| s < self.links.len())
                        .unwrap_or(0)
                        .min(self.links.len().saturating_sub(1)),
                ));

                if self.mode == Mode::BrowsingLinks {
                    self.fetch_preview_title();
                }
            }
        }
    }

    fn fetch_preview_title(&self) {
        if self.mode == Mode::BrowsingLinks {
            if let Some(i) = self.link_state.selected() {
                if i < self.links.len() {
                    let url = self.links[i].clone();
                    if !self.link_titles.contains_key(&url) {
                        let tx = self.title_tx.clone();
                        tokio::spawn(async move {
                            let client = reqwest::Client::builder()
                                .timeout(std::time::Duration::from_secs(5))
                                .build()
                                .unwrap_or_default();

                            let Ok(mut resp) = client.get(&url).send().await else { return };
                            let mut buf = String::new();

                            while let Ok(Some(chunk)) = resp.chunk().await {
                                buf.push_str(&String::from_utf8_lossy(&chunk));
                                let lower = buf.to_ascii_lowercase();
                                if let Some(end) = lower.find("</title>") {
                                    if let Some(start) = lower[..end].rfind("<title>") {
                                        let title = buf[start + 7..end].trim().to_string();
                                        let decoded_title =
                                            html_escape::decode_html_entities(&title).to_string();
                                        if !decoded_title.is_empty() {
                                            let _ = tx.send((url, decoded_title)).await;
                                        }
                                    }
                                    break;
                                }
                                if buf.len() > 32 * 1024 {
                                    break;
                                }
                            }
                        });
                    }
                }
            }
        }
    }

    fn save_links(&self) {
        if let Some(i) = self.file_state.selected() {
            let content = self.links.join("\n") + if self.links.is_empty() { "" } else { "\n" };
            let _ = fs::write(&self.files[i], content);
        }
    }

    fn domain_name(&self, idx: usize) -> String {
        self.files[idx]
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| {
                let s = s.strip_prefix("archive_").unwrap_or(s);
                let s = s.replace('_', ".");
                s.strip_prefix("www.").unwrap_or(&s).to_string()
            })
            .unwrap_or_default()
    }

    fn open_url(&self) {
        if let Some(i) = self.link_state.selected() {
            if i < self.links.len() {
                let url = &self.links[i];
                let browser = std::env::var("BROWSER").unwrap_or_else(|_| "xdg-open".to_string());
                let _ = Command::new(browser)
                    .arg(url)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
        }
    }

    fn copy_url(&self) {
        if let Some(i) = self.link_state.selected() {
            if i < self.links.len() {
                let url = &self.links[i];
                let opts = Options::new();
                let _ = opts.copy(Source::Bytes(url.as_bytes().into()), MimeType::Text);
            }
        }
    }

    fn open_search_url(&self) {
        if let Some(i) = self.search_state.selected() {
            if let Some((_, url)) = self.search_results.get(i) {
                let browser = std::env::var("BROWSER").unwrap_or_else(|_| "xdg-open".to_string());
                let _ = Command::new(browser)
                    .arg(url)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
        }
    }

    fn copy_search_url(&self) {
        if let Some(i) = self.search_state.selected() {
            if let Some((_, url)) = self.search_results.get(i) {
                let opts = Options::new();
                let _ = opts.copy(Source::Bytes(url.as_bytes().into()), MimeType::Text);
            }
        }
    }

    fn search_global(&mut self, query: &str) {
        self.search_results.clear();

        if query.is_empty() {
            self.search_state.select(None);
            return;
        }

        let matcher = SkimMatcherV2::default();
        let mut scored: Vec<(i64, String, String)> = Vec::new();

        for (fi, file_path) in self.files.iter().enumerate() {
            let domain = self.domain_name(fi);
            if let Ok(content) = fs::read_to_string(file_path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if let Some(score) = matcher.fuzzy_match(line, query) {
                        scored.push((score, domain.clone(), line.to_string()));
                    }
                }
            }
        }

        scored.sort_unstable_by(|a, b| b.0.cmp(&a.0));

        self.search_results = scored.into_iter().map(|(_, d, u)| (d, u)).collect();

        if !self.search_results.is_empty() {
            self.search_state.select(Some(0));
        }
    }

    fn cache_screenshot(&mut self) {
        let url = match self.mode {
            Mode::BrowsingLinks => self.link_state.selected().and_then(|i| self.links.get(i).cloned()),
            Mode::Searching => self.search_state.selected().and_then(|i| {
                self.search_results.get(i).map(|(_, url)| url.clone())
            }),
            _ => None,
        };
        self.load_screenshot(url);
    }

    fn load_screenshot(&mut self, url: Option<String>) {
        if url.as_ref() == self.cached_image_url.as_ref() {
            return;
        }
        self.cached_image = None;
        self.cached_image_url = None;
        if let Some(ref u) = url {
            let path = clippa_rs::screenshot_path(u);
            if path.exists() {
                if let Ok(img) = image::open(&path) {
                    let mut picker = Picker::new((8, 18));
                    picker.guess_protocol();
                    self.cached_image = Some(picker.new_resize_protocol(img));
                    self.cached_image_url = Some(u.clone());
                }
            }
        }
    }

    fn next(&mut self) {
        match self.mode {
            Mode::BrowsingFiles => {
                let i = match self.file_state.selected() {
                    Some(i) => {
                        if i >= self.files.len() - 1 {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.file_state.select(Some(i));
                self.selected_links.clear();
                self.load_links();
            }
            Mode::BrowsingLinks => {
                if !self.links.is_empty() {
                    let i = match self.link_state.selected() {
                        Some(i) => {
                            if i >= self.links.len() - 1 {
                                0
                            } else {
                                i + 1
                            }
                        }
                        None => 0,
                    };
                    self.link_state.select(Some(i));
                    self.fetch_preview_title();
                }
            }
            Mode::Searching => {
                if !self.search_results.is_empty() {
                    let i = match self.search_state.selected() {
                        Some(i) => {
                            if i >= self.search_results.len() - 1 {
                                0
                            } else {
                                i + 1
                            }
                        }
                        None => 0,
                    };
                    self.search_state.select(Some(i));
                }
            }
            Mode::ViewingImage => {}
        }
    }

    fn previous(&mut self) {
        match self.mode {
            Mode::BrowsingFiles => {
                let i = match self.file_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            self.files.len() - 1
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.file_state.select(Some(i));
                self.selected_links.clear();
                self.load_links();
            }
            Mode::BrowsingLinks => {
                if !self.links.is_empty() {
                    let i = match self.link_state.selected() {
                        Some(i) => {
                            if i == 0 {
                                self.links.len() - 1
                            } else {
                                i - 1
                            }
                        }
                        None => 0,
                    };
                    self.link_state.select(Some(i));
                    self.fetch_preview_title();
                }
            }
            Mode::Searching => {
                if !self.search_results.is_empty() {
                    let i = match self.search_state.selected() {
                        Some(i) => {
                            if i == 0 {
                                self.search_results.len() - 1
                            } else {
                                i - 1
                            }
                        }
                        None => 0,
                    };
                    self.search_state.select(Some(i));
                }
            }
            Mode::ViewingImage => {}
        }
    }

    fn toggle_selection(&mut self) {
        match self.mode {
            Mode::BrowsingFiles => {
                if let Some(i) = self.file_state.selected() {
                    if !self.selected_files.remove(&i) {
                        self.selected_files.insert(i);
                    }
                }
            }
            Mode::BrowsingLinks => {
                if let Some(i) = self.link_state.selected() {
                    if !self.selected_links.remove(&i) {
                        self.selected_links.insert(i);
                    }
                }
            }
            Mode::Searching => {}
            Mode::ViewingImage => {}
        }
    }

    fn toggle_select_all(&mut self) {
        match self.mode {
            Mode::BrowsingFiles => {
                if self.selected_files.len() == self.files.len() && !self.files.is_empty() {
                    self.selected_files.clear();
                } else if !self.files.is_empty() {
                    self.selected_files = (0..self.files.len()).collect();
                }
            }
            Mode::BrowsingLinks => {
                if self.selected_links.len() == self.links.len() && !self.links.is_empty() {
                    self.selected_links.clear();
                } else if !self.links.is_empty() {
                    self.selected_links = (0..self.links.len()).collect();
                }
            }
            Mode::Searching => {}
            Mode::ViewingImage => {}
        }
    }

    fn delete_selection(&mut self) {
        match self.mode {
            Mode::BrowsingFiles => {
                let mut to_delete: Vec<usize> = self.selected_files.iter().copied().collect();
                if to_delete.is_empty() {
                    if let Some(i) = self.file_state.selected() {
                        to_delete.push(i);
                    }
                }
                if to_delete.is_empty() {
                    return;
                }
                to_delete.sort_unstable_by(|a, b| b.cmp(a));

                for &i in &to_delete {
                    if i < self.files.len() {
                        let _ = fs::remove_file(&self.files[i]);
                        self.files.remove(i);
                        self.file_link_counts.remove(i);
                    }
                }

                self.selected_files.clear();
                self.selected_links.clear();

                if self.files.is_empty() {
                    self.file_state.select(None);
                    self.links.clear();
                    self.link_state.select(None);
                } else {
                    let current = self.file_state.selected().unwrap_or(0);
                    self.file_state.select(Some(current.min(self.files.len() - 1)));
                    self.load_links();
                }
            }
            Mode::BrowsingLinks => {
                let mut to_delete: Vec<usize> = self.selected_links.iter().copied().collect();
                if to_delete.is_empty() {
                    if let Some(i) = self.link_state.selected() {
                        to_delete.push(i);
                    }
                }
                if to_delete.is_empty() {
                    return;
                }
                to_delete.sort_unstable_by(|a, b| b.cmp(a));
                to_delete.dedup();

                for &i in &to_delete {
                    if i < self.links.len() {
                        self.links.remove(i);
                    }
                }

                self.selected_links.clear();

                if self.links.is_empty() {
                    self.link_state.select(None);
                } else {
                    let current = self.link_state.selected().unwrap_or(0);
                    self.link_state.select(Some(current.min(self.links.len() - 1)));
                }

                self.save_links();
            }
            Mode::Searching => {
                let sel = self.search_state.selected();
                let to_delete = sel.and_then(|i| self.search_results.get(i).cloned());
                let Some((domain, url)) = to_delete else { return };

                let file_idx = self.files.iter().position(|p| {
                    let name = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.strip_prefix("archive_").unwrap_or(s))
                        .unwrap_or("");
                    name == domain
                });
                let Some(fi) = file_idx else { return };

                let path = &self.files[fi];
                let Ok(content) = fs::read_to_string(path) else { return };
                let lines: Vec<&str> = content.lines().collect();
                let line_idx = lines.iter().position(|l| l.trim() == url);
                let Some(li) = line_idx else { return };

                let mut keep: Vec<&str> = lines.clone();
                keep.remove(li);
                let new_content = keep.join("\n") + if keep.is_empty() { "" } else { "\n" };
                let _ = fs::write(path, new_content);

                self.file_link_counts[fi] = self.file_link_counts[fi].saturating_sub(1);

                if fi == self.file_state.selected().unwrap_or(usize::MAX) {
                    self.load_links();
                }

                self.search_results.remove(sel.unwrap());
                let len = self.search_results.len();
                if len > 0 {
                    let new_sel = sel.unwrap().min(len - 1);
                    self.search_state.select(Some(new_sel));
                } else {
                    self.search_state.select(None);
                }
            }
            Mode::ViewingImage => {}
        }
    }

    fn next_domain(&mut self) {
        if self.files.is_empty() {
            return;
        }
        let i = match self.file_state.selected() {
            Some(i) => {
                if i >= self.files.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.file_state.select(Some(i));
        self.selected_links.clear();
        self.load_links();
        if self.mode == Mode::BrowsingLinks {
            self.fetch_preview_title();
        }
    }

    fn prev_domain(&mut self) {
        if self.files.is_empty() {
            return;
        }
        let i = match self.file_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.files.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.file_state.select(Some(i));
        self.selected_links.clear();
        self.load_links();
        if self.mode == Mode::BrowsingLinks {
            self.fetch_preview_title();
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut app = App::new();

    while !app.should_quit {
        app.check_for_updates();
        app.check_for_title_updates();
        if app.mode != Mode::ViewingImage {
            app.cache_screenshot();
        }
        terminal.draw(|f| ui(f, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if app.mode == Mode::ViewingImage {
                        app.mode = app.prev_mode.clone();
                    } else if app.mode == Mode::Searching {
                        match key.code {
                            KeyCode::Esc => {
                                app.search_query.clear();
                                app.search_results.clear();
                                app.mode = app.prev_mode.clone();
                            }
                            KeyCode::Char('j') | KeyCode::Down => app.next(),
                            KeyCode::Char('k') | KeyCode::Up => app.previous(),
                            KeyCode::Enter => app.open_search_url(),
                            KeyCode::Char('y') => app.copy_search_url(),
                            KeyCode::Char('d') | KeyCode::Delete => {
                                app.delete_selection();
                                let q = app.search_query.clone();
                                app.search_global(&q);
                            }
                            KeyCode::Char('v') => {
                                if let Some(i) = app.search_state.selected() {
                                    if i < app.search_results.len() {
                                        let (_, url) = &app.search_results[i];
                                        let path = clippa_rs::screenshot_path(url);
                                        if path.exists() {
                                            app.load_screenshot(Some(url.clone()));
                                            app.prev_mode = app.mode.clone();
                                            app.mode = Mode::ViewingImage;
                                        }
                                    }
                                }
                            }
                            KeyCode::Backspace => {
                                app.search_query.pop();
                                let q = app.search_query.clone();
                                app.search_global(&q);
                            }
                            KeyCode::Char(c) if !c.is_control() => {
                                app.search_query.push(c);
                                let q = app.search_query.clone();
                                app.search_global(&q);
                            }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') => app.should_quit = true,
                            KeyCode::Char('j') | KeyCode::Down => app.next(),
                            KeyCode::Char('k') | KeyCode::Up => app.previous(),
                            KeyCode::Char('a')
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                app.toggle_select_all();
                            }
                            KeyCode::Char(' ') => app.toggle_selection(),
                            KeyCode::Esc => {
                                app.selected_files.clear();
                                app.selected_links.clear();
                            }
                            KeyCode::Enter if app.mode == Mode::BrowsingFiles => {
                                if !app.links.is_empty() {
                                    app.mode = Mode::BrowsingLinks;
                                    app.fetch_preview_title();
                                }
                            }
                            KeyCode::Enter if app.mode == Mode::BrowsingLinks => {
                                app.open_url();
                            }
                            KeyCode::Char('y') if app.mode == Mode::BrowsingLinks => {
                                app.copy_url();
                            }
                            KeyCode::Char('z') if app.mode == Mode::BrowsingLinks => {
                                app.mode = Mode::BrowsingFiles;
                            }
                            KeyCode::Char('v') if app.mode == Mode::BrowsingLinks && app.cached_image.is_some() => {
                                app.prev_mode = app.mode.clone();
                                app.mode = Mode::ViewingImage;
                            }
                            KeyCode::Tab => app.next_domain(),
                            KeyCode::BackTab => app.prev_domain(),
                            KeyCode::Char('d') | KeyCode::Delete => app.delete_selection(),
                            KeyCode::Char('/') => {
                                app.prev_mode = app.mode.clone();
                                app.mode = Mode::Searching;
                                app.search_query.clear();
                                app.search_results.clear();
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

fn render_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .title(" Preview ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    if let Some(ref mut state) = app.cached_image {
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_stateful_widget(StatefulImage::new(None), inner, state);
        return;
    }

    let preview_text = match app.mode {
        Mode::BrowsingFiles => {
            if let Some(i) = app.file_state.selected() {
                if i < app.files.len() {
                    let count = app.file_link_counts[i];
                    format!("{}  |  {} links", app.domain_name(i), count)
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        }
        Mode::BrowsingLinks => {
            if let Some(i) = app.link_state.selected() {
                if i < app.links.len() {
                    let url = &app.links[i];
                    if let Some(title) = app.link_titles.get(url) {
                        format!("{} - {}", title, url)
                    } else {
                        format!("Fetching... {}", url)
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        }
        Mode::Searching => String::new(),
        Mode::ViewingImage => String::new(),
    };

    f.render_widget(
        Paragraph::new(preview_text)
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_image_fullscreen(f: &mut Frame, app: &mut App) {
    let area = f.size();
    let block = Block::default()
        .title(" Screenshot — press any key to exit ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    if let Some(ref mut state) = app.cached_image {
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_stateful_widget(StatefulImage::new(None), inner, state);
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    if app.mode == Mode::ViewingImage {
        return render_image_fullscreen(f, app);
    }
    if app.mode == Mode::Searching {
        return ui_search(f, app);
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(6),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(f.size());

    f.render_widget(
        Paragraph::new("Manage Your Links")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        chunks[0],
    );

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(chunks[1]);

    let file_items: Vec<ListItem> = app
        .files
        .iter()
        .enumerate()
        .map(|(i, _p)| {
            let count = app.file_link_counts[i];
            let display = format!("{} ({})", app.domain_name(i), count);
            let text = if app.selected_files.contains(&i) {
                Line::from(vec![
                    Span::styled(" * ", Style::default().fg(Color::Yellow)),
                    Span::raw(display),
                ])
            } else {
                Line::from(vec![Span::raw("   "), Span::raw(display)])
            };
            ListItem::new(text)
        })
        .collect();

    let file_block = Block::default().title(" Domains ").borders(Borders::ALL);
    let file_list = List::new(file_items)
        .block(if app.mode == Mode::BrowsingFiles {
            file_block.border_style(Style::default().fg(Color::Yellow))
        } else {
            file_block
        })
        .highlight_style(
            Style::default()
                .bg(Color::Indexed(237))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    f.render_stateful_widget(file_list, main_chunks[0], &mut app.file_state);

    let link_items: Vec<ListItem> = app
        .links
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let text = if app.selected_links.contains(&i) {
                Line::from(vec![
                    Span::styled(" * ", Style::default().fg(Color::Yellow)),
                    Span::raw(s.as_str()),
                ])
            } else {
                Line::from(vec![Span::raw("   "), Span::raw(s.as_str())])
            };
            ListItem::new(text)
        })
        .collect();

    let link_block = Block::default()
        .title(" Archived Links ")
        .borders(Borders::ALL);
    let link_list = List::new(link_items)
        .block(if app.mode == Mode::BrowsingLinks {
            link_block.border_style(Style::default().fg(Color::Yellow))
        } else {
            link_block
        })
        .highlight_style(Style::default().bg(Color::Indexed(237)).fg(Color::Red))
        .highlight_symbol("[-] ");
    f.render_stateful_widget(link_list, main_chunks[1], &mut app.link_state);

    render_preview(f, app, chunks[2]);

    let status_text = match app.mode {
        Mode::BrowsingFiles => {
            let total = app.files.len();
            let current = app.file_state.selected().map(|i| i + 1).unwrap_or(0);
            let sel = app.selected_files.len();
            if sel > 0 {
                format!(" [F] {}/{}  |  Sel: {}", current, total, sel)
            } else {
                format!(" [F] {}/{}", current, total)
            }
        }
        Mode::BrowsingLinks => {
            let total = app.links.len();
            let current = app.link_state.selected().map(|i| i + 1).unwrap_or(0);
            let sel = app.selected_links.len();
            if sel > 0 {
                format!(" [L] {}/{}  |  Sel: {}", current, total, sel)
            } else {
                format!(" [L] {}/{}", current, total)
            }
        }
        Mode::Searching => String::new(),
        Mode::ViewingImage => String::new(),
    };

    f.render_widget(
        Paragraph::new(status_text)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::TOP)),
        chunks[3],
    );

    let help = match app.mode {
        Mode::BrowsingFiles => {
            " [j/k] Move | [Tab] Next Domain | [Space] Select | [^A] All | [d] Delete | [Enter] Open | [/] Search | [q] Quit "
        }
        Mode::BrowsingLinks => {
            " [j/k] Move | [Tab] Next Domain | [Space] Select | [^A] All | [d] Delete | [Enter] Open URL | [y] Copy URL | [/] Search | [z] Back | [q] Quit "
        }
        Mode::Searching => "",
        Mode::ViewingImage => "",
    };
    f.render_widget(
        Paragraph::new(help)
            .block(Block::default().borders(Borders::ALL))
            .alignment(Alignment::Center),
        chunks[4],
    );
}

fn ui_search(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(6),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(f.size());

    f.render_widget(
        Paragraph::new("Search All Archives")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        chunks[0],
    );

    let search_display = if app.search_query.is_empty() {
        " Type to fuzzy-search across all archives...".to_string()
    } else {
        format!(" / {}", app.search_query)
    };
    f.render_widget(
        Paragraph::new(search_display)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Green)),
            )
            .style(Style::default().fg(Color::Green)),
        chunks[1],
    );

    let result_items: Vec<ListItem> = app
        .search_results
        .iter()
        .map(|(domain, url)| {
            let display = format!(" {}  {}", domain, url);
            ListItem::new(display)
        })
        .collect();

    let results_list = List::new(result_items)
        .block(
            Block::default()
                .title(format!(" Results ({})", app.search_results.len()))
                .borders(Borders::ALL),
        )
        .highlight_style(Style::default().bg(Color::Indexed(237)).fg(Color::Red))
        .highlight_symbol("[-] ");
    f.render_stateful_widget(results_list, chunks[2], &mut app.search_state);

    let block = Block::default()
        .title(" Preview ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let ss_path = app.search_state.selected().and_then(|i| {
        app.search_results.get(i).map(|(_, url)| clippa_rs::screenshot_path(url))
    });

    if let Some(ref path) = ss_path {
        if path.exists() {
            if let Ok(img) = image::open(path) {
                let mut picker = Picker::new((8, 18));
                picker.guess_protocol();
                let inner = block.inner(chunks[3]);
                f.render_widget(block, chunks[3]);
                let mut state = picker.new_resize_protocol(img);
                f.render_stateful_widget(StatefulImage::new(None), inner, &mut state);
                return;
            }
        }
    }

    let preview_text = if let Some(i) = app.search_state.selected() {
        if let Some((domain, url)) = app.search_results.get(i) {
            if let Some(title) = app.link_titles.get(url) {
                format!("{} | {} - {}", domain, title, url)
            } else {
                format!("{} - {}", domain, url)
            }
        } else {
            String::new()
        }
    } else if app.search_query.is_empty() {
        " Type above to search across all archives".to_string()
    } else {
        " No matches found".to_string()
    };

    f.render_widget(
        Paragraph::new(preview_text)
            .block(block)
            .wrap(Wrap { trim: true }),
        chunks[3],
    );

    let status_text = if app.search_query.is_empty() {
        " [S] Type to search".to_string()
    } else {
        format!(
            " [S] {} result{} | \"{}\"",
            app.search_results.len(),
            if app.search_results.len() == 1 { "" } else { "s" },
            app.search_query
        )
    };
    f.render_widget(
        Paragraph::new(status_text).style(Style::default().fg(Color::DarkGray)),
        chunks[4],
    );

    f.render_widget(
        Paragraph::new(" [j/k] Move | [Enter] Open URL | [y] Copy URL | [d] Delete | [Esc] Back ")
            .block(Block::default().borders(Borders::ALL))
            .alignment(Alignment::Center),
        chunks[5],
    );
}
