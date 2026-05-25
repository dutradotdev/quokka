use std::io::{IsTerminal, Write};

use anyhow::{bail, Result};
use dialoguer::{theme::ColorfulTheme, Confirm};
use owo_colors::OwoColorize;

use crate::device::{Device, MediaFile, WalkCallback, WalkProgress};
use crate::ui::{format_bytes, spinner};

/// Anything outside these roots is invisible to `analyze` — the safety
/// guardrail that prevents a "select all + delete" from breaking Photos.app
/// or sync state. See spec for the rationale before widening this list.
const ROOTS: &[&str] = &["/DCIM", "/Downloads", "/Recordings", "/Books"];

pub async fn run(device: &dyn Device, top: usize, delete: bool) -> Result<()> {
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if delete && !interactive {
        bail!("`--delete` is destructive and needs an interactive terminal. Re-run from a TTY.");
    }

    let all = walk(device).await?;
    let total_files = all.len();
    let total_bytes: u64 = all.iter().map(|f| f.size_bytes).sum();

    if all.is_empty() {
        let mut out = anstream::stdout();
        writeln!(out, "No files in DCIM, Downloads, Recordings, Books.")?;
        return Ok(());
    }

    let sorted = sort_by_size(all);

    if !delete {
        let top_n: Vec<MediaFile> = sorted.iter().take(top).cloned().collect();
        let mut out = anstream::stdout();
        writeln!(
            out,
            "{}",
            render_file_list(&top_n, total_files, total_bytes)
        )?;
        return Ok(());
    }

    let outcome = tui::run(sorted).await?;
    match outcome {
        tui::Outcome::Quit => Ok(()),
        tui::Outcome::Picked(picked) => {
            if picked.is_empty() {
                return Ok(());
            }
            confirm_and_delete(device, &picked).await
        }
    }
}

async fn walk(device: &dyn Device) -> Result<Vec<MediaFile>> {
    let bar = spinner("Scanning...");
    let bar_for_cb = bar.clone();
    let on_progress: WalkCallback = Box::new(move |p: WalkProgress| {
        bar_for_cb.set_message(format!(
            "Scanning... {} files, {}",
            p.files_seen,
            format_bytes(p.bytes_seen)
        ));
    });
    let result = device.afc_walk(ROOTS, on_progress).await;
    bar.finish_and_clear();
    result
}

pub fn sort_by_size(mut files: Vec<MediaFile>) -> Vec<MediaFile> {
    files.sort_by_key(|f| std::cmp::Reverse(f.size_bytes));
    files
}

pub fn top_n_by_size(files: Vec<MediaFile>, top: usize) -> Vec<MediaFile> {
    let mut sorted = sort_by_size(files);
    sorted.truncate(top);
    sorted
}

pub(crate) fn ext_lower(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

pub fn kind_from_ext(path: &str) -> &'static str {
    match ext_lower(path).as_str() {
        "mov" | "mp4" | "m4v" | "hevc" => "Video",
        "jpg" | "jpeg" | "heic" | "png" | "gif" => "Photo",
        "m4a" | "mp3" | "aac" | "wav" => "Audio",
        "pdf" | "epub" => "Doc",
        _ => "Other",
    }
}

pub fn render_file_list(files: &[MediaFile], total_files: usize, total_bytes: u64) -> String {
    if files.is_empty() {
        return "No files in DCIM, Downloads, Recordings, Books.\n".to_string();
    }
    let size_w = files
        .iter()
        .map(|f| format_bytes(f.size_bytes).len())
        .max()
        .unwrap_or(0)
        .max("size".len());
    let kind_w = files
        .iter()
        .map(|f| kind_from_ext(&f.path).len())
        .max()
        .unwrap_or(0)
        .max("kind".len());

    let mut out = String::new();
    out.push_str(&format!("{:>size_w$}  {:<kind_w$}  path\n", "size", "kind"));
    for f in files {
        out.push_str(&format!(
            "{size:>size_w$}  {kind:<kind_w$}  {path}\n",
            size = format_bytes(f.size_bytes),
            kind = kind_from_ext(&f.path),
            path = f.path,
        ));
    }
    let shown_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
    out.push_str(&format!(
        "{} files shown · {} · {} total files scanned · {}\n",
        files.len(),
        format_bytes(shown_bytes).bold(),
        total_files,
        format_bytes(total_bytes).bold(),
    ));
    out
}

