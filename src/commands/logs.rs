//! `quokka logs` — stream parsed syslog entries. Plain mode emits one line
//! per entry to stdout; TUI mode renders a colored live viewer.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::Result;
use owo_colors::OwoColorize;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use crate::device::{Device, LogEntry, LogLevel};

const DEFAULT_MIN_LEVEL: LogLevel = LogLevel::Notice;

#[derive(Debug, Clone)]
pub struct Options {
    pub no_tui: bool,
    pub min_level: LogLevel,
    pub process_filter: Option<String>,
    pub save_path: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            no_tui: false,
            min_level: DEFAULT_MIN_LEVEL,
            process_filter: None,
            save_path: None,
        }
    }
}

pub async fn run(device: &dyn Device, opts: Options) -> Result<()> {
    let rx = device.stream_logs().await?;
    let plain_mode = opts.no_tui || !std::io::stdout().is_terminal();
    if plain_mode {
        plain::run(rx, opts).await
    } else {
        tui::run(rx, opts).await
    }
}

#[derive(Debug, Clone)]
pub struct Filter {
    pub min_level: LogLevel,
    pub process: Option<String>,
}

pub fn matches_filter(entry: &LogEntry, filter: &Filter) -> bool {
    if level_rank(entry.level) < level_rank(filter.min_level) {
        return false;
    }
    if let Some(needle) = &filter.process {
        if !entry
            .process
            .to_lowercase()
            .contains(&needle.to_lowercase())
        {
            return false;
        }
    }
    true
}

fn level_rank(level: LogLevel) -> u8 {
    match level {
        LogLevel::Debug => 0,
        LogLevel::Info => 1,
        LogLevel::Notice => 2,
        LogLevel::Warning => 3,
        LogLevel::Error => 4,
        LogLevel::Fault => 5,
        LogLevel::Unknown => 2, // treat as notice
    }
}

pub fn format_plain(entry: &LogEntry) -> String {
    let ts = format_iso8601(entry.timestamp_unix_ms);
    let proc = match entry.pid {
        Some(pid) => format!("{}[{pid}]", entry.process),
        None => entry.process.clone(),
    };
    format!(
        "{ts}  {proc}  <{}>  {}",
        entry.level.as_str(),
        entry.message
    )
}

fn format_iso8601(ts_ms: Option<i64>) -> String {
    match ts_ms {
        Some(ms) => {
            let secs = ms / 1000;
            let frac = (ms % 1000).unsigned_abs();
            let (year, month, day, hh, mm, ss) = unix_to_ymd_hms(secs);
            format!("{year:04}-{month:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}.{frac:03}Z")
        }
        None => "-".to_string(),
    }
}

fn unix_to_ymd_hms(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let hh = rem / 3600;
    let mm = (rem / 60) % 60;
    let ss = rem % 60;
    // Civil-from-days (Hinnant).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32, hh, mm, ss)
}

pub mod parser {
    use super::*;

    /// Parse a single BSD-syslog-ish iOS log line. Returns a structured
    /// `LogEntry`; if parsing fails the entry has `level: Unknown`,
    /// `process: "?"`, and the raw line as `message` so no data is lost.
    pub fn parse_syslog_line(raw: &str) -> LogEntry {
        // Frames from `syslog_relay` come delimited by `\n\x00`. The trailing
        // delim is already stripped by `read_until_delim`, but real captures
        // sometimes show stray nulls / newlines on either end.
        let trimmed = raw
            .trim_start_matches('\0')
            .trim_start_matches('\n')
            .trim_end_matches('\0')
            .trim_end_matches('\n');

        // Continuation lines (leading whitespace) — caller stitches.
        // Format: "Mmm DD HH:MM:SS host process[pid] <Level>: message"
        // Skip the "Mmm DD HH:MM:SS" prefix (15 chars + 1 space at minimum).
        let mut rest = trimmed;

        let host_start = match find_after_timestamp(rest) {
            Some(idx) => idx,
            None => return unknown_entry(trimmed),
        };
        // BSD syslog: "Mmm DD HH:MM:SS" — bytes 7..15 are the HH:MM:SS slice.
        let time_text = rest.get(7..15).map(|s| s.to_string());
        rest = &rest[host_start..];

        // host process[pid] <Level>: message
        let (host, after_host) = match rest.split_once(' ') {
            Some(pair) => pair,
            None => return unknown_entry(trimmed),
        };

        let (process_token, after_proc) = match after_host.split_once(' ') {
            Some(pair) => pair,
            None => return unknown_entry(trimmed),
        };

        let (process, pid) = parse_process_pid(process_token);

        // Expect "<Level>:" then message.
        let (level, message) = if let Some(end) = after_proc.find('>') {
            if after_proc.starts_with('<') {
                let level_text = &after_proc[1..end];
                let after = &after_proc[end + 1..];
                let after = after.trim_start_matches(':').trim_start();
                (LogLevel::parse(level_text), after.to_string())
            } else {
                (LogLevel::Unknown, after_proc.to_string())
            }
        } else {
            (LogLevel::Unknown, after_proc.to_string())
        };

        LogEntry {
            timestamp_unix_ms: None,
            time_text,
            host: host.to_string(),
            process,
            pid,
            level,
            message,
        }
    }

