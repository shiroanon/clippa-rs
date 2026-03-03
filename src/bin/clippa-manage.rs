use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{prelude::*, widgets::*};
use std::{
    fs,
    io::{Result, stdout},
    path::PathBuf,
    time::{Duration, SystemTime},
};

#[derive(PartialEq)]
enum Mode {
    BrowsingFiles,
    BrowsingLinks,
}

struct App {
    files: Vec<PathBuf>,
    file_state: ListState,
    links: Vec<String>,
    link_state: ListState,
    mode: Mode,
    should_quit: bool,
    last_modified: Option<SystemTime>,
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

        let mut app = Self {
            files,
            file_state: ListState::default(),
            links: Vec::new(),
            link_state: ListState::default(),
            mode: Mode::BrowsingFiles,
            should_quit: false,
            last_modified: None,
        };

        if !app.files.is_empty() {
            app.file_state.select(Some(0));
            app.load_links();
        }
        app
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
                } else {
                    self.link_state.select(None);
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
                }
            }
        }
    }

    fn delete_link(&mut self) {
        if self.mode == Mode::BrowsingLinks {
            if let Some(i) = self.link_state.selected() {
                self.links.remove(i);
                self.save_links();
                self.load_links(); // Refresh timestamp to prevent double-load
            }
        }
    }
}

fn main() -> Result<()> {
    stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut app = App::new();

    while !app.should_quit {
        app.check_for_updates();
        terminal.draw(|f| ui(f, &mut app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => app.should_quit = true,
                        KeyCode::Char('j') | KeyCode::Down => app.next(),
                        KeyCode::Char('k') | KeyCode::Up => app.previous(),
                        KeyCode::Enter | KeyCode::Char('l') if app.mode == Mode::BrowsingFiles => {
                            if !app.links.is_empty() {
                                app.mode = Mode::BrowsingLinks;
                            }
                        }
                        KeyCode::Esc | KeyCode::Char('h') if app.mode == Mode::BrowsingLinks => {
                            app.mode = Mode::BrowsingFiles;
                        }
                        KeyCode::Char('x') | KeyCode::Delete => app.delete_link(),
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
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.size());

    // Header
    f.render_widget(
        Paragraph::new(" CLIPPA-RS: REALTIME MANAGER ")
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
        .map(|p| ListItem::new(p.file_name().unwrap().to_string_lossy().into_owned()))
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
        .map(|s| ListItem::new(s.as_str()))
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

    // Footer
    let help = match app.mode {
        Mode::BrowsingFiles => " [j/k] Move | [Enter] Open Domain | [q] Quit ",
        Mode::BrowsingLinks => " [j/k] Move | [x] Delete Link | [Esc] Back ",
    };
    f.render_widget(
        Paragraph::new(help)
            .block(Block::default().borders(Borders::ALL))
            .alignment(Alignment::Center),
        chunks[2],
    );
}