pub fn build_confirm_prompt(picked: &[MediaFile]) -> String {
    let total: u64 = picked.iter().map(|f| f.size_bytes).sum();
    let mut s = format!(
        "Delete {} file(s) ({})? They will be removed from the device permanently.",
        picked.len(),
        format_bytes(total),
    );
    if picked.iter().any(|f| f.path.starts_with("/DCIM/")) {
        s.push_str("\nNote: Photos.app may still show thumbnails until the next library refresh.");
    }
    s
}

async fn confirm_and_delete(device: &dyn Device, picked: &[MediaFile]) -> Result<()> {
    let mut out = anstream::stdout();
    let confirmed = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(build_confirm_prompt(picked))
        .default(true)
        .interact()?;
    if !confirmed {
        writeln!(out, "Aborted.")?;
        return Ok(());
    }
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut bytes_freed: u64 = 0;
    for file in picked {
        match device.afc_delete(&file.path).await {
            Ok(()) => {
                bytes_freed = bytes_freed.saturating_add(file.size_bytes);
                ok += 1;
                writeln!(out, "{} {}", "✓".green(), file.path)?;
            }
            Err(e) => {
                failed += 1;
                writeln!(out, "{} {}: {e}", "✗".red(), file.path)?;
            }
        }
    }
    if failed == 0 {
        writeln!(
            out,
            "Deleted {ok} files ({}).",
            format_bytes(bytes_freed).bold()
        )?;
    } else {
        writeln!(
            out,
            "Deleted {ok} files ({}); {failed} failed.",
            format_bytes(bytes_freed).bold()
        )?;
    }
    Ok(())
}

pub mod heuristics {
    use std::collections::{HashMap, HashSet};
    use std::path::Path;

    use super::{ext_lower, MediaFile};

    /// Edited variants on iPhone are stored as `IMG_E<digits>` next to the
    /// original `IMG_<digits>`. Used to detect the original/edited pair.
    const EDITED_PREFIX: &str = "IMG_E";
    const ORIGINAL_PREFIX: &str = "IMG_";
    const ONE_YEAR_SECS: i64 = 365 * 24 * 60 * 60;

    pub struct Match {
        pub label: &'static str,
        pub description: &'static str,
        pub enabled: bool,
        indices: Vec<usize>,
    }

    impl Match {
        pub fn count(&self) -> usize {
            self.indices.len()
        }

        pub fn indices(&self) -> &[usize] {
            &self.indices
        }

        pub fn bytes(&self, files: &[MediaFile]) -> u64 {
            self.indices.iter().map(|&i| files[i].size_bytes).sum()
        }
    }

    fn build(label: &'static str, description: &'static str, indices: Vec<usize>) -> Match {
        let enabled = !indices.is_empty();
        Match {
            label,
            description,
            enabled,
            indices,
        }
    }

    pub fn detect_all(files: &[MediaFile], now_unix: i64) -> Vec<Match> {
        vec![
            build(
                "Live Photo videos with photo sibling",
                ".MOV with matching .HEIC/.JPG — rarely watched",
                live_photo_motion(files),
            ),
            build(
                "Originals when edited version exists",
                "IMG_X kept when IMG_EX is in the same folder",
                originals_with_edited(files),
            ),
            build(
                "Old screenshots (> 1 year)",
                ".PNG in DCIM modified more than a year ago",
                old_screenshots(files, now_unix - ONE_YEAR_SECS),
            ),
            build(
                "Exact duplicates (name + size)",
                "Same filename and size in different folders",
                exact_duplicates(files),
            ),
        ]
    }