    fn unknown_entry(raw: &str) -> LogEntry {
        LogEntry {
            timestamp_unix_ms: None,
            time_text: None,
            host: String::new(),
            process: "?".to_string(),
            pid: None,
            level: LogLevel::Unknown,
            message: raw.to_string(),
        }
    }

    /// The iOS syslog timestamp is "Mmm DD HH:MM:SS" (15 chars). Return the
    /// index after that prefix + its trailing space, or None on malformed.
    fn find_after_timestamp(s: &str) -> Option<usize> {
        // Fast path: the 16th byte should be a space.
        let bytes = s.as_bytes();
        if bytes.len() < 16 {
            return None;
        }
        if bytes[15] != b' ' {
            return None;
        }
        Some(16)
    }

    fn parse_process_pid(token: &str) -> (String, Option<u32>) {
        if let Some(open) = token.rfind('[') {
            if token.ends_with(']') {
                let pid_str = &token[open + 1..token.len() - 1];
                if let Ok(pid) = pid_str.parse::<u32>() {
                    return (token[..open].to_string(), Some(pid));
                }
            }
        }
        (token.to_string(), None)
    }

    pub fn is_continuation(raw: &str) -> bool {
        raw.starts_with(' ') || raw.starts_with('\t')
    }
}

mod plain {
    use super::*;

    pub async fn run(
        mut rx: tokio::sync::mpsc::Receiver<Result<LogEntry>>,
        opts: Options,
    ) -> Result<()> {
        let filter = Filter {
            min_level: opts.min_level,
            process: opts.process_filter,
        };
        let mut save_file = if let Some(path) = opts.save_path {
            Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .await?,
            )
        } else {
            None
        };
        let mut out = anstream::stdout();
        while let Some(item) = rx.recv().await {
            match item {
                Ok(entry) => {
                    if !matches_filter(&entry, &filter) {
                        continue;
                    }
                    let line = format_plain(&entry);
                    let colored = colorize(&line, entry.level);
                    writeln!(out, "{colored}")?;
                    if let Some(f) = save_file.as_mut() {
                        f.write_all(line.as_bytes()).await?;
                        f.write_all(b"\n").await?;
                    }
                }
                Err(e) => {
                    eprintln!("! parse error: {e}");
                }
            }
        }
        Ok(())
    }

    fn colorize(line: &str, level: LogLevel) -> String {
        match level {
            LogLevel::Fault => line.bright_red().to_string(),
            LogLevel::Error => line.red().to_string(),
            LogLevel::Warning => line.yellow().to_string(),
            LogLevel::Info | LogLevel::Debug => line.bright_black().to_string(),
            _ => line.to_string(),
        }
    }
}

mod tui {
    use super::*;
    use std::collections::VecDeque;
    use std::io;
    use std::time::{Duration, Instant};

    use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::{execute, terminal};
    use futures::StreamExt;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{List, ListItem, Paragraph};
    use ratatui::{Frame, Terminal};

    const BUFFER_CAP: usize = 10_000;
    const SAVE_NOTICE_TTL: Duration = Duration::from_secs(3);
    const COL_TIME: usize = 8;
    const COL_LEVEL: usize = 6;
    const COL_PROCESS: usize = 22;
    const LEVEL_CYCLE: &[LogLevel] = &[
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Notice,
        LogLevel::Warning,
        LogLevel::Error,
        LogLevel::Fault,
    ];

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum InputMode {
        Normal,
        Search,
        Process,
    }

    struct State {
        buffer: VecDeque<LogEntry>,
        total_received: usize,
        filter: Filter,
        search: String,
        // Cursor offset from the BOTTOM of the filtered stream. 0 = bottom.
        scroll_from_bottom: usize,
        paused: bool,
        pending_while_paused: usize,
        input_mode: InputMode,
        input_buffer: String,
        save_notice: Option<(String, Instant)>,
        stream_ended: bool,
    }

