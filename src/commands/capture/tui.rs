//! Phase 6 TUI core.
//!
//! - **6.1:** state types + `ingest` + `apply_filter` (no rendering).
//! - **6.2:** Stream-view rendering via `App::draw`. Hosts view is a
//!   placeholder; full tree-table arrives in 6.3.
//!
//! The `App` is the single source of truth a future crossterm event loop
//! (Phase 6.4) will mutate from key events. Keeping draw side-effect-free
//! (no I/O, no async) lets the whole pipeline be snapshot-tested with
//! `ratatui::backend::TestBackend`.

// Most of the surface is scaffolding consumed in later phases (event loop,
// prompt handling). Tests in this file exercise `ingest`, `apply_filter`,
// and the draw path; the rest is verified once 6.4 wires it up.
#![allow(dead_code)]

use std::collections::{HashSet, VecDeque};
use std::time::Instant;

use ratatui::layout::{Alignment, Constraint, Direction as LayoutDir, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::device::Packet;

use super::pcap_io;
use super::style as palette;
use super::{parse_summary, Direction as PktDir, Filter, HostAggregator, ParsedPacket, Protocol};

/// Ring buffer capacity for the live row buffer. Spec value; tests use
/// [`App::with_capacity`] to keep overflow assertions cheap.
pub const DEFAULT_ROW_CAP: usize = 5000;

/// Minimum viable terminal size. Below this we render a centred "resize"
/// hint instead of the full layout — anything smaller would clip the
/// table beyond legibility.
const MIN_COLS: u16 = 80;
const MIN_ROWS: u16 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Stream,
    Hosts,
}

/// One row in the live buffer. Stores the original packet plus the parsed
/// summary computed exactly once at ingest time — the rest of the TUI
/// (filter re-checks, render, hosts rebuild) reuses the cached value
/// instead of re-running `parse_summary` per redraw.
#[derive(Debug, Clone)]
pub struct DisplayRow {
    pub pkt: Packet,
    pub parsed: Option<ParsedPacket>,
    /// Wall-clock instant the packet entered the buffer. Used by
    /// [`super::HostAggregator::add_at`] so filter replays preserve the
    /// host's original `first_seen` instead of resetting it to the moment
    /// the user toggled a filter.
    pub arrival: Instant,
}

impl DisplayRow {
    fn new(pkt: Packet) -> Self {
        let parsed = parse_summary(&pkt);
        Self {
            pkt,
            parsed,
            arrival: Instant::now(),
        }
    }
}

/// Which filter slot the inline prompt is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterField {
    App,
    Pid,
    Port,
    Proto,
    Interface,
}

