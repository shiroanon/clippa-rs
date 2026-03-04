use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{prelude::*, widgets::*};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Result, stdout},
    path::PathBuf,
    time::{Duration, SystemTime},
};
use tokio::sync::mpsc;

#[derive(PartialEq)]
enum Mode {
    BrowsingFiles,
    BrowsingLinks,
}

struct App {
    files: Vec<PathBuf>,
    file_state: ListState,
    selected_files: HashSet<usize>,
    links: Vec<String>,
    link_state: ListState,
    selected_links: HashSet<usize>,
    mode: Mode,
    should_quit: bool,
    last_modified: Option<SystemTime>,
    link_titles: HashMap<String, String>,
    title_tx: mpsc::Sender<(String, String)>,
    title_rx: mpsc::Receiver<(String, String)>,
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

        let (tx, rx) = mpsc::channel(100);

        let mut app = Self {
            files,
            file_state: ListState::default(),
            selected_files: HashSet::new(),
            links: Vec::new(),
            link_state: ListState::default(),
            selected_links: HashSet::new(),
            mode: Mode::BrowsingFiles,
            should_quit: false,
            last_modified: None,
            link_titles: HashMap::new(),
            title_tx: tx,
            title_rx: rx,
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

                if !self.links.is_empty() {
                    self.link_state
                        .select(Some(current_sel.unwrap_or(0).min(self.links.len() - 1)));
                    self.fetch_preview_title();
                } else {
                    self.link_state.select(None);
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
                            // Stream chunks and stop as soon as we see </title>.
                            // This avoids downloading the full page body.
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
                                        if !title.is_empty() {
                                            let _ = tx.send((url, title)).await;
                                        }
                                    }
                                    break;
                                }
                                // Safety cap: stop after 32KB to avoid hanging on weird pages
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
                self.load_links();
            }
        }
    }

    fn next_domain(&mut self) {
        if self.files.is_empty() { return; }
        let i = match self.file_state.selected() {
            Some(i) => if i >= self.files.len() - 1 { 0 } else { i + 1 },
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
        if self.files.is_empty() { return; }
        let i = match self.file_state.selected() {
            Some(i) => if i == 0 { self.files.len() - 1 } else { i - 1 },
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
        terminal.draw(|f| ui(f, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => app.should_quit = true,
                        KeyCode::Char('j') | KeyCode::Down => app.next(),
                        KeyCode::Char('k') | KeyCode::Up => app.previous(),
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
                        KeyCode::Char('z') if app.mode == Mode::BrowsingLinks => {
                            app.mode = Mode::BrowsingFiles;
                        }
                        KeyCode::Tab => app.next_domain(),
                        KeyCode::BackTab => app.prev_domain(),
                        KeyCode::Char('d') | KeyCode::Delete => app.delete_selection(),
                        _ => {}
                    }
                }
            }
        }
    }

    stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(0),    // Main list
            Constraint::Length(4), // Preview pane
            Constraint::Length(3), // Footer
        ])
        .split(f.size());

    // Header
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

    // File List
    let file_items: Vec<ListItem> = app
        .files
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let content = p.file_name().unwrap().to_string_lossy().into_owned();
            let text = if app.selected_files.contains(&i) {
                Line::from(vec![Span::styled(" * ", Style::default().fg(Color::Yellow)), Span::raw(content)])
            } else {
                Line::from(vec![Span::raw("   "), Span::raw(content)])
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

    // Link List
    let link_items: Vec<ListItem> = app
        .links
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let text = if app.selected_links.contains(&i) {
                Line::from(vec![Span::styled(" * ", Style::default().fg(Color::Yellow)), Span::raw(s.as_str())])
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

    // Preview Pane
    let preview_text = match app.mode {
        Mode::BrowsingFiles => {
            if let Some(i) = app.file_state.selected() {
                if i < app.files.len() {
                    app.files[i].file_name().unwrap_or_default().to_string_lossy().into_owned()
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
    };

    f.render_widget(
        Paragraph::new(preview_text)
            .block(
                Block::default()
                    .title(" Preview ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: true }),
        chunks[2],
    );

    // Footer
    let help = match app.mode {
        Mode::BrowsingFiles => " [j/k] Move | [Tab] Next Domain | [Space] Select | [^A] All | [d] Delete | [Enter] Open | [q] Quit ",
        Mode::BrowsingLinks => " [j/k] Move | [Tab] Next Domain | [Space] Select | [^A] All | [d] Delete | [z] Back ",
    };
    f.render_widget(
        Paragraph::new(help)
            .block(Block::default().borders(Borders::ALL))
            .alignment(Alignment::Center),
        chunks[3],
    );
}