    impl State {
        fn new(filter: Filter) -> Self {
            Self {
                buffer: VecDeque::with_capacity(BUFFER_CAP),
                total_received: 0,
                filter,
                search: String::new(),
                scroll_from_bottom: 0,
                paused: false,
                pending_while_paused: 0,
                input_mode: InputMode::Normal,
                input_buffer: String::new(),
                save_notice: None,
                stream_ended: false,
            }
        }

        fn push(&mut self, entry: LogEntry) {
            self.total_received += 1;
            let popped = if self.buffer.len() >= BUFFER_CAP {
                self.buffer.pop_front();
                true
            } else {
                false
            };
            let passes = matches_filter(&entry, &self.filter);
            self.buffer.push_back(entry);
            if self.paused {
                // Freeze the view: a new visible entry pushes the cursor up
                // one row so the same content stays on screen.
                if passes {
                    self.pending_while_paused += 1;
                    self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(1);
                }
                // If the buffer was full and we popped one, the cursor's
                // distance from the bottom needs to drop by one to stay anchored.
                if popped && self.scroll_from_bottom > 0 {
                    self.scroll_from_bottom -= 1;
                }
            }
        }

        fn filtered(&self) -> Vec<&LogEntry> {
            self.buffer
                .iter()
                .filter(|e| matches_filter(e, &self.filter))
                .collect()
        }

        fn match_count(&self) -> usize {
            if self.search.is_empty() {
                return 0;
            }
            let needle = self.search.to_lowercase();
            self.filtered()
                .iter()
                .filter(|e| row_matches_search(e, &needle))
                .count()
        }

        fn cycle_level(&mut self) {
            let cur = LEVEL_CYCLE
                .iter()
                .position(|l| *l == self.filter.min_level)
                .unwrap_or(0);
            let next = (cur + 1) % LEVEL_CYCLE.len();
            self.filter.min_level = LEVEL_CYCLE[next];
            self.scroll_from_bottom = 0;
        }

        fn clear_buffer(&mut self) {
            self.buffer.clear();
            self.pending_while_paused = 0;
            self.scroll_from_bottom = 0;
        }

        fn jump_top(&mut self) {
            let n = self.filtered().len();
            self.scroll_from_bottom = n.saturating_sub(1);
        }

        fn jump_bottom(&mut self) {
            self.scroll_from_bottom = 0;
            // Re-engage live tail.
            self.paused = false;
            self.pending_while_paused = 0;
        }

        fn page_up(&mut self, page: usize) {
            let n = self.filtered().len();
            self.scroll_from_bottom =
                (self.scroll_from_bottom + page.max(1)).min(n.saturating_sub(1));
        }

        fn page_down(&mut self, page: usize) {
            self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(page.max(1));
        }