impl FilterField {
    /// Wire label shown in the prompt prefix (`:app ...`, `:pid ...`).
    fn wire_label(self) -> &'static str {
        match self {
            FilterField::App => "app",
            FilterField::Pid => "pid",
            FilterField::Port => "port",
            FilterField::Proto => "proto",
            FilterField::Interface => "iface",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PromptState {
    pub field: FilterField,
    pub buffer: String,
    /// Inline validation message rendered next to the prompt. `None` when
    /// the buffer is either empty or syntactically acceptable.
    pub error: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct StreamViewState {
    pub selected: usize,
    pub scroll_offset: usize,
}

#[derive(Debug, Default, Clone)]
pub struct HostsViewState {
    pub selected_process: usize,
    pub selected_host: Option<usize>,
    /// PIDs whose host list is collapsed in the tree-table.
    pub collapsed: HashSet<u32>,
}

impl HostsViewState {
    /// Flip the collapsed flag for `pid`. Used by the future event loop's
    /// Enter handler — split out as its own method so 6.3 can unit-test
    /// the toggle without the rest of the input plumbing.
    pub fn toggle_collapse(&mut self, pid: u32) {
        if !self.collapsed.remove(&pid) {
            self.collapsed.insert(pid);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Stats {
    /// Packets that survived the filter and entered the buffer. Pre-filter
    /// rejects do not count — Stream and Hosts views must agree on this.
    pub count: usize,
    /// Producer-side drop counter as reported by `PacketStream::dropped`.
    /// Updated by the event loop in Phase 6.4; stays at zero in headless
    /// tests.
    pub dropped: u64,
    pub started_at: Instant,
    /// Last error returned by the `--save` writer, if any. Surfaced on the
    /// top bar so a full disk or revoked permission doesn't silently
    /// truncate the pcapng — see C2 from the Phase 6 review.
    pub save_error: Option<String>,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            count: 0,
            dropped: 0,
            started_at: Instant::now(),
            save_error: None,
        }
    }
}

pub struct App {
    pub view: View,
    pub rows: VecDeque<DisplayRow>,
    pub aggregator: HostAggregator,
    pub filter: Filter,
    pub stream_state: StreamViewState,
    pub hosts_state: HostsViewState,
    pub prompt: Option<PromptState>,
    pub stats: Stats,
    cap: usize,
    /// Set when state changes; the future redraw tick consults this to
    /// avoid re-rendering an idle frame.
    pub dirty: bool,
}

impl App {
    pub fn new(filter: Filter, view: View) -> Self {
        Self::with_capacity(filter, view, DEFAULT_ROW_CAP)
    }

    pub fn with_capacity(filter: Filter, view: View, cap: usize) -> Self {
        Self {
            view,
            rows: VecDeque::with_capacity(cap),
            aggregator: HostAggregator::new(),
            filter,
            stream_state: StreamViewState::default(),
            hosts_state: HostsViewState::default(),
            prompt: None,
            stats: Stats::default(),
            cap,
            dirty: false,
        }
    }

    /// Push one packet through the filter and into the buffer + aggregator.
    /// Non-matching packets are silently dropped — they never enter storage,
    /// never update `stats.count`, and never feed the host aggregator.
    pub fn ingest(&mut self, pkt: Packet) {
        if !self.filter.matches_packet(&pkt) {
            return;
        }
        let row = DisplayRow::new(pkt);
        if !self.filter.matches_parsed(row.parsed.as_ref()) {
            return;
        }
        if let Some(p) = &row.parsed {
            self.aggregator.add_at(&row.pkt, p, row.arrival);
        }
        self.rows.push_back(row);
        if self.rows.len() > self.cap {
            self.rows.pop_front();
        }
        self.stats.count += 1;
        self.dirty = true;
    }

    /// Replace the active filter. The "filter is a lens" semantic from the
    /// spec: surviving rows are kept (and re-fed to a fresh aggregator),
    /// non-matching rows are dropped from storage outright.
    pub fn apply_filter(&mut self, new_filter: Filter) {
        self.filter = new_filter;
        let filter = &self.filter;
        self.rows
            .retain(|r| filter.matches_packet(&r.pkt) && filter.matches_parsed(r.parsed.as_ref()));
        self.aggregator = HostAggregator::new();
        for r in &self.rows {
            if let Some(p) = &r.parsed {
                // `r.arrival` was captured at ingest time so the replayed
                // aggregator keeps each host's original `first_seen`.
                self.aggregator.add_at(&r.pkt, p, r.arrival);
            }
        }
        // Selection indexes may now point past the end; clamp them.
        let len = self.rows.len();
        if len == 0 {
            self.stream_state.selected = 0;
            self.stream_state.scroll_offset = 0;
        } else if self.stream_state.selected >= len {
            self.stream_state.selected = len - 1;
        }
        // Hosts view: clamp the process/host selection against the new
        // aggregator so a filter that drops the highlighted process
        // doesn't leave the detail pane pointing into thin air.
        let proc_count = self.aggregator.per_proc.len();
        if proc_count == 0 {
            self.hosts_state.selected_process = 0;
            self.hosts_state.selected_host = None;
        } else {
            if self.hosts_state.selected_process >= proc_count {
                self.hosts_state.selected_process = proc_count - 1;
                self.hosts_state.selected_host = None;
            }
            if let Some(h) = self.hosts_state.selected_host {
                let host_count = self
                    .aggregator
                    .per_proc
                    .values()
                    .nth(self.hosts_state.selected_process)
                    .map(|m| m.len())
                    .unwrap_or(0);
                if h >= host_count {
                    self.hosts_state.selected_host = if host_count == 0 {
                        None
                    } else {
                        Some(host_count - 1)
                    };
                }
            }
        }
        self.dirty = true;
    }

    /// Entry point for the future event loop's draw tick. Dispatches by
    /// view, with a small-terminal fallback when the layout would not fit.
    /// Clears the dirty flag so the next idle tick skips re-rendering.
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < MIN_COLS || area.height < MIN_ROWS {
            draw_too_small(frame, area);
        } else {
            match self.view {
                View::Stream => draw_stream(frame, self),
                View::Hosts => draw_hosts(frame, self),
            }
        }
        self.dirty = false;
    }

    /// Dispatch one key event. Pure — no I/O, no terminal calls, no async.
    /// The caller (event loop) consults the returned [`KeyOutcome`] and
    /// decides whether to redraw or exit.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> KeyOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        // Ctrl-C at any state quits — matches the event loop's outer
        // signal::ctrl_c so users get the same behaviour whether the
        // terminal forwards the signal or sends the key event.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return KeyOutcome::Quit;
        }
        if self.prompt.is_some() {
            self.handle_prompt_key(key)
        } else {
            self.handle_idle_key(key)
        }
    }

    fn handle_idle_key(&mut self, key: crossterm::event::KeyEvent) -> KeyOutcome {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => KeyOutcome::Quit,
            KeyCode::Char('a') => self.open_prompt(FilterField::App),
            KeyCode::Char('p') => self.open_prompt(FilterField::Proto),
            KeyCode::Char('P') => self.open_prompt(FilterField::Port),
            KeyCode::Char('i') => self.open_prompt(FilterField::Interface),
            KeyCode::Char('d') => self.open_prompt(FilterField::Pid),
            KeyCode::Char('c') => {
                self.apply_filter(Filter::default());
                self.dirty = true;
                KeyOutcome::Dirty
            }
            KeyCode::Tab => {
                self.view = match self.view {
                    View::Stream => View::Hosts,
                    View::Hosts => View::Stream,
                };
                self.dirty = true;
                KeyOutcome::Dirty
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::Enter => self.activate_selection(),
            _ => KeyOutcome::Continue,
        }
    }

    fn open_prompt(&mut self, field: FilterField) -> KeyOutcome {
        self.prompt = Some(PromptState {
            field,
            buffer: String::new(),
            error: None,
        });
        self.dirty = true;
        KeyOutcome::Dirty
    }

    fn handle_prompt_key(&mut self, key: crossterm::event::KeyEvent) -> KeyOutcome {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.dirty = true;
                KeyOutcome::Dirty
            }
            KeyCode::Enter => self.commit_prompt(),
            KeyCode::Backspace => {
                if let Some(p) = self.prompt.as_mut() {
                    p.buffer.pop();
                    p.error = None;
                }
                self.dirty = true;
                KeyOutcome::Dirty
            }
            KeyCode::Char(c) => {
                if let Some(p) = self.prompt.as_mut() {
                    p.buffer.push(c);
                    p.error = None;
                }
                self.dirty = true;
                KeyOutcome::Dirty
            }
            _ => KeyOutcome::Continue,
        }
    }

    /// Parse the prompt buffer for the active field and either apply the
    /// new filter (success — prompt closes) or surface an inline error
    /// (prompt stays open). Empty buffer clears the corresponding field.
    fn commit_prompt(&mut self) -> KeyOutcome {
        let Some(prompt) = self.prompt.as_ref() else {
            return KeyOutcome::Continue;
        };
        let buf = prompt.buffer.trim().to_string();
        let field = prompt.field;
        let mut new_filter = self.filter.clone();
        let parsed: Result<(), &'static str> = match field {
            FilterField::App => {
                new_filter.app = if buf.is_empty() {
                    None
                } else {
                    Some(buf.clone())
                };
                Ok(())
            }
            FilterField::Interface => {
                new_filter.interface = if buf.is_empty() {
                    None
                } else {
                    Some(buf.clone())
                };
                Ok(())
            }
            FilterField::Pid => {
                if buf.is_empty() {
                    new_filter.pid = None;
                    Ok(())
                } else {
                    match buf.parse::<u32>() {
                        Ok(n) => {
                            new_filter.pid = Some(n);
                            Ok(())
                        }
                        Err(_) => Err("expected number"),
                    }
                }
            }
            FilterField::Port => {
                if buf.is_empty() {
                    new_filter.port = None;
                    Ok(())
                } else {
                    match buf.parse::<u16>() {
                        Ok(n) if n > 0 => {
                            new_filter.port = Some(n);
                            Ok(())
                        }
                        _ => Err("expected port (1-65535)"),
                    }
                }
            }
            FilterField::Proto => {
                if buf.is_empty() {
                    new_filter.proto = None;
                    Ok(())
                } else {
                    match buf.to_ascii_lowercase().as_str() {
                        "tcp" => {
                            new_filter.proto = Some(Protocol::Tcp);
                            Ok(())
                        }
                        "udp" => {
                            new_filter.proto = Some(Protocol::Udp);
                            Ok(())
                        }
                        "icmp" => {
                            new_filter.proto = Some(Protocol::Icmp);
                            Ok(())
                        }
                        _ => Err("expected tcp/udp/icmp"),
                    }
                }
            }
        };
        match parsed {
            Ok(()) => {
                self.apply_filter(new_filter);
                self.prompt = None;
                self.dirty = true;
                KeyOutcome::Dirty
            }
            Err(msg) => {
                if let Some(p) = self.prompt.as_mut() {
                    p.error = Some(msg.into());
                }
                self.dirty = true;
                KeyOutcome::Dirty
            }
        }
    }

    fn move_selection(&mut self, delta: isize) -> KeyOutcome {
        match self.view {
            View::Stream => {
                if self.rows.is_empty() {
                    return KeyOutcome::Continue;
                }
                let len = self.rows.len() as isize;
                let cur = self.stream_state.selected as isize;
                let next = (cur + delta).clamp(0, len - 1) as usize;
                if next == self.stream_state.selected {
                    return KeyOutcome::Continue;
                }
                self.stream_state.selected = next;
                self.dirty = true;
                KeyOutcome::Dirty
            }
            View::Hosts => self.move_hosts_selection(delta),
        }
    }

    fn move_hosts_selection(&mut self, delta: isize) -> KeyOutcome {
        let visible = self.visible_hosts_rows();
        if visible.is_empty() {
            return KeyOutcome::Continue;
        }
        let cur_key = (
            self.hosts_state.selected_process,
            self.hosts_state.selected_host,
        );
        let cur_idx = visible.iter().position(|k| k == &cur_key).unwrap_or(0) as isize;
        let next_idx = (cur_idx + delta).clamp(0, visible.len() as isize - 1) as usize;
        let (p, h) = visible[next_idx];
        if (p, h) == cur_key {
            return KeyOutcome::Continue;
        }
        self.hosts_state.selected_process = p;
        self.hosts_state.selected_host = h;
        self.dirty = true;
        KeyOutcome::Dirty
    }

    /// Flat list of (process_index, Option<host_index>) entries currently
    /// visible in the hosts tree. Children of collapsed processes are
    /// skipped, so up/down navigation jumps over them.
    fn visible_hosts_rows(&self) -> Vec<(usize, Option<usize>)> {
        let mut rows = Vec::new();
        for (proc_idx, ((pid, _), hosts)) in self.aggregator.per_proc.iter().enumerate() {
            rows.push((proc_idx, None));
            if !self.hosts_state.collapsed.contains(pid) {
                for host_idx in 0..hosts.len() {
                    rows.push((proc_idx, Some(host_idx)));
                }
            }
        }
        rows
    }

    /// Enter handler. In Hosts view, hitting Enter on a *process* row
    /// toggles its collapsed state. Everywhere else it is a no-op for
    /// now — input semantics on host rows / stream rows can grow in 6.5.
    fn activate_selection(&mut self) -> KeyOutcome {
        if self.view != View::Hosts {
            return KeyOutcome::Continue;
        }
        if self.hosts_state.selected_host.is_some() {
            return KeyOutcome::Continue;
        }
        let pid = match self
            .aggregator
            .per_proc
            .keys()
            .nth(self.hosts_state.selected_process)
        {
            Some((pid, _)) => *pid,
            None => return KeyOutcome::Continue,
        };
        self.hosts_state.toggle_collapse(pid);
        self.dirty = true;
        KeyOutcome::Dirty
    }
}