    fn parent_dir(path: &str) -> &str {
        Path::new(path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("")
    }

    fn stem(path: &str) -> &str {
        Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
    }

    fn basename(path: &str) -> &str {
        Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
    }

    pub fn live_photo_motion(files: &[MediaFile]) -> Vec<usize> {
        let photo_keys: HashSet<(String, String)> = files
            .iter()
            .filter(|f| matches!(ext_lower(&f.path).as_str(), "heic" | "jpg" | "jpeg"))
            .map(|f| (parent_dir(&f.path).to_string(), stem(&f.path).to_string()))
            .collect();
        files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                ext_lower(&f.path) == "mov"
                    && photo_keys
                        .contains(&(parent_dir(&f.path).to_string(), stem(&f.path).to_string()))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn originals_with_edited(files: &[MediaFile]) -> Vec<usize> {
        let unedited_form: HashSet<(String, String)> = files
            .iter()
            .filter_map(|f| {
                stem(&f.path).strip_prefix(EDITED_PREFIX).map(|rest| {
                    (
                        parent_dir(&f.path).to_string(),
                        format!("{ORIGINAL_PREFIX}{rest}"),
                    )
                })
            })
            .collect();
        files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                unedited_form
                    .contains(&(parent_dir(&f.path).to_string(), stem(&f.path).to_string()))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn old_screenshots(files: &[MediaFile], cutoff_unix: i64) -> Vec<usize> {
        files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                ext_lower(&f.path) == "png"
                    && f.path.starts_with("/DCIM/")
                    && f.modified_unix < cutoff_unix
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn exact_duplicates(files: &[MediaFile]) -> Vec<usize> {
        let mut groups: HashMap<(String, u64), Vec<usize>> = HashMap::new();
        for (i, f) in files.iter().enumerate() {
            groups
                .entry((basename(&f.path).to_string(), f.size_bytes))
                .or_default()
                .push(i);
        }
        let mut out: Vec<usize> = groups
            .into_values()
            .filter(|idxs| idxs.len() > 1)
            .flat_map(|idxs| idxs.into_iter().skip(1))
            .collect();
        out.sort_unstable();
        out
    }
}

mod tui {
    use std::io;

    use anyhow::Result;
    use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::{execute, terminal};
    use futures::StreamExt;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{List, ListItem, ListState, Paragraph};
    use ratatui::{Frame, Terminal};

    use super::{kind_from_ext, MediaFile};
    use crate::ui::format_bytes;

    pub enum Outcome {
        Quit,
        Picked(Vec<MediaFile>),
    }

    pub async fn run(files: Vec<MediaFile>) -> Result<Outcome> {
        if files.is_empty() {
            return Ok(Outcome::Quit);
        }
        let mut state = State::new(files);
        let mut term = TerminalGuard::enter()?;
        let mut events = EventStream::new();

        loop {
            term.0.draw(|f| draw(f, &mut state))?;
            if let Some(Ok(event)) = events.next().await {
                if let Some(out) = handle_event(event, &mut state) {
                    return Ok(out);
                }
            }
        }
    }

    /// `None` disables the filter; values are bytes.
    pub(super) const THRESHOLDS: &[Option<u64>] = &[
        None,
        Some(1_000_000),
        Some(DEFAULT_THRESHOLD),
        Some(100_000_000),
        Some(1_000_000_000),
    ];

    const DEFAULT_THRESHOLD: u64 = 10_000_000;

    pub(super) struct Overlay {
        pub matches: Vec<super::heuristics::Match>,
        pub cursor: usize,
    }

    struct State {
        files: Vec<MediaFile>,
        selected: Vec<bool>,
        list: ListState,
        search_query: String,
        search_active: bool,
        threshold_idx: usize,
        overlay: Option<Overlay>,
    }

    impl State {
        fn new(files: Vec<MediaFile>) -> Self {
            let len = files.len();
            let mut list = ListState::default();
            list.select(Some(0));
            Self {
                files,
                selected: vec![false; len],
                list,
                search_query: String::new(),
                search_active: false,
                threshold_idx: THRESHOLDS
                    .iter()
                    .position(|t| *t == Some(DEFAULT_THRESHOLD))
                    .unwrap_or(0),
                overlay: None,
            }
        }

        fn open_overlay(&mut self) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let matches = super::heuristics::detect_all(&self.files, now);
            self.overlay = Some(Overlay { matches, cursor: 0 });
        }

        fn close_overlay(&mut self) {
            self.overlay = None;
        }

        fn overlay_apply(&mut self) {
            let Some(overlay) = self.overlay.take() else {
                return;
            };
            // Two-pass so a file covered by both an enabled and a disabled
            // heuristic ends up marked (any "on" wins). Files outside every
            // heuristic keep whatever the user toggled manually.
            let len = self.files.len();
            let mut covered = vec![false; len];
            let mut mark = vec![false; len];
            for m in &overlay.matches {
                for &i in m.indices() {
                    if i >= len {
                        continue;
                    }
                    covered[i] = true;
                    if m.enabled {
                        mark[i] = true;
                    }
                }
            }
            for i in 0..len {
                if covered[i] {
                    self.selected[i] = mark[i];
                }
            }
        }

        fn threshold(&self) -> Option<u64> {
            THRESHOLDS[self.threshold_idx]
        }

        fn cycle_threshold(&mut self) {
            self.threshold_idx = (self.threshold_idx + 1) % THRESHOLDS.len();
            self.clamp_cursor();
        }

        fn visible(&self) -> Vec<usize> {
            let needle = if self.search_query.is_empty() {
                None
            } else {
                Some(self.search_query.to_lowercase())
            };
            let min_size = self.threshold();
            self.files
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    min_size.is_none_or(|m| f.size_bytes >= m)
                        && needle
                            .as_ref()
                            .is_none_or(|n| f.path.to_lowercase().contains(n))
                })
                .map(|(i, _)| i)
                .collect()
        }

        fn cursor_index(&self) -> Option<usize> {
            let visible = self.visible();
            let cur = self.list.selected()?;
            visible.get(cur).copied()
        }

        fn clamp_cursor(&mut self) {
            let len = self.visible().len();
            if len == 0 {
                self.list.select(None);
                return;
            }
            let cur = self.list.selected().unwrap_or(0).min(len - 1);
            self.list.select(Some(cur));
        }

        fn visible_len(&self) -> usize {
            self.visible().len()
        }

        fn move_up(&mut self) {
            let cur = self.list.selected().unwrap_or(0);
            self.list.select(Some(cur.saturating_sub(1)));
        }

        fn move_down(&mut self) {
            let cur = self.list.selected().unwrap_or(0);
            let next = (cur + 1).min(self.visible_len().saturating_sub(1));
            self.list.select(Some(next));
        }

        fn page_up(&mut self, page: usize) {
            let cur = self.list.selected().unwrap_or(0);
            self.list.select(Some(cur.saturating_sub(page.max(1))));
        }

        fn page_down(&mut self, page: usize) {
            let cur = self.list.selected().unwrap_or(0);
            let next = (cur + page.max(1)).min(self.visible_len().saturating_sub(1));
            self.list.select(Some(next));
        }

        fn jump_top(&mut self) {
            if self.visible_len() == 0 {
                self.list.select(None);
            } else {
                self.list.select(Some(0));
            }
        }

        fn jump_bottom(&mut self) {
            let len = self.visible_len();
            if len == 0 {
                self.list.select(None);
            } else {
                self.list.select(Some(len - 1));
            }
        }

        fn toggle(&mut self) {
            if let Some(i) = self.cursor_index() {
                if let Some(slot) = self.selected.get_mut(i) {
                    *slot = !*slot;
                }
            }
        }

        fn picked(&self) -> Vec<MediaFile> {
            self.selected
                .iter()
                .enumerate()
                .filter(|&(_, &s)| s)
                .map(|(i, _)| self.files[i].clone())
                .collect()
        }
    }

    fn handle_event(event: Event, state: &mut State) -> Option<Outcome> {
        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event
        else {
            return None;
        };

        if state.overlay.is_some() {
            return handle_overlay_event(code, modifiers, state);
        }

        if state.search_active {
            match code {
                KeyCode::Esc => {
                    state.search_active = false;
                    state.search_query.clear();
                    state.clamp_cursor();
                }
                KeyCode::Enter => state.search_active = false,
                KeyCode::Backspace => {
                    state.search_query.pop();
                    state.clamp_cursor();
                }
                KeyCode::Char('c') if modifiers == KeyModifiers::CONTROL => {
                    return Some(Outcome::Quit);
                }
                KeyCode::Char(c) => {
                    state.search_query.push(c);
                    state.clamp_cursor();
                }
                _ => {}
            }
            return None;
        }

        match (code, modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('q'), _) => Some(Outcome::Quit),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(Outcome::Quit),
            (KeyCode::Enter, _) => Some(Outcome::Picked(state.picked())),
            (KeyCode::Char('/'), _) => {
                state.search_active = true;
                None
            }
            (KeyCode::Char(' '), _) => {
                state.toggle();
                None
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                state.move_up();
                None
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                state.move_down();
                None
            }
            (KeyCode::PageUp, _) => {
                state.page_up(10);
                None
            }
            (KeyCode::PageDown, _) => {
                state.page_down(10);
                None
            }
            (KeyCode::Home, _) | (KeyCode::Char('g'), _) => {
                state.jump_top();
                None
            }
            (KeyCode::End, _) | (KeyCode::Char('G'), _) => {
                state.jump_bottom();
                None
            }
            (KeyCode::Char('m'), _) => {
                state.cycle_threshold();
                None
            }
            (KeyCode::Char('a'), _) => {
                state.open_overlay();
                None
            }
            _ => None,
        }
    }