        /// Move cursor to next / previous match (wraps).
        fn jump_to_match(&mut self, forward: bool) {
            if self.search.is_empty() {
                return;
            }
            let needle = self.search.to_lowercase();
            let filtered = self.filtered();
            let n = filtered.len();
            if n == 0 {
                return;
            }
            // Current cursor row from TOP.
            let cur_top = n.saturating_sub(1).saturating_sub(self.scroll_from_bottom);
            let match_indices: Vec<usize> = filtered
                .iter()
                .enumerate()
                .filter_map(|(i, e)| {
                    if row_matches_search(e, &needle) {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect();
            if match_indices.is_empty() {
                return;
            }
            let target = if forward {
                match_indices
                    .iter()
                    .find(|&&i| i > cur_top)
                    .or_else(|| match_indices.first())
                    .copied()
                    .unwrap()
            } else {
                match_indices
                    .iter()
                    .rev()
                    .find(|&&i| i < cur_top)
                    .or_else(|| match_indices.last())
                    .copied()
                    .unwrap()
            };
            self.scroll_from_bottom = n.saturating_sub(1).saturating_sub(target);
        }
    }

    pub async fn run(
        mut rx: tokio::sync::mpsc::Receiver<Result<LogEntry>>,
        opts: Options,
    ) -> Result<()> {
        let mut term = TerminalGuard::enter()?;
        let mut state = State::new(Filter {
            min_level: opts.min_level,
            process: opts.process_filter,
        });
        let mut events = EventStream::new();

        let outcome: Result<()> = loop {
            term.0.draw(|f| draw(f, &state))?;

            // Save notice TTL.
            if let Some((_, when)) = state.save_notice {
                if when.elapsed() > SAVE_NOTICE_TTL {
                    state.save_notice = None;
                }
            }

            tokio::select! {
                biased;
                maybe_event = events.next() => {
                    if let Some(Ok(event)) = maybe_event {
                        if let Some(quit) = handle_event(event, &mut state).await {
                            if quit { break Ok(()); }
                        }
                    }
                }
                msg = rx.recv() => {
                    match msg {
                        Some(Ok(entry)) => state.push(entry),
                        Some(Err(_)) | None => state.stream_ended = true,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            }
        };
        outcome
    }

    /// Returns `Some(true)` to quit, `Some(false)` for handled (no-op),
    /// `None` for unhandled.
    async fn handle_event(event: Event, state: &mut State) -> Option<bool> {
        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event
        else {
            return None;
        };

        // Input-capture modes intercept most keys.
        if state.input_mode != InputMode::Normal {
            match (code, modifiers) {
                (KeyCode::Esc, _) => {
                    state.input_buffer.clear();
                    state.input_mode = InputMode::Normal;
                }
                (KeyCode::Enter, _) => {
                    match state.input_mode {
                        InputMode::Search => {
                            state.search = state.input_buffer.clone();
                            state.scroll_from_bottom = 0;
                        }
                        InputMode::Process => {
                            state.filter.process = if state.input_buffer.is_empty() {
                                None
                            } else {
                                Some(state.input_buffer.clone())
                            };
                            state.scroll_from_bottom = 0;
                        }
                        InputMode::Normal => {}
                    }
                    state.input_buffer.clear();
                    state.input_mode = InputMode::Normal;
                }
                (KeyCode::Backspace, _) => {
                    state.input_buffer.pop();
                }
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Some(true),
                (KeyCode::Char(c), _) => state.input_buffer.push(c),
                _ => {}
            }
            return Some(false);
        }

        match (code, modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => Some(true),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(true),
            (KeyCode::Char(' '), _) => {
                state.paused = !state.paused;
                if !state.paused {
                    state.pending_while_paused = 0;
                    state.scroll_from_bottom = 0;
                }
                Some(false)
            }
            (KeyCode::Char('l'), _) => {
                state.cycle_level();
                Some(false)
            }
            (KeyCode::Char('p'), _) => {
                state.input_mode = InputMode::Process;
                state.input_buffer = state.filter.process.clone().unwrap_or_default();
                Some(false)
            }
            (KeyCode::Char('/'), _) => {
                state.input_mode = InputMode::Search;
                state.input_buffer = state.search.clone();
                Some(false)
            }
            (KeyCode::Char('n'), _) => {
                state.jump_to_match(true);
                state.paused = true;
                Some(false)
            }
            (KeyCode::Char('N'), _) => {
                state.jump_to_match(false);
                state.paused = true;
                Some(false)
            }
            (KeyCode::Char('w'), _) => {
                let path = save_filename();
                let result = write_buffer(&path, &state.filtered()).await;
                match result {
                    Ok(n) => {
                        state.save_notice = Some((
                            format!("Saved {n} lines to {}", path.display()),
                            Instant::now(),
                        ));
                    }
                    Err(e) => {
                        state.save_notice = Some((format!("Save failed: {e}"), Instant::now()));
                    }
                }
                Some(false)
            }
            (KeyCode::Char('c'), _) => {
                state.clear_buffer();
                Some(false)
            }
            (KeyCode::Char('g'), _) | (KeyCode::Home, _) => {
                state.jump_top();
                state.paused = true;
                Some(false)
            }
            (KeyCode::Char('G'), _) | (KeyCode::End, _) => {
                state.jump_bottom();
                Some(false)
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                let n = state.filtered().len();
                state.scroll_from_bottom = (state.scroll_from_bottom + 1).min(n.saturating_sub(1));
                state.paused = true;
                Some(false)
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                state.scroll_from_bottom = state.scroll_from_bottom.saturating_sub(1);
                if state.scroll_from_bottom == 0 {
                    state.paused = false;
                    state.pending_while_paused = 0;
                }
                Some(false)
            }
            (KeyCode::PageUp, _) => {
                state.page_up(10);
                state.paused = true;
                Some(false)
            }
            (KeyCode::PageDown, _) => {
                state.page_down(10);
                Some(false)
            }
            _ => None,
        }
    }

    fn save_filename() -> std::path::PathBuf {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let (y, m, d, hh, mm, ss) = unix_to_ymd_hms(now as i64);
        std::path::PathBuf::from(format!(
            "qk-logs-{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}.log"
        ))
    }

    async fn write_buffer(path: &std::path::Path, entries: &[&LogEntry]) -> std::io::Result<usize> {
        let mut f = tokio::fs::File::create(path).await?;
        // Header row so the file matches the on-screen table.
        let header = format!(
            "{time:<COL_TIME$}  {lvl:<COL_LEVEL$}  {proc:<COL_PROCESS$}  message\n",
            time = "time",
            lvl = "level",
            proc = "process",
        );
        f.write_all(header.as_bytes()).await?;
        for e in entries {
            f.write_all(format_tabular(e).as_bytes()).await?;
            f.write_all(b"\n").await?;
        }
        f.flush().await?;
        Ok(entries.len())
    }

    /// Single-line tabular rendering matching the on-screen columns. No ANSI
    /// colors — meant for piping/grepping/diffing.
    pub(super) fn format_tabular(entry: &LogEntry) -> String {
        format!(
            "{time:<COL_TIME$}  {lvl:<COL_LEVEL$}  {proc:<COL_PROCESS$}  {msg}",
            time = entry.time_text.as_deref().unwrap_or("--:--:--"),
            lvl = level_short(entry.level),
            proc = proc_with_pid(entry),
            msg = entry.message,
        )
    }

    fn draw(f: &mut Frame, state: &State) {
        let show_input = state.input_mode != InputMode::Normal;
        let show_save = state.save_notice.is_some();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                              // header
                Constraint::Length(1),                              // filter/search summary
                Constraint::Length(if show_input { 1 } else { 0 }), // input line
                Constraint::Length(if show_save { 1 } else { 0 }),  // save notice
                Constraint::Length(1),                              // column header
                Constraint::Min(1),                                 // log list
                Constraint::Length(2),                              // footer (2 rows)
            ])
            .split(f.area());

        // Header.
        let mode_text = if state.stream_ended {
            "✗ Stream ended".to_string()
        } else if state.paused {
            if state.pending_while_paused > 0 {
                format!("⏸ Paused · {} new", state.pending_while_paused)
            } else {
                "⏸ Paused".to_string()
            }
        } else {
            "▶ Live".to_string()
        };
        let filtered_n = state.filtered().len();
        let match_text = if state.search.is_empty() {
            String::new()
        } else {
            format!(" · {} matched", state.match_count())
        };
        let header = format!(
            " qk logs · {} lines · {} visible{} · {mode_text}",
            state.total_received, filtered_n, match_text
        );
        let header_style = if state.stream_ended {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else if state.paused {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        };
        f.render_widget(Paragraph::new(header).style(header_style), chunks[0]);

        // Filter / search summary line.
        let mut summary_spans: Vec<Span> = vec![Span::raw(" Filter: level≥")];
        summary_spans.push(Span::styled(
            state.filter.min_level.as_str(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        if let Some(p) = &state.filter.process {
            summary_spans.push(Span::raw(" · process="));
            summary_spans.push(Span::styled(p.clone(), Style::default().fg(Color::Cyan)));
        }
        if !state.search.is_empty() {
            summary_spans.push(Span::raw("    Search: "));
            summary_spans.push(Span::styled(
                format!("\"{}\"", state.search),
                Style::default().fg(Color::Magenta),
            ));
        }
        f.render_widget(
            Paragraph::new(Line::from(summary_spans)).style(Style::default().fg(Color::Gray)),
            chunks[1],
        );

        // Input line (search or process), when active.
        if show_input {
            let prompt = match state.input_mode {
                InputMode::Search => " / ",
                InputMode::Process => " process: ",
                InputMode::Normal => "",
            };
            let line = Line::from(vec![
                Span::styled(
                    prompt,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(state.input_buffer.clone()),
                Span::styled("▌", Style::default().fg(Color::Cyan)),
            ]);
            f.render_widget(Paragraph::new(line), chunks[2]);
        }

        // Save notice.
        if let Some((msg, _)) = &state.save_notice {
            f.render_widget(
                Paragraph::new(format!(" {msg}")).style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                chunks[3],
            );
        }

        // Log list — render slice ending at cursor.
        // Column header.
        f.render_widget(Paragraph::new(column_header_line()), chunks[4]);

        let viewport = chunks[5].height as usize;
        let filtered = state.filtered();
        let n = filtered.len();
        let end = n.saturating_sub(state.scroll_from_bottom);
        let start = end.saturating_sub(viewport);
        let needle = if state.search.is_empty() {
            None
        } else {
            Some(state.search.to_lowercase())
        };
        let items: Vec<ListItem> = filtered[start..end]
            .iter()
            .map(|e| format_row_with_search(e, needle.as_deref()))
            .collect();
        f.render_widget(List::new(items), chunks[5]);

        // Footer — 2 rows of keybinds, context-aware.
        let (footer_l1, footer_l2): (Line, Line) = if state.input_mode != InputMode::Normal {
            (
                Line::from(vec![
                    Span::raw(" type to filter · "),
                    Span::styled("enter", Style::default().fg(Color::Cyan)),
                    Span::raw(" apply · "),
                    Span::styled("esc", Style::default().fg(Color::Cyan)),
                    Span::raw(" cancel"),
                ]),
                Line::from(""),
            )
        } else {
            (
                Line::from(vec![
                    Span::styled(" ↑↓", Style::default().fg(Color::Cyan)),
                    Span::raw(" scroll · "),
                    Span::styled("PgUp/PgDn", Style::default().fg(Color::Cyan)),
                    Span::raw(" page · "),
                    Span::styled("g/G", Style::default().fg(Color::Cyan)),
                    Span::raw(" top/bottom · "),
                    Span::styled("space", Style::default().fg(Color::Cyan)),
                    Span::raw(" pause"),
                ]),
                Line::from(vec![
                    Span::styled(" l", Style::default().fg(Color::Cyan)),
                    Span::raw(" level · "),
                    Span::styled("p", Style::default().fg(Color::Cyan)),
                    Span::raw(" process · "),
                    Span::styled("/", Style::default().fg(Color::Cyan)),
                    Span::raw(" search · "),
                    Span::styled("n/N", Style::default().fg(Color::Cyan)),
                    Span::raw(" next/prev · "),
                    Span::styled("w", Style::default().fg(Color::Cyan)),
                    Span::raw(" save · "),
                    Span::styled("c", Style::default().fg(Color::Cyan)),
                    Span::raw(" clear · "),
                    Span::styled("q", Style::default().fg(Color::Cyan)),
                    Span::raw(" quit"),
                ]),
            )
        };
        f.render_widget(
            Paragraph::new(vec![footer_l1, footer_l2]).style(Style::default().fg(Color::Gray)),
            chunks[6],
        );
    }

    fn row_matches_search(entry: &LogEntry, needle_lower: &str) -> bool {
        entry.message.to_lowercase().contains(needle_lower)
            || entry.process.to_lowercase().contains(needle_lower)
    }

    fn level_style(level: LogLevel) -> Style {
        match level {
            LogLevel::Fault => Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
            LogLevel::Error => Style::default().fg(Color::Red),
            LogLevel::Warning => Style::default().fg(Color::Yellow),
            LogLevel::Notice => Style::default(),
            LogLevel::Info | LogLevel::Debug => Style::default().fg(Color::DarkGray),
            LogLevel::Unknown => Style::default(),
        }
    }

    fn level_short(level: LogLevel) -> &'static str {
        match level {
            LogLevel::Fault => "FAULT",
            LogLevel::Error => "ERROR",
            LogLevel::Warning => "WARN",
            LogLevel::Notice => "NOTE",
            LogLevel::Info => "INFO",
            LogLevel::Debug => "DEBUG",
            LogLevel::Unknown => "?",
        }
    }

    fn truncate(s: &str, w: usize) -> String {
        let chars: Vec<char> = s.chars().collect();
        if chars.len() <= w {
            format!("{s:<w$}")
        } else {
            // -1 for the ellipsis
            let kept: String = chars.iter().take(w.saturating_sub(1)).collect();
            format!("{kept}…")
        }
    }

    fn proc_with_pid(entry: &LogEntry) -> String {
        match entry.pid {
            Some(pid) => format!("{}[{pid}]", entry.process),
            None => entry.process.clone(),
        }
    }

    /// Column header line that sits above the log list.
    fn column_header_line() -> Line<'static> {
        Line::from(Span::styled(
            format!(
                " {time:<COL_TIME$}  {lvl:<COL_LEVEL$}  {proc:<COL_PROCESS$}  message",
                time = "time",
                lvl = "level",
                proc = "process",
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ))
    }

    fn format_row_with_search(entry: &LogEntry, needle: Option<&str>) -> ListItem<'static> {
        let row_style = level_style(entry.level);
        let time_col = truncate(entry.time_text.as_deref().unwrap_or("--:--:--"), COL_TIME);
        let level_col = truncate(level_short(entry.level), COL_LEVEL);
        let proc_col = truncate(&proc_with_pid(entry), COL_PROCESS);
        let prefix = format!(" {time_col}  {level_col}  {proc_col}  ");

        let mut spans: Vec<Span<'static>> = Vec::with_capacity(4);
        // Time dim, level styled by severity, process default-with-dim,
        // message in the level color.
        spans.push(Span::styled(
            format!(" {time_col}  "),
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            format!("{level_col}  "),
            level_style(entry.level).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("{proc_col}  "),
            Style::default().fg(Color::Cyan),
        ));

        let msg = &entry.message;
        let needle_l = needle.map(|n| n.to_lowercase()).filter(|n| !n.is_empty());
        if let Some(needle_l) = needle_l {
            let lower = msg.to_lowercase();
            let mut i = 0;
            while i < msg.len() {
                match lower[i..].find(&needle_l) {
                    Some(off) => {
                        let start = i + off;
                        let end = start + needle_l.len();
                        if start > i {
                            spans.push(Span::styled(msg[i..start].to_string(), row_style));
                        }
                        spans.push(Span::styled(
                            msg[start..end].to_string(),
                            Style::default()
                                .bg(Color::Yellow)
                                .fg(Color::Black)
                                .add_modifier(Modifier::BOLD),
                        ));
                        i = end;
                    }
                    None => {
                        spans.push(Span::styled(msg[i..].to_string(), row_style));
                        break;
                    }
                }
            }
        } else {
            spans.push(Span::styled(msg.clone(), row_style));
        }

        // Silence unused-var lint if `prefix` ends up unused in future tweaks.
        let _ = prefix;
        ListItem::new(Line::from(spans))
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
    use super::parser::*;
    use super::*;

    fn entry(process: &str, level: LogLevel) -> LogEntry {
        LogEntry {
            timestamp_unix_ms: None,
            time_text: None,
            host: "host".into(),
            process: process.into(),
            pid: Some(1),
            level,
            message: "msg".into(),
        }
    }

    #[test]
    fn matches_filter_respects_min_level() {
        let f = Filter {
            min_level: LogLevel::Warning,
            process: None,
        };
        assert!(!matches_filter(&entry("p", LogLevel::Info), &f));
        assert!(!matches_filter(&entry("p", LogLevel::Notice), &f));
        assert!(matches_filter(&entry("p", LogLevel::Warning), &f));
        assert!(matches_filter(&entry("p", LogLevel::Error), &f));
        assert!(matches_filter(&entry("p", LogLevel::Fault), &f));
    }

    #[test]
    fn matches_filter_process_substring_case_insensitive() {
        let f = Filter {
            min_level: LogLevel::Debug,
            process: Some("springboard".into()),
        };
        assert!(matches_filter(&entry("SpringBoard", LogLevel::Debug), &f));
        assert!(matches_filter(
            &entry("springboardhelper", LogLevel::Debug),
            &f
        ));
        assert!(!matches_filter(&entry("mediaserverd", LogLevel::Debug), &f));
    }

    #[test]
    fn parse_syslog_line_extracts_fields() {
        let raw =
            "Nov 14 22:13:20 Lucass-iPhone SpringBoard[63] <Warning>: Bluetooth: reconnecting";
        let e = parse_syslog_line(raw);
        assert_eq!(e.process, "SpringBoard");
        assert_eq!(e.pid, Some(63));
        assert_eq!(e.level, LogLevel::Warning);
        assert!(e.message.contains("Bluetooth"));
    }

    #[test]
    fn parse_syslog_line_handles_missing_pid() {
        let raw = "Nov 14 22:13:20 host process <Error>: boom";
        let e = parse_syslog_line(raw);
        assert_eq!(e.process, "process");
        assert_eq!(e.pid, None);
        assert_eq!(e.level, LogLevel::Error);
    }

    #[test]
    fn parse_syslog_line_garbage_becomes_unknown() {
        let raw = "totally not a syslog line";
        let e = parse_syslog_line(raw);
        assert_eq!(e.process, "?");
        assert_eq!(e.level, LogLevel::Unknown);
        assert!(e.message.contains("totally"));
    }

    #[test]
    fn is_continuation_detects_leading_whitespace() {
        assert!(is_continuation("    continuation"));
        assert!(is_continuation("\tcontinuation"));
        assert!(!is_continuation("Nov 14 ..."));
    }

    #[test]
    fn log_level_parse_canonical_names_case_insensitive() {
        assert_eq!(LogLevel::parse("Debug"), LogLevel::Debug);
        assert_eq!(LogLevel::parse("INFO"), LogLevel::Info);
        assert_eq!(LogLevel::parse("notice"), LogLevel::Notice);
        assert_eq!(LogLevel::parse("Warning"), LogLevel::Warning);
        assert_eq!(LogLevel::parse("Error"), LogLevel::Error);
        assert_eq!(LogLevel::parse("Fault"), LogLevel::Fault);
        assert_eq!(LogLevel::parse(" warning "), LogLevel::Warning);
    }

    #[test]
    fn log_level_parse_recognises_aliases_ios_actually_emits() {
        // The syslog stream from real devices uses these aliases — losing
        // any of them silently downgrades the entry to Unknown.
        assert_eq!(LogLevel::parse("warn"), LogLevel::Warning);
        assert_eq!(LogLevel::parse("err"), LogLevel::Error);
        assert_eq!(LogLevel::parse("critical"), LogLevel::Fault);
        assert_eq!(LogLevel::parse("emergency"), LogLevel::Fault);
        assert_eq!(LogLevel::parse("alert"), LogLevel::Fault);
    }

    #[test]
    fn log_level_parse_unknown_does_not_panic() {
        assert_eq!(LogLevel::parse(""), LogLevel::Unknown);
        assert_eq!(LogLevel::parse("garbage"), LogLevel::Unknown);
    }

    #[test]
    fn log_level_as_str_round_trips_through_parse() {
        for lvl in [
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Notice,
            LogLevel::Warning,
            LogLevel::Error,
            LogLevel::Fault,
        ] {
            assert_eq!(LogLevel::parse(lvl.as_str()), lvl);
        }
    }

    #[test]
    fn parse_syslog_line_captures_time_text_slice() {
        let raw = "Nov 14 22:13:20 host SpringBoard[63] <Notice>: hello";
        let e = parse_syslog_line(raw);
        assert_eq!(e.time_text.as_deref(), Some("22:13:20"));
        assert_eq!(e.host, "host");
        assert_eq!(e.message, "hello");
    }

    #[test]
    fn parse_syslog_line_strips_null_and_newline_framing() {
        // `syslog_relay` delivers frames as "\nLINE\0"; trim_start/end strip
        // both. A regression would push the leading byte into the date column
        // and the whole line would fall back to Unknown.
        let raw = "\nNov 14 22:13:20 host process[7] <Error>: boom\0";
        let e = parse_syslog_line(raw);
        assert_eq!(e.process, "process");
        assert_eq!(e.pid, Some(7));
        assert_eq!(e.level, LogLevel::Error);
        assert_eq!(e.message, "boom");
    }

    #[test]
    fn parse_syslog_line_non_numeric_pid_keeps_brackets_in_process() {
        // `process[abc]` shouldn't crash and shouldn't claim a PID it can't parse.
        let raw = "Nov 14 22:13:20 host weird[abc] <Info>: x";
        let e = parse_syslog_line(raw);
        assert_eq!(e.pid, None);
        assert_eq!(e.process, "weird[abc]");
    }

    #[test]
    fn parse_syslog_line_missing_level_brackets_keeps_message_intact() {
        // No `<Level>:` — the parser keeps everything after the process token
        // as the message and marks level Unknown.
        let raw = "Nov 14 22:13:20 host p[1] just a message";
        let e = parse_syslog_line(raw);
        assert_eq!(e.level, LogLevel::Unknown);
        assert!(e.message.contains("just a message"));
    }

    #[test]
    fn format_plain_renders_dash_when_timestamp_missing() {
        let e = LogEntry {
            timestamp_unix_ms: None,
            time_text: None,
            host: "h".into(),
            process: "p".into(),
            pid: None,
            level: LogLevel::Info,
            message: "msg".into(),
        };
        let out = format_plain(&e);
        assert!(
            out.starts_with("-  "),
            "missing timestamp should render as a leading dash: {out:?}"
        );
        // No PID → no `[...]` suffix on the process token.
        assert!(!out.contains("p["));
    }

    #[test]
    fn format_plain_includes_iso_ts_or_dash() {
        let e = LogEntry {
            timestamp_unix_ms: Some(1_700_000_000_000),
            time_text: None,
            host: "h".into(),
            process: "p".into(),
            pid: Some(7),
            level: LogLevel::Warning,
            message: "hello".into(),
        };
        let out = format_plain(&e);
        assert!(out.contains("2023-11-14T22:13:20.000Z"));
        assert!(out.contains("p[7]"));
        assert!(out.contains("<Warning>"));
        assert!(out.contains("hello"));
    }
}