/// Result of dispatching one key event through [`App::handle_key`]. The
/// event loop consults this to decide whether to redraw or break out of
/// the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    /// State did not change; safe to skip the next redraw.
    Continue,
    /// State changed; a redraw should follow.
    Dirty,
    /// User asked to exit; the event loop should break.
    Quit,
}

// ---------------------------------------------------------------------------
// Draw functions. Pure: read `App`, mutate the `Frame`, return nothing.
// ---------------------------------------------------------------------------

fn draw_too_small(frame: &mut Frame, area: Rect) {
    let msg = format!("Resize terminal to at least {MIN_COLS}×{MIN_ROWS} to use qk capture.");
    let p = Paragraph::new(msg)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    // Centre vertically.
    let y = area.y + area.height / 2;
    let line_area = Rect {
        x: area.x,
        y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(p, line_area);
}

fn draw_hosts(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(LayoutDir::Vertical)
        .constraints([
            Constraint::Length(1), // top bar
            Constraint::Length(1), // filter row
            Constraint::Min(5),    // tree-table
            Constraint::Length(7), // detail pane
            Constraint::Length(1), // footer / prompt
        ])
        .split(area);
    draw_top_bar(frame, chunks[0], app);
    draw_filter_row(frame, chunks[1], app);
    draw_hosts_tree(frame, chunks[2], app);
    draw_hosts_detail(frame, chunks[3], app);
    draw_footer_or_prompt(frame, chunks[4], app);
}

/// Sort hosts within one process by descending traffic. Returned as a Vec
/// of references so both the tree and the detail pane see the same order
/// — the selected_host index in [`HostsViewState`] is into this order.
fn sorted_hosts(
    map: &std::collections::BTreeMap<(std::net::IpAddr, u16), super::HostStats>,
) -> Vec<((std::net::IpAddr, u16), &super::HostStats)> {
    let mut rows: Vec<_> = map.iter().map(|(k, v)| (*k, v)).collect();
    rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.bytes_out + s.bytes_in));
    rows
}