    fn handle_overlay_event(
        code: KeyCode,
        modifiers: KeyModifiers,
        state: &mut State,
    ) -> Option<Outcome> {
        match (code, modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(Outcome::Quit),
            (KeyCode::Esc, _) | (KeyCode::Char('q'), _) => {
                state.close_overlay();
                None
            }
            (KeyCode::Enter, _) => {
                state.overlay_apply();
                None
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                if let Some(o) = state.overlay.as_mut() {
                    o.cursor = o.cursor.saturating_sub(1);
                }
                None
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                if let Some(o) = state.overlay.as_mut() {
                    let max = o.matches.len().saturating_sub(1);
                    o.cursor = (o.cursor + 1).min(max);
                }
                None
            }
            (KeyCode::Char(' '), _) => {
                if let Some(o) = state.overlay.as_mut() {
                    if let Some(m) = o.matches.get_mut(o.cursor) {
                        m.enabled = !m.enabled;
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn draw(f: &mut Frame, state: &mut State) {
        let show_search = state.search_active || !state.search_query.is_empty();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(if show_search { 1 } else { 0 }),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(f.area());

        let visible = state.visible();
        let (picked_count, picked_bytes) = state
            .selected
            .iter()
            .enumerate()
            .filter(|&(_, &s)| s)
            .fold((0usize, 0u64), |(c, b), (i, _)| {
                (c + 1, b + state.files[i].size_bytes)
            });
        let threshold_label = match state.threshold() {
            None => "all sizes".to_string(),
            Some(b) => format!("≥ {}", format_bytes(b)),
        };
        let status = format!(
            " {} / {} files ({}) · {} selected ({})",
            visible.len(),
            state.files.len(),
            threshold_label,
            picked_count,
            format_bytes(picked_bytes),
        );
        f.render_widget(
            Paragraph::new(status).style(
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            chunks[0],
        );

        if show_search {
            let cursor = if state.search_active { "▌" } else { "" };
            let line = Line::from(vec![
                Span::styled(
                    " / ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(state.search_query.clone()),
                Span::styled(cursor, Style::default().fg(Color::Cyan)),
            ]);
            f.render_widget(Paragraph::new(line), chunks[1]);
        }

        let (size_w, kind_w) = visible
            .iter()
            .fold(("size".len(), "kind".len()), |(sw, kw), &i| {
                let f = &state.files[i];
                (
                    sw.max(format_bytes(f.size_bytes).len()),
                    kw.max(kind_from_ext(&f.path).len()),
                )
            });

        let header_indent = " ".repeat(1 + 4);
        let header_line = format!(
            "{header_indent}{:>size_w$}  {:<kind_w$}  path",
            "size", "kind",
        );
        f.render_widget(
            Paragraph::new(header_line).style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            chunks[2],
        );

        let items: Vec<ListItem> = visible
            .iter()
            .map(|&i| row(&state.files[i], i, state, size_w, kind_w))
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶");
        f.render_stateful_widget(list, chunks[3], &mut state.list);

        let footer_spans: Vec<Span> = if state.search_active {
            vec![
                Span::raw(" type to filter · "),
                Span::styled("enter", Style::default().fg(Color::Cyan)),
                Span::raw(" apply · "),
                Span::styled("esc", Style::default().fg(Color::Cyan)),
                Span::raw(" clear"),
            ]
        } else {
            vec![
                Span::styled(" ↑↓", Style::default().fg(Color::Cyan)),
                Span::raw(" nav · "),
                Span::styled("space", Style::default().fg(Color::Cyan)),
                Span::raw(" toggle · "),
                Span::styled("enter", Style::default().fg(Color::Cyan)),
                Span::raw(" delete · "),
                Span::styled("/", Style::default().fg(Color::Cyan)),
                Span::raw(" search · "),
                Span::styled("m", Style::default().fg(Color::Cyan)),
                Span::raw(" min-size · "),
                Span::styled("a", Style::default().fg(Color::Cyan)),
                Span::raw(" auto-mark · "),
                Span::styled("q", Style::default().fg(Color::Cyan)),
                Span::raw(" quit"),
            ]
        };
        f.render_widget(
            Paragraph::new(Line::from(footer_spans)).style(Style::default().fg(Color::Gray)),
            chunks[4],
        );

        if state.overlay.is_some() {
            draw_overlay(f, state);
        }
    }

    fn draw_overlay(f: &mut Frame, state: &State) {
        use ratatui::widgets::{Block, Borders, Clear};
        let Some(overlay) = state.overlay.as_ref() else {
            return;
        };
        let area = centered_rect(72, 60, f.area());
        f.render_widget(Clear, area);

        let inner = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(area);

        let block = Block::default()
            .title(" Auto-mark waste ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::with_capacity(overlay.matches.len() * 3);
        let mut total_files = 0usize;
        let mut total_bytes = 0u64;
        for (i, m) in overlay.matches.iter().enumerate() {
            let checkbox = if m.enabled { "[x]" } else { "[ ]" };
            let count = m.count();
            let bytes = m.bytes(&state.files);
            if m.enabled {
                total_files += count;
                total_bytes += bytes;
            }
            let cursor_mark = if i == overlay.cursor { "▶ " } else { "  " };
            let label_style = if i == overlay.cursor {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::raw(cursor_mark),
                Span::raw(format!("{checkbox} ")),
                Span::styled(m.label.to_string(), label_style),
            ]));
            lines.push(Line::from(vec![Span::styled(
                format!("    {count} files · {}", format_bytes(bytes)),
                Style::default().fg(Color::DarkGray),
            )]));
            lines.push(Line::from(vec![Span::styled(
                format!("    {}", m.description),
                Style::default().fg(Color::DarkGray),
            )]));
            lines.push(Line::from(""));
        }
        f.render_widget(Paragraph::new(lines), inner[0]);

        f.render_widget(
            Paragraph::new(format!(
                "Will mark: {total_files} files · {}",
                format_bytes(total_bytes)
            ))
            .style(
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            inner[1],
        );

        let footer = Line::from(vec![
            Span::styled("↑↓", Style::default().fg(Color::Cyan)),
            Span::raw(" nav · "),
            Span::styled("space", Style::default().fg(Color::Cyan)),
            Span::raw(" toggle · "),
            Span::styled("enter", Style::default().fg(Color::Cyan)),
            Span::raw(" apply · "),
            Span::styled("esc", Style::default().fg(Color::Cyan)),
            Span::raw(" cancel"),
        ]);
        f.render_widget(
            Paragraph::new(footer).style(Style::default().fg(Color::Gray)),
            inner[2],
        );
    }

    fn centered_rect(pct_x: u16, pct_y: u16, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
        let popup_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - pct_y) / 2),
                Constraint::Percentage(pct_y),
                Constraint::Percentage((100 - pct_y) / 2),
            ])
            .split(area);
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - pct_x) / 2),
                Constraint::Percentage(pct_x),
                Constraint::Percentage((100 - pct_x) / 2),
            ])
            .split(popup_layout[1])[1]
    }

    fn row(
        file: &MediaFile,
        i: usize,
        state: &State,
        size_w: usize,
        kind_w: usize,
    ) -> ListItem<'static> {
        let checkbox = if state.selected[i] { "[x]" } else { "[ ]" };
        let size_str = format_bytes(file.size_bytes);
        let kind = kind_from_ext(&file.path);
        ListItem::new(Line::from(vec![
            Span::raw(format!(" {checkbox}  ")),
            Span::raw(format!("{size_str:>size_w$}")),
            Span::raw("  "),
            Span::raw(format!("{kind:<kind_w$}")),
            Span::raw("  "),
            Span::raw(file.path.clone()),
        ]))
    }

    struct TerminalGuard(Terminal<CrosstermBackend<io::Stdout>>);

    impl TerminalGuard {
        fn enter() -> Result<Self> {
            terminal::enable_raw_mode()?;
            let mut stdout = io::stdout();
            execute!(stdout, terminal::EnterAlternateScreen)?;
            let backend = CrosstermBackend::new(stdout);
            let terminal = Terminal::new(backend)?;
            Ok(Self(terminal))
        }
    }

    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(io::stdout(), terminal::LeaveAlternateScreen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mf(path: &str, size: u64) -> MediaFile {
        MediaFile {
            path: path.into(),
            size_bytes: size,
            modified_unix: 0,
        }
    }

    fn mf_at(path: &str, size: u64, modified_unix: i64) -> MediaFile {
        MediaFile {
            path: path.into(),
            size_bytes: size,
            modified_unix,
        }
    }

    #[test]
    fn heuristics_live_photo_motion_pairs_mov_with_photo() {
        let files = vec![
            mf("/DCIM/100APPLE/IMG_0001.HEIC", 4_000_000),
            mf("/DCIM/100APPLE/IMG_0001.MOV", 3_000_000),
            mf("/DCIM/100APPLE/IMG_0002.MOV", 500_000_000),
            mf("/DCIM/100APPLE/IMG_0003.JPG", 2_000_000),
            mf("/DCIM/100APPLE/IMG_0003.MOV", 1_500_000),
        ];
        let hits = heuristics::live_photo_motion(&files);
        assert_eq!(hits, vec![1, 4]);
    }

    #[test]
    fn heuristics_originals_with_edited_marks_originals() {
        let files = vec![
            mf("/DCIM/100APPLE/IMG_1234.HEIC", 4_000_000),
            mf("/DCIM/100APPLE/IMG_E1234.HEIC", 4_500_000),
            mf("/DCIM/100APPLE/IMG_5555.HEIC", 1_000_000),
        ];
        let hits = heuristics::originals_with_edited(&files);
        assert_eq!(hits, vec![0]);
    }

    #[test]
    fn heuristics_old_screenshots_respects_age_and_extension() {
        let files = vec![
            mf_at("/DCIM/100APPLE/IMG_0001.PNG", 1_000_000, 100),
            mf_at("/DCIM/100APPLE/IMG_0002.PNG", 1_000_000, 9999),
            mf_at("/DCIM/100APPLE/IMG_0003.HEIC", 1_000_000, 100),
            mf_at("/Downloads/foo.PNG", 1_000_000, 100),
        ];
        let hits = heuristics::old_screenshots(&files, 1000);
        assert_eq!(hits, vec![0]);
    }

    #[test]
    fn heuristics_exact_duplicates_marks_extra_copies() {
        let files = vec![
            mf("/DCIM/100APPLE/IMG_0001.HEIC", 4_000_000),
            mf("/Downloads/IMG_0001.HEIC", 4_000_000),
            mf("/DCIM/100APPLE/IMG_0001.HEIC.bak", 4_000_000),
            mf("/DCIM/100APPLE/UNIQUE.HEIC", 1_000_000),
        ];
        let hits = heuristics::exact_duplicates(&files);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn kind_from_ext_classifies_known_extensions() {
        assert_eq!(kind_from_ext("/DCIM/IMG.MOV"), "Video");
        assert_eq!(kind_from_ext("/DCIM/IMG.mp4"), "Video");
        assert_eq!(kind_from_ext("/DCIM/IMG.HEIC"), "Photo");
        assert_eq!(kind_from_ext("/DCIM/IMG.jpg"), "Photo");
        assert_eq!(kind_from_ext("/Recordings/m.m4a"), "Audio");
        assert_eq!(kind_from_ext("/Downloads/x.pdf"), "Doc");
        assert_eq!(kind_from_ext("/Books/x.EPUB"), "Doc");
    }

    #[test]
    fn kind_from_ext_falls_back_to_other() {
        assert_eq!(kind_from_ext("/Downloads/NOEXT"), "Other");
        assert_eq!(kind_from_ext("/x.weird"), "Other");
        assert_eq!(kind_from_ext(""), "Other");
    }

    #[test]
    fn top_n_truncates_and_sorts_descending() {
        let files = vec![mf("/a", 10), mf("/b", 100), mf("/c", 50), mf("/d", 1000)];
        let top = top_n_by_size(files, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].path, "/d");
        assert_eq!(top[1].path, "/b");
    }

    #[test]
    fn top_n_does_not_exceed_input_len() {
        let files = vec![mf("/a", 10), mf("/b", 100)];
        let top = top_n_by_size(files, 50);
        assert_eq!(top.len(), 2);
    }

    #[test]
    fn render_file_list_includes_header_rows_and_summary() {
        let files = vec![
            mf("/DCIM/IMG.MOV", 4_200_000_000),
            mf("/Downloads/x.pdf", 50_000_000),
        ];
        let out = render_file_list(&files, 1234, 84_200_000_000);
        assert!(out.contains("size"));
        assert!(out.contains("kind"));
        assert!(out.contains("path"));
        assert!(out.contains("Video"));
        assert!(out.contains("Doc"));
        assert!(out.contains("/DCIM/IMG.MOV"));
        assert!(out.contains("2 files shown"));
        assert!(out.contains("1234"));
    }

    #[test]
    fn render_file_list_handles_empty() {
        let out = render_file_list(&[], 0, 0);
        assert!(out.contains("No files"));
    }

    #[test]
    fn build_confirm_prompt_warns_for_dcim_paths() {
        let picked = vec![mf("/DCIM/IMG.MOV", 100)];
        let prompt = build_confirm_prompt(&picked);
        assert!(prompt.contains("permanently"));
        assert!(prompt.contains("Photos.app"));
    }

    #[test]
    fn build_confirm_prompt_omits_dcim_warning_when_no_dcim() {
        let picked = vec![mf("/Downloads/x.pdf", 100)];
        let prompt = build_confirm_prompt(&picked);
        assert!(prompt.contains("permanently"));
        assert!(!prompt.contains("Photos.app"));
    }
}