fn draw_hosts_tree(frame: &mut Frame, area: Rect, app: &App) {
    if app.aggregator.is_empty() {
        let p = Paragraph::new("no hosts yet — waiting for traffic")
            .alignment(Alignment::Center)
            .style(palette::HINT);
        let y = area.y + area.height / 2;
        let line_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        frame.render_widget(p, line_area);
        return;
    }
    let mut lines: Vec<Line> = Vec::new();
    let sel_proc = app.hosts_state.selected_process;
    let sel_host = app.hosts_state.selected_host;
    for (proc_idx, ((pid, comm), hosts)) in app.aggregator.per_proc.iter().enumerate() {
        let collapsed = app.hosts_state.collapsed.contains(pid);
        let chev = if collapsed { "▶" } else { "▼" };
        let owner = super::owner_label(*pid, comm);
        let proc_text = format!("{chev} {owner}");
        let proc_line = if proc_idx == sel_proc && sel_host.is_none() {
            Line::from(Span::styled(proc_text, palette::ROW_SELECTED))
        } else {
            Line::from(proc_text)
        };
        lines.push(proc_line);
        if collapsed {
            continue;
        }
        for (host_idx, ((ip, port), stats)) in sorted_hosts(hosts).into_iter().enumerate() {
            let endpoint = super::Endpoint {
                ip,
                port: Some(port),
            };
            let host_text = format!(
                "    {:<24}  {} pkts   {} out / {} in",
                endpoint.to_string(),
                stats.pkts,
                crate::ui::format_bytes(stats.bytes_out),
                crate::ui::format_bytes(stats.bytes_in),
            );
            let host_line = if proc_idx == sel_proc && sel_host == Some(host_idx) {
                Line::from(Span::styled(host_text, palette::ROW_SELECTED))
            } else {
                Line::from(host_text)
            };
            lines.push(host_line);
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_hosts_detail(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::with_capacity(6);
    lines.push(Line::from(Span::styled(
        "Selected host",
        palette::DETAIL_HEADING,
    )));
    let Some(((pid, comm), hosts)) = app
        .aggregator
        .per_proc
        .iter()
        .nth(app.hosts_state.selected_process)
    else {
        lines.push(Line::from(Span::styled(
            "(no host selected)",
            palette::HINT,
        )));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };
    let owner = super::owner_label(*pid, comm);
    lines.push(Line::from(format!("Process:   {owner}")));
    let Some(host_idx) = app.hosts_state.selected_host else {
        lines.push(Line::from(Span::styled(
            "(pick a row inside the process to see traffic)",
            palette::HINT,
        )));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };
    let sorted = sorted_hosts(hosts);
    let Some(((ip, port), stats)) = sorted.get(host_idx) else {
        lines.push(Line::from(Span::styled(
            "(host index out of range)",
            palette::HINT,
        )));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };
    let endpoint = super::Endpoint {
        ip: *ip,
        port: Some(*port),
    };
    lines.push(Line::from(format!("Endpoint:  {endpoint}")));
    lines.push(Line::from(format!(
        "Traffic:   {} pkts · {} out / {} in",
        stats.pkts,
        crate::ui::format_bytes(stats.bytes_out),
        crate::ui::format_bytes(stats.bytes_in),
    )));
    let first_ago = stats.first_seen.elapsed().as_secs();
    let last_ago = stats.last_seen.elapsed().as_secs();
    lines.push(Line::from(format!(
        "Seen:      first {first_ago}s ago · last {last_ago}s ago"
    )));
    // Recent tail: arrow + bytes per entry, most-recent first, max 8.
    let mut recent_spans: Vec<Span> = vec![Span::raw("Recent:    ")];
    for (i, (_, dir, bytes)) in stats.recent.iter().rev().take(8).enumerate() {
        if i > 0 {
            recent_spans.push(Span::raw(" · "));
        }
        let arrow = match dir {
            PktDir::Out => Span::styled("↑", palette::ARROW_OUT),
            PktDir::In => Span::styled("↓", palette::ARROW_IN),
        };
        recent_spans.push(arrow);
        recent_spans.push(Span::raw(format!("{bytes}")));
    }
    lines.push(Line::from(recent_spans));
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_stream(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(LayoutDir::Vertical)
        .constraints([
            Constraint::Length(1), // top bar
            Constraint::Length(1), // filter row
            Constraint::Min(5),    // table (header + rows)
            Constraint::Length(7), // detail pane (heading + 5 fields + blank)
            Constraint::Length(1), // footer / prompt
        ])
        .split(area);

    draw_top_bar(frame, chunks[0], app);
    draw_filter_row(frame, chunks[1], app);
    draw_table(frame, chunks[2], app);
    draw_detail(frame, chunks[3], app);
    draw_footer_or_prompt(frame, chunks[4], app);
}

fn draw_top_bar(frame: &mut Frame, area: Rect, app: &App) {
    let elapsed = app.stats.started_at.elapsed();
    let total = elapsed.as_secs();
    let mm = total / 60;
    let ss = total % 60;
    let drop_text = format!("{} dropped", app.stats.dropped);
    let drop_style = if app.stats.dropped > 0 {
        palette::DROP_WARN
    } else {
        Style::new()
    };
    let (stream_style, hosts_style) = match app.view {
        View::Stream => (palette::VIEW_TAB_ACTIVE, palette::VIEW_TAB_INACTIVE),
        View::Hosts => (palette::VIEW_TAB_INACTIVE, palette::VIEW_TAB_ACTIVE),
    };
    let mut spans = vec![
        Span::raw("qk capture · "),
        Span::raw(format!("{} pkts", app.stats.count)),
        Span::raw(" · "),
        Span::styled(drop_text, drop_style),
        Span::raw(" · "),
        Span::raw(format!("{mm}:{ss:02}")),
        Span::raw(" · ["),
        Span::styled("Stream", stream_style),
        Span::raw(" | "),
        Span::styled("Hosts", hosts_style),
        Span::raw("]"),
    ];
    if let Some(err) = &app.stats.save_error {
        // Loud, persistent indicator: silent truncation of `--save out.pcap`
        // because of a full disk / revoked permission was a Phase 6 review
        // finding (C2). Top-bar is the only place the user sees while raw
        // mode is on, so the error rides along with packet count.
        spans.push(Span::raw(" · "));
        spans.push(Span::styled(
            format!("save error: {err}"),
            palette::DROP_WARN,
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_filter_row(frame: &mut Frame, area: Rect, app: &App) {
    let chips = filter_chips(&app.filter);
    let mut spans: Vec<Span> = Vec::with_capacity(chips.len() * 2 + 1);
    spans.push(Span::raw("Filters: "));
    if chips.is_empty() {
        spans.push(Span::styled("(none)", palette::HINT));
    } else {
        for (i, chip) in chips.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" | "));
            }
            spans.push(Span::styled(chip.clone(), palette::ACTIVE_FILTER));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn filter_chips(filter: &Filter) -> Vec<String> {
    let mut chips = Vec::new();
    if let Some(a) = filter.app.as_deref() {
        chips.push(format!("app={a}"));
    }
    if let Some(p) = filter.pid {
        chips.push(format!("pid={p}"));
    }
    if let Some(p) = filter.port {
        chips.push(format!("port={p}"));
    }
    if let Some(p) = filter.proto {
        chips.push(format!("proto={}", proto_label(p)));
    }
    if let Some(i) = filter.interface.as_deref() {
        chips.push(format!("iface={i}"));
    }
    chips
}

fn proto_label(p: Protocol) -> &'static str {
    match p {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
        Protocol::Icmp => "icmp",
        Protocol::Other => "other",
    }
}

fn draw_table(frame: &mut Frame, area: Rect, app: &App) {
    if app.rows.is_empty() {
        let p = Paragraph::new("no packets yet — waiting for traffic")
            .alignment(Alignment::Center)
            .style(palette::HINT);
        let y = area.y + area.height / 2;
        let line_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        frame.render_widget(p, line_area);
        return;
    }
    // Widths sum to 93; with 7 single-space gaps that totals exactly 100.
    // Phase 6.2 only targets the 100-col snapshot test — wider terminals
    // just get the same widths with trailing whitespace.
    let widths = [
        Constraint::Length(12), // Time   "12:34:56.789"
        Constraint::Length(3),  // Dir    "↑" / "↓"
        Constraint::Length(20), // Process "mDNSResponder (198)" = 19 chars
        Constraint::Length(5),  // Proto  "OTHER"
        Constraint::Length(21), // Src
        Constraint::Length(3),  // " → "
        Constraint::Length(21), // Dst
        Constraint::Length(8),  // Bytes  "10000 B"
    ];
    let header = Row::new([
        "Time", "Dir", "Process", "Proto", "Src", "→", "Dst", "Bytes",
    ])
    .style(palette::TABLE_HEADER);
    // Visible body rows = area height − 1 for the header. Without this
    // skip the ratatui Table truncates at the viewport top so a selected
    // row past the viewport never gets drawn (C3 from the Phase 6 review).
    let visible_rows = area.height.saturating_sub(1) as usize;
    let selected = app.stream_state.selected;
    let first = if visible_rows == 0 || app.rows.len() <= visible_rows {
        0
    } else if selected >= visible_rows {
        selected + 1 - visible_rows
    } else {
        0
    };
    let body: Vec<Row> = app
        .rows
        .iter()
        .enumerate()
        .skip(first)
        .take(visible_rows.max(1))
        .map(|(i, row)| {
            let row_cell = row_cells(row);
            if i == selected {
                row_cell.style(palette::ROW_SELECTED)
            } else {
                row_cell
            }
        })
        .collect();
    let table = Table::new(body, widths).header(header).column_spacing(1);
    frame.render_widget(table, area);
}

fn row_cells(row: &DisplayRow) -> Row<'static> {
    let time = super::format_time(row.pkt.seconds, row.pkt.microseconds);
    let dir = PktDir::from_io_byte(row.pkt.io);
    let arrow_cell = match dir {
        PktDir::Out => Cell::from(Span::styled("↑", palette::ARROW_OUT)),
        PktDir::In => Cell::from(Span::styled("↓", palette::ARROW_IN)),
    };
    let owner = super::owner_label(row.pkt.pid, &row.pkt.comm);
    let bytes = row.pkt.data.len();
    let (proto, src, dst) = match row.parsed.as_ref() {
        Some(p) => (
            proto_label(p.protocol).to_uppercase(),
            p.src.to_string(),
            p.dst.to_string(),
        ),
        None => ("—".to_string(), "—".to_string(), "—".to_string()),
    };
    Row::new(vec![
        Cell::from(time),
        arrow_cell,
        Cell::from(owner),
        Cell::from(proto),
        Cell::from(src),
        Cell::from("→".to_string()),
        Cell::from(dst),
        Cell::from(format!("{bytes} B")),
    ])
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::with_capacity(6);
    lines.push(Line::from(Span::styled(
        "Selected packet",
        palette::DETAIL_HEADING,
    )));
    if let Some(row) = app.rows.get(app.stream_state.selected) {
        let dir = PktDir::from_io_byte(row.pkt.io);
        let owner = super::owner_label(row.pkt.pid, &row.pkt.comm);
        let dir_word = match dir {
            PktDir::Out => "outbound",
            PktDir::In => "inbound",
        };
        let endpoints = match row.parsed.as_ref() {
            Some(p) => format!("{} → {}", p.src, p.dst),
            None => "(unparsed)".to_string(),
        };
        let proto = match row.parsed.as_ref() {
            Some(p) => format!("{}, {} bytes", proto_label(p.protocol), row.pkt.data.len()),
            None => format!("unparsed, {} bytes", row.pkt.data.len()),
        };
        let comment = pcap_io::packet_comment(&row.pkt);
        lines.push(Line::from(format!("Process:   {owner}")));
        lines.push(Line::from(format!(
            "Direction: {dir_word} on {}",
            row.pkt.interface
        )));
        lines.push(Line::from(format!("Endpoints: {endpoints}")));
        lines.push(Line::from(format!("Protocol:  {proto}")));
        lines.push(Line::from(format!("Comment:   {comment}")));
    } else {
        lines.push(Line::from(Span::styled(
            "(no packet selected)",
            palette::HINT,
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_footer_or_prompt(frame: &mut Frame, area: Rect, app: &App) {
    match &app.prompt {
        Some(prompt) => draw_prompt(frame, area, prompt),
        None => draw_footer(frame, area),
    }
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    // Each `[X]` letter gets the hotkey colour; the rest is plain.
    let mut spans: Vec<Span> = Vec::new();
    for (letter, rest) in [
        ("a", "pp"),
        ("p", "roto"),
        ("P", "ort"),
        ("i", "face"),
        ("d", "pid"),
        ("c", "lear"),
        ("q", "uit"),
    ] {
        spans.push(Span::raw("["));
        spans.push(Span::styled(letter, palette::HOTKEY_LABEL));
        spans.push(Span::raw("]"));
        spans.push(Span::raw(rest));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_prompt(frame: &mut Frame, area: Rect, prompt: &PromptState) {
    let mut spans = vec![
        Span::styled(":", palette::PROMPT_PREFIX),
        Span::raw(prompt.field.wire_label()),
        Span::raw(" "),
        Span::raw(prompt.buffer.clone()),
        Span::raw("_"),
    ];
    if let Some(err) = &prompt.error {
        spans.push(Span::raw("    "));
        spans.push(Span::styled(err.clone(), palette::PROMPT_ERROR));
        spans.push(Span::raw(" · "));
    } else {
        spans.push(Span::raw("    "));
    }
    spans.push(Span::styled("Esc cancel", palette::HINT));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---------------------------------------------------------------------------
// Event loop. Wires the device's packet stream, crossterm key events, a 30Hz
// redraw tick, and ctrl-C together through `tokio::select!`. Terminal state
// is unwound by [`TerminalGuard::Drop`] even on panic.
// ---------------------------------------------------------------------------

/// Map a [`super::Mode`] to the initial [`View`] the TUI opens with.
/// Lives outside [`run`] so the wiring (CLI flag → Mode → initial view)
/// can be unit-tested without a terminal.
pub fn initial_view_from_mode(mode: super::Mode) -> View {
    match mode {
        super::Mode::Hosts => View::Hosts,
        _ => View::Stream,
    }
}

/// RAII handle for the alt screen + raw mode. Constructed by entering the
/// alt screen and enabling raw mode; `Drop` reverses both, even if the
/// event loop panicked or returned an error. Errors during teardown are
/// swallowed — we are best-effort by definition.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Run the interactive capture TUI until the user quits, the device
/// stream ends, or `--max` is reached. Requires a TTY on stdout; fails
/// fast with a clear error otherwise so pipe consumers don't see a
/// stuck-blank screen.
pub async fn run(device: &dyn crate::device::Device, opts: super::Options) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use crossterm::event::{Event, EventStream};
    use crossterm::{execute, terminal};
    use futures::StreamExt as _;
    use ratatui::backend::CrosstermBackend;
    use ratatui::Terminal;
    use std::io::stdout;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    // The TTY gate lives in `capture::run` — if we got here, stdout is
    // a terminal. Non-TTY callers (CI, pipes, `--save`) take the legacy
    // line-renderer path instead.

    let initial_view = initial_view_from_mode(opts.mode);

    let mut writer = match &opts.save {
        Some(path) => Some(
            super::CaptureFile::open(path)
                .with_context(|| format!("failed to open {} for writing", path.display()))?,
        ),
        None => None,
    };

    let stream = device.capture_packets().await?;
    let crate::device::PacketStream { mut rx, dropped } = stream;

    // Order matters: construct the guard *before* the fallible
    // EnterAlternateScreen call so a transient failure there still
    // unwinds raw mode on drop (C13 from the Phase 6 review).
    terminal::enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(stdout(), terminal::EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(opts.filter.clone(), initial_view);
    let mut events = EventStream::new();
    let mut redraw = tokio::time::interval(Duration::from_millis(33));
    redraw.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Initial paint so the screen isn't blank until the first event.
    app.dirty = true;
    terminal.draw(|f| app.draw(f))?;

    let result: anyhow::Result<()> = loop {
        // Sync the producer-side drop counter once per loop iteration so
        // the top bar's "N dropped" stays close to live. Only mark dirty
        // when the value actually moved — otherwise idle frames never
        // re-paint and the user sees a stale count (C16).
        let now_dropped = dropped.load(Ordering::Relaxed);
        if now_dropped != app.stats.dropped {
            app.stats.dropped = now_dropped;
            app.dirty = true;
        }

        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => break Ok(()),
            maybe_pkt = rx.recv() => {
                match maybe_pkt {
                    Some(Ok(p)) => {
                        // Apply the filter before writing to the pcap sink
                        // so `qk capture --app instagram --save out.pcap`
                        // doesn't pollute the file with every device
                        // packet (C7). `matches_packet` is the cheap
                        // pre-parse gate; `matches_parsed` (port/proto)
                        // runs inside `app.ingest` and we mirror it here.
                        let keep = opts.filter.matches_packet(&p)
                            && opts.filter.matches_parsed(super::parse_summary(&p).as_ref());
                        if keep {
                            if let Some(w) = writer.as_mut() {
                                if let Err(e) = w.write(&p) {
                                    // Surface the failure on the top bar
                                    // (C2). Raw mode would garble stderr,
                                    // and silently truncating the saved
                                    // pcap is what the review flagged.
                                    if app.stats.save_error.is_none() {
                                        app.stats.save_error = Some(format!("{e}"));
                                        app.dirty = true;
                                    }
                                }
                            }
                        }
                        app.ingest(p);
                        if let Some(limit) = opts.max {
                            if app.stats.count >= limit { break Ok(()); }
                        }
                    }
                    Some(Err(e)) => break Err(anyhow::anyhow!("capture ended: {e}")),
                    None => break Ok(()),
                }
            }
            maybe_evt = events.next() => {
                match maybe_evt {
                    Some(Ok(Event::Key(key))) => {
                        if matches!(app.handle_key(key), KeyOutcome::Quit) {
                            break Ok(());
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        app.dirty = true;
                    }
                    _ => {}
                }
            }
            _ = redraw.tick() => {
                if app.dirty {
                    terminal.draw(|f| app.draw(f))?;
                }
            }
        }
    };

    drop(writer);
    drop(_guard);

    // If the user was on the Hosts view, the live aggregator has the
    // only meaningful state — drop a final snapshot to stdout before the
    // exit lines so the data survives the alt-screen restore.
    if app.view == View::Hosts && !app.aggregator.is_empty() {
        let header = format!("Final hosts snapshot ({} packets)", app.stats.count);
        print!("{}", app.aggregator.render(&header));
    }

    // Restore-the-summary lines the legacy monolith emitted on exit (C8/C9
    // from the Phase 6 review). At this point the alt screen is gone, so
    // eprintln! lands in the user's normal scrollback.
    let final_drops = dropped.load(Ordering::Relaxed);
    let hit_max = matches!(opts.max, Some(limit) if app.stats.count >= limit);
    let count = app.stats.count;
    if hit_max {
        eprintln!(
            "Reached --max {}, stopping ({final_drops} dropped).",
            opts.max.unwrap()
        );
    } else {
        eprintln!("Stopped after {count} packets ({final_drops} dropped).");
    }
    if let Some(path) = &opts.save {
        match &app.stats.save_error {
            Some(err) => eprintln!(
                "warning: --save target {} ended with error: {err}",
                path.display()
            ),
            None => eprintln!("Saved to {}", path.display()),
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::capture::Protocol;
    use etherparse::PacketBuilder;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    fn packet(io: u8, pid: u32, comm: &str, iface: &str, payload: Vec<u8>) -> Packet {
        let mut data = vec![0u8; 14];
        data.extend_from_slice(&payload);
        Packet {
            pid,
            comm: comm.into(),
            epid: 0,
            ecomm: String::new(),
            interface: iface.into(),
            seconds: 12 * 3600 + 34 * 60 + 56,
            microseconds: 789_000,
            io,
            data,
        }
    }

    fn tcp_v4(src_port: u16, dst_port: u16) -> Vec<u8> {
        let b = PacketBuilder::ipv4([192, 168, 1, 42], [31, 13, 65, 36], 64)
            .tcp(src_port, dst_port, 0, 1000);
        let mut buf = Vec::with_capacity(b.size(0));
        b.write(&mut buf, &[]).unwrap();
        buf
    }

    fn udp_v4(src_port: u16, dst_port: u16) -> Vec<u8> {
        let b = PacketBuilder::ipv4([192, 168, 1, 42], [1, 1, 1, 1], 64).udp(src_port, dst_port);
        let mut buf = Vec::with_capacity(b.size(0));
        b.write(&mut buf, &[]).unwrap();
        buf
    }

    /// Render the app to a [`TestBackend`] of the given size and return
    /// the resulting buffer as a string, with trailing spaces stripped
    /// for readable inline snapshots.
    fn render(app: &mut App, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        buffer_to_string(terminal.backend().buffer())
    }

    fn buffer_to_string(buf: &Buffer) -> String {
        let w = buf.area.width as usize;
        let mut out = String::new();
        for (i, cell) in buf.content().iter().enumerate() {
            if i > 0 && i % w == 0 {
                out.push('\n');
            }
            out.push_str(cell.symbol());
        }
        out.lines()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ----- ingest / apply_filter (Phase 6.1) -------------------------------

    #[test]
    fn ingest_accepts_packet_matching_empty_filter() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.ingest(packet(1, 1, "Safari", "en0", tcp_v4(54321, 443)));
        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.stats.count, 1);
        assert!(app.dirty);
    }

    #[test]
    fn ingest_drops_packet_failing_pre_parse_filter() {
        let mut app = App::new(
            Filter {
                app: Some("Instagram".into()),
                ..Default::default()
            },
            View::Stream,
        );
        app.ingest(packet(1, 1, "Safari", "en0", tcp_v4(54321, 443)));
        assert!(app.rows.is_empty());
        assert_eq!(app.stats.count, 0);
    }

    #[test]
    fn ingest_drops_packet_failing_post_parse_filter() {
        let mut app = App::new(
            Filter {
                proto: Some(Protocol::Udp),
                ..Default::default()
            },
            View::Stream,
        );
        app.ingest(packet(1, 1, "Safari", "en0", tcp_v4(54321, 443)));
        assert!(app.rows.is_empty());
        assert_eq!(app.stats.count, 0);
    }

    #[test]
    fn ingest_caches_parsed_summary_on_display_row() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.ingest(packet(1, 1, "Safari", "en0", tcp_v4(54321, 443)));
        let row = &app.rows[0];
        let parsed = row.parsed.as_ref().expect("parse should succeed");
        assert_eq!(parsed.protocol, Protocol::Tcp);
        assert_eq!(parsed.dst.port, Some(443));
    }

    #[test]
    fn ingest_keeps_row_when_parse_fails_and_filter_permits() {
        let mut app = App::new(Filter::default(), View::Stream);
        let p = packet(1, 1, "x", "en0", vec![0xff, 0xff]);
        app.ingest(p);
        assert_eq!(app.rows.len(), 1);
        assert!(app.rows[0].parsed.is_none());
        assert_eq!(app.stats.count, 1);
    }

    #[test]
    fn ring_buffer_overflow_evicts_oldest_row() {
        let mut app = App::with_capacity(Filter::default(), View::Stream, 3);
        for port in 1..=5 {
            app.ingest(packet(1, 1, "x", "en0", tcp_v4(port, 443)));
        }
        assert_eq!(app.rows.len(), 3);
        let first_src = app.rows.front().unwrap().parsed.as_ref().unwrap().src.port;
        let last_src = app.rows.back().unwrap().parsed.as_ref().unwrap().src.port;
        assert_eq!(first_src, Some(3));
        assert_eq!(last_src, Some(5));
        assert_eq!(app.stats.count, 5);
    }

    #[test]
    fn apply_filter_drops_rows_that_no_longer_match() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.ingest(packet(1, 1, "Instagram", "en0", tcp_v4(54321, 443)));
        app.ingest(packet(1, 1, "Safari", "en0", tcp_v4(54321, 443)));
        app.ingest(packet(1, 1, "InstagramShare", "en0", tcp_v4(54321, 443)));
        assert_eq!(app.rows.len(), 3);

        app.apply_filter(Filter {
            app: Some("instagram".into()),
            ..Default::default()
        });
        assert_eq!(app.rows.len(), 2);
        assert!(app.rows.iter().all(|r| r.pkt.comm.contains("Instagram")));
    }

    #[test]
    fn apply_filter_rebuilds_aggregator_from_surviving_rows() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.ingest(packet(1, 10, "Safari", "en0", tcp_v4(54321, 443)));
        app.ingest(packet(1, 20, "Instagram", "en0", tcp_v4(54321, 443)));
        assert_eq!(app.aggregator.per_proc.len(), 2);

        app.apply_filter(Filter {
            pid: Some(20),
            ..Default::default()
        });
        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.aggregator.per_proc.len(), 1);
        let only = app.aggregator.per_proc.keys().next().unwrap();
        assert_eq!(only.0, 20);
    }

    #[test]
    fn apply_filter_to_empty_buffer_is_a_noop() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.apply_filter(Filter {
            proto: Some(Protocol::Tcp),
            ..Default::default()
        });
        assert!(app.rows.is_empty());
        assert!(app.aggregator.is_empty());
    }

    #[test]
    fn stats_count_only_reflects_accepted_packets() {
        let mut app = App::new(
            Filter {
                proto: Some(Protocol::Tcp),
                ..Default::default()
            },
            View::Stream,
        );
        for _ in 0..3 {
            app.ingest(packet(1, 1, "Safari", "en0", tcp_v4(54321, 443)));
        }
        for _ in 0..2 {
            app.ingest(packet(1, 1, "Safari", "en0", udp_v4(5353, 53)));
        }
        assert_eq!(app.stats.count, 3);
        assert_eq!(app.rows.len(), 3);
    }

    #[test]
    fn apply_filter_clamps_stream_selection_past_end() {
        let mut app = App::new(Filter::default(), View::Stream);
        for src in 1..=4 {
            app.ingest(packet(1, 1, "Safari", "en0", tcp_v4(src, 443)));
        }
        app.stream_state.selected = 3;
        app.apply_filter(Filter {
            port: Some(80),
            ..Default::default()
        });
        assert!(app.rows.is_empty());
        assert_eq!(app.stream_state.selected, 0);
    }

    // ----- draw (Phase 6.2) ------------------------------------------------

    #[test]
    fn draw_clears_dirty_flag() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.ingest(packet(1, 1, "Safari", "en0", tcp_v4(54321, 443)));
        assert!(app.dirty);
        // Drive draw through a TestBackend so the side-effect runs.
        let _ = render(&mut app, 100, 30);
        assert!(!app.dirty, "draw must clear the dirty flag");
    }

    #[test]
    fn stream_renders_three_rows_snapshot() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.ingest(packet(1, 4521, "Instagram", "en0", tcp_v4(54321, 443)));
        app.ingest(packet(0, 4521, "Instagram", "en0", tcp_v4(54321, 443)));
        app.ingest(packet(1, 198, "mDNSResponder", "en0", udp_v4(5353, 53)));
        let out = render(&mut app, 100, 30);
        insta::assert_snapshot!("stream_three_rows", out);
    }

    #[test]
    fn stream_renders_empty_state_snapshot() {
        let mut app = App::new(Filter::default(), View::Stream);
        let out = render(&mut app, 100, 30);
        insta::assert_snapshot!("stream_empty_state", out);
    }

    #[test]
    fn stream_renders_prompt_with_partial_buffer_snapshot() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.ingest(packet(1, 4521, "Instagram", "en0", tcp_v4(54321, 443)));
        app.prompt = Some(PromptState {
            field: FilterField::App,
            buffer: "insta".into(),
            error: None,
        });
        let out = render(&mut app, 100, 30);
        insta::assert_snapshot!("stream_prompt_partial", out);
    }

    #[test]
    fn stream_renders_prompt_with_inline_error_snapshot() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.prompt = Some(PromptState {
            field: FilterField::Pid,
            buffer: "abc".into(),
            error: Some("expected number".into()),
        });
        let out = render(&mut app, 100, 30);
        // Only assert on the prompt line — full layout is covered by other snapshots.
        let last_line = out.lines().last().unwrap();
        assert_eq!(last_line, ":pid abc_    expected number · Esc cancel");
    }

    #[test]
    fn stream_top_bar_shows_active_filter_chips_snapshot() {
        let mut app = App::new(
            Filter {
                app: Some("instagram".into()),
                proto: Some(Protocol::Tcp),
                ..Default::default()
            },
            View::Stream,
        );
        app.ingest(packet(1, 4521, "Instagram", "en0", tcp_v4(54321, 443)));
        let out = render(&mut app, 100, 30);
        let filter_line = out.lines().nth(1).unwrap();
        assert_eq!(filter_line, "Filters: app=instagram | proto=tcp");
    }

    #[test]
    fn terminal_too_small_shows_resize_hint_snapshot() {
        let mut app = App::new(Filter::default(), View::Stream);
        let out = render(&mut app, 40, 20);
        insta::assert_snapshot!("terminal_too_small", out);
    }

    // ----- hosts view (Phase 6.3) -----------------------------------------

    /// Build an outbound TCP packet to a specific remote IP, so the hosts
    /// aggregator buckets distinct hosts under one process.
    fn tcp_to(pid: u32, comm: &str, dst_ip: [u8; 4], dst_port: u16) -> Packet {
        let b = PacketBuilder::ipv4([192, 168, 1, 42], dst_ip, 64).tcp(54321, dst_port, 0, 1000);
        let mut payload = Vec::with_capacity(b.size(0));
        b.write(&mut payload, &[]).unwrap();
        let mut data = vec![0u8; 14];
        data.extend_from_slice(&payload);
        Packet {
            pid,
            comm: comm.into(),
            epid: 0,
            ecomm: String::new(),
            interface: "en0".into(),
            seconds: 12 * 3600 + 34 * 60 + 56,
            microseconds: 789_000,
            io: 1,
            data,
        }
    }

    fn seeded_hosts_app() -> App {
        let mut app = App::new(Filter::default(), View::Hosts);
        // Two processes × three remote hosts each. BTreeMap orders by
        // (pid, comm), so Safari (pid 4520) comes before Instagram (4521).
        app.ingest(tcp_to(4521, "Instagram", [31, 13, 65, 36], 443));
        app.ingest(tcp_to(4521, "Instagram", [157, 240, 241, 5], 443));
        app.ingest(tcp_to(4521, "Instagram", [8, 8, 8, 8], 443));
        app.ingest(tcp_to(4520, "Safari", [17, 142, 180, 36], 443));
        app.ingest(tcp_to(4520, "Safari", [1, 1, 1, 1], 443));
        app.ingest(tcp_to(4520, "Safari", [142, 250, 80, 46], 443));
        app
    }

    #[test]
    fn hosts_view_state_toggle_collapse_flips_membership() {
        let mut state = HostsViewState::default();
        assert!(!state.collapsed.contains(&4521));
        state.toggle_collapse(4521);
        assert!(state.collapsed.contains(&4521));
        state.toggle_collapse(4521);
        assert!(!state.collapsed.contains(&4521));
        // Second pid is independent.
        state.toggle_collapse(4521);
        state.toggle_collapse(4520);
        assert_eq!(state.collapsed.len(), 2);
    }

    #[test]
    fn hosts_renders_two_processes_three_hosts_snapshot() {
        let mut app = seeded_hosts_app();
        let out = render(&mut app, 100, 30);
        insta::assert_snapshot!("hosts_two_procs_three_hosts", out);
    }

    #[test]
    fn hosts_renders_with_one_process_collapsed_snapshot() {
        let mut app = seeded_hosts_app();
        // Collapse Safari (pid 4520). The chevron flips to ▶ and its host
        // rows disappear; Instagram stays expanded.
        app.hosts_state.collapsed.insert(4520);
        let out = render(&mut app, 100, 30);
        insta::assert_snapshot!("hosts_one_collapsed", out);
    }

    #[test]
    fn hosts_detail_pane_shows_selected_host_snapshot() {
        let mut app = seeded_hosts_app();
        // Select the first host inside Safari (index 0 = 1.1.1.1:443 since
        // ties in traffic break by IP order via BTreeMap iteration).
        app.hosts_state.selected_process = 0;
        app.hosts_state.selected_host = Some(0);
        let out = render(&mut app, 100, 30);
        insta::assert_snapshot!("hosts_detail_selected", out);
    }

    #[test]
    fn hosts_empty_state_when_no_traffic_yet() {
        let mut app = App::new(Filter::default(), View::Hosts);
        let out = render(&mut app, 100, 30);
        // Centre line shows the empty-state hint; full layout coverage
        // lives in the populated snapshots.
        assert!(
            out.contains("no hosts yet"),
            "empty hosts view should render its hint, got:\n{out}"
        );
    }

    // ----- handle_key state machine (Phase 6.4) ----------------------------

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn k_ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn k_char(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty())
    }

    #[test]
    fn handle_key_quits_on_q_from_idle() {
        let mut app = App::new(Filter::default(), View::Stream);
        assert_eq!(app.handle_key(k_char('q')), KeyOutcome::Quit);
    }

    #[test]
    fn handle_key_quits_on_esc_from_idle() {
        let mut app = App::new(Filter::default(), View::Stream);
        assert_eq!(app.handle_key(k(KeyCode::Esc)), KeyOutcome::Quit);
    }

    #[test]
    fn handle_key_ctrl_c_quits_from_idle_and_from_prompt() {
        let mut app = App::new(Filter::default(), View::Stream);
        assert_eq!(app.handle_key(k_ctrl(KeyCode::Char('c'))), KeyOutcome::Quit);
        app.prompt = Some(PromptState {
            field: FilterField::App,
            buffer: "foo".into(),
            error: None,
        });
        assert_eq!(app.handle_key(k_ctrl(KeyCode::Char('c'))), KeyOutcome::Quit);
    }

    #[test]
    fn handle_key_opens_prompts_for_each_hotkey() {
        for (ch, expected) in [
            ('a', FilterField::App),
            ('p', FilterField::Proto),
            ('P', FilterField::Port),
            ('i', FilterField::Interface),
            ('d', FilterField::Pid),
        ] {
            let mut app = App::new(Filter::default(), View::Stream);
            assert_eq!(app.handle_key(k_char(ch)), KeyOutcome::Dirty);
            let prompt = app.prompt.as_ref().expect("hotkey should open prompt");
            assert_eq!(prompt.field, expected, "char {ch} should open {expected:?}");
            assert!(prompt.buffer.is_empty());
            assert!(prompt.error.is_none());
        }
    }

    #[test]
    fn handle_key_c_clears_all_filters() {
        let mut app = App::new(
            Filter {
                app: Some("instagram".into()),
                proto: Some(Protocol::Tcp),
                pid: Some(4521),
                ..Default::default()
            },
            View::Stream,
        );
        assert_eq!(app.handle_key(k_char('c')), KeyOutcome::Dirty);
        assert!(app.filter.app.is_none());
        assert!(app.filter.proto.is_none());
        assert!(app.filter.pid.is_none());
    }

    #[test]
    fn handle_key_tab_toggles_view() {
        let mut app = App::new(Filter::default(), View::Stream);
        assert_eq!(app.handle_key(k(KeyCode::Tab)), KeyOutcome::Dirty);
        assert_eq!(app.view, View::Hosts);
        assert_eq!(app.handle_key(k(KeyCode::Tab)), KeyOutcome::Dirty);
        assert_eq!(app.view, View::Stream);
    }

    #[test]
    fn handle_key_arrow_keys_move_stream_selection() {
        let mut app = App::new(Filter::default(), View::Stream);
        for src in 1..=3 {
            app.ingest(packet(1, 1, "Safari", "en0", tcp_v4(src, 443)));
        }
        // Selection starts at 0; Down moves to 1.
        assert_eq!(app.handle_key(k(KeyCode::Down)), KeyOutcome::Dirty);
        assert_eq!(app.stream_state.selected, 1);
        // Down again → 2.
        app.handle_key(k(KeyCode::Down));
        assert_eq!(app.stream_state.selected, 2);
        // Down at last row → clamped, no-op.
        assert_eq!(app.handle_key(k(KeyCode::Down)), KeyOutcome::Continue);
        assert_eq!(app.stream_state.selected, 2);
        // Up moves to 1.
        assert_eq!(app.handle_key(k(KeyCode::Up)), KeyOutcome::Dirty);
        assert_eq!(app.stream_state.selected, 1);
    }

    #[test]
    fn handle_key_arrow_keys_noop_on_empty_stream() {
        let mut app = App::new(Filter::default(), View::Stream);
        assert_eq!(app.handle_key(k(KeyCode::Down)), KeyOutcome::Continue);
        assert_eq!(app.handle_key(k(KeyCode::Up)), KeyOutcome::Continue);
    }

    #[test]
    fn handle_key_enter_on_hosts_process_toggles_collapse() {
        let mut app = seeded_hosts_app();
        // selected_process=0 (Safari), selected_host=None → process row.
        assert!(!app.hosts_state.collapsed.contains(&4520));
        assert_eq!(app.handle_key(k(KeyCode::Enter)), KeyOutcome::Dirty);
        assert!(app.hosts_state.collapsed.contains(&4520));
        // Enter again toggles back.
        app.handle_key(k(KeyCode::Enter));
        assert!(!app.hosts_state.collapsed.contains(&4520));
    }

    #[test]
    fn handle_key_enter_on_hosts_host_row_is_noop() {
        let mut app = seeded_hosts_app();
        app.hosts_state.selected_host = Some(0);
        assert_eq!(app.handle_key(k(KeyCode::Enter)), KeyOutcome::Continue);
    }

    #[test]
    fn handle_key_arrows_skip_collapsed_children_in_hosts() {
        let mut app = seeded_hosts_app();
        // Collapse Safari so only its process row is visible; arrows
        // should jump from Safari header straight to Instagram header.
        app.hosts_state.collapsed.insert(4520);
        // Starting position: (0, None) = Safari header.
        assert_eq!(app.handle_key(k(KeyCode::Down)), KeyOutcome::Dirty);
        assert_eq!(app.hosts_state.selected_process, 1);
        assert_eq!(app.hosts_state.selected_host, None);
    }

    #[test]
    fn handle_key_prompt_esc_closes_without_applying() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.handle_key(k_char('a'));
        app.handle_key(k_char('x'));
        assert!(app.prompt.is_some());
        assert_eq!(app.handle_key(k(KeyCode::Esc)), KeyOutcome::Dirty);
        assert!(app.prompt.is_none());
        // Filter unchanged — Esc must not apply.
        assert!(app.filter.app.is_none());
    }

    #[test]
    fn handle_key_prompt_chars_and_backspace_edit_buffer() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.handle_key(k_char('a'));
        for c in "insta".chars() {
            assert_eq!(app.handle_key(k_char(c)), KeyOutcome::Dirty);
        }
        assert_eq!(app.prompt.as_ref().unwrap().buffer, "insta");
        app.handle_key(k(KeyCode::Backspace));
        assert_eq!(app.prompt.as_ref().unwrap().buffer, "inst");
    }

    #[test]
    fn handle_key_prompt_enter_applies_app_filter() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.handle_key(k_char('a'));
        for c in "instagram".chars() {
            app.handle_key(k_char(c));
        }
        assert_eq!(app.handle_key(k(KeyCode::Enter)), KeyOutcome::Dirty);
        assert!(app.prompt.is_none());
        assert_eq!(app.filter.app.as_deref(), Some("instagram"));
    }

    #[test]
    fn handle_key_prompt_enter_validates_pid_and_keeps_error() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.handle_key(k_char('d'));
        for c in "abc".chars() {
            app.handle_key(k_char(c));
        }
        // Enter on invalid pid → prompt stays open with an inline error.
        assert_eq!(app.handle_key(k(KeyCode::Enter)), KeyOutcome::Dirty);
        let prompt = app
            .prompt
            .as_ref()
            .expect("prompt should stay open on error");
        assert_eq!(prompt.error.as_deref(), Some("expected number"));
        assert!(app.filter.pid.is_none());
        // Fix the buffer and re-submit.
        app.handle_key(k(KeyCode::Backspace));
        app.handle_key(k(KeyCode::Backspace));
        app.handle_key(k(KeyCode::Backspace));
        for c in "4521".chars() {
            app.handle_key(k_char(c));
        }
        assert_eq!(app.handle_key(k(KeyCode::Enter)), KeyOutcome::Dirty);
        assert!(app.prompt.is_none());
        assert_eq!(app.filter.pid, Some(4521));
    }

    #[test]
    fn handle_key_prompt_enter_validates_proto() {
        let mut app = App::new(Filter::default(), View::Stream);
        app.handle_key(k_char('p'));
        for c in "wat".chars() {
            app.handle_key(k_char(c));
        }
        app.handle_key(k(KeyCode::Enter));
        assert_eq!(
            app.prompt.as_ref().unwrap().error.as_deref(),
            Some("expected tcp/udp/icmp")
        );
        // Valid value (case-insensitive) clears the error and applies.
        app.handle_key(k(KeyCode::Backspace));
        app.handle_key(k(KeyCode::Backspace));
        app.handle_key(k(KeyCode::Backspace));
        for c in "TCP".chars() {
            app.handle_key(k_char(c));
        }
        app.handle_key(k(KeyCode::Enter));
        assert!(app.prompt.is_none());
        assert_eq!(app.filter.proto, Some(Protocol::Tcp));
    }

    #[test]
    fn handle_key_prompt_empty_buffer_clears_field() {
        let mut app = App::new(
            Filter {
                app: Some("instagram".into()),
                ..Default::default()
            },
            View::Stream,
        );
        // Open app prompt and immediately Enter with empty buffer → clears.
        app.handle_key(k_char('a'));
        app.handle_key(k(KeyCode::Enter));
        assert!(app.prompt.is_none());
        assert!(app.filter.app.is_none());
    }

    #[test]
    fn handle_key_unmapped_keys_in_idle_are_noop() {
        let mut app = App::new(Filter::default(), View::Stream);
        // F1 is not bound; expect Continue and no state change.
        assert_eq!(app.handle_key(k(KeyCode::F(1))), KeyOutcome::Continue);
        assert!(app.prompt.is_none());
        assert_eq!(app.view, View::Stream);
    }

    // ----- CLI flag → initial view wiring (Phase 6.5) ----------------------

    #[test]
    fn initial_view_from_mode_maps_hosts_to_hosts_view() {
        use crate::commands::capture::Mode;
        assert_eq!(initial_view_from_mode(Mode::Hosts), View::Hosts);
        // Every other mode opens on the Stream view. Headless / Dns / Sni
        // never reach this helper at runtime (capture::run dispatches them
        // elsewhere), but the mapping must still be sane in case the
        // dispatch shape changes later.
        assert_eq!(initial_view_from_mode(Mode::Stream), View::Stream);
        assert_eq!(initial_view_from_mode(Mode::Headless), View::Stream);
        assert_eq!(initial_view_from_mode(Mode::Dns), View::Stream);
        assert_eq!(initial_view_from_mode(Mode::Sni), View::Stream);
    }
}
