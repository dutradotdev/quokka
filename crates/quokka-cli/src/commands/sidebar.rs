//! Multi-device launcher: a ratatui sidebar shown by the bare `quokka` / `qk`
//! invocation when 2+ devices are connected across platforms.
//!
//! Left pane lists every connected device (platform tag, name, model, and a
//! battery/storage glance); right pane shows the selected device's dashboard
//! over a capability-aware action menu. Devices are connected lazily and cached
//! — the selected device loads first for a fast first paint, then the rest fill
//! in so the sidebar stats populate without blocking startup.
//!
//! The pure `draw` path renders an immutable [`SidebarState`], so the layout is
//! exercised with `ratatui::backend::TestBackend` in tests; the event loop and
//! its terminal handling are the only non-pure parts.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::{execute, terminal};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use crate::commands::device_action::{self, DeviceAction};
use crate::commands::{analyze, media};
use crate::device::{
    self, Device, DeviceListing, DeviceStatus, MediaFile, Platform, WalkCallback, WalkProgress,
};
use crate::ui::{format_bar, format_bytes, format_percent, wait_for_enter, DASH};

/// Fixed width of the device sidebar column.
const SIDEBAR_WIDTH: u16 = 30;
/// Minimum right-pane width for the side-by-side layout to be usable.
const RIGHT_MIN_WIDTH: u16 = 44;
/// Below these the screen can't hold the two-pane layout; show a hint instead.
const MIN_WIDTH: u16 = SIDEBAR_WIDTH + RIGHT_MIN_WIDTH;
const MIN_HEIGHT: u16 = 14;
/// Width of the storage usage bar in the right-pane summary.
const SUMMARY_BAR_WIDTH: usize = 10;
/// Idle redraw cadence so the "connecting…" placeholders refresh while a
/// device loads, even when no key is pressed.
const REDRAW_TICK: Duration = Duration::from_millis(250);

/// Which pane has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Devices,
    Actions,
}

/// Per-device load state. The connected handle lives separately in the run
/// loop's `devices` vector (it is neither `Clone` nor `Debug`); this is only
/// what the draw path needs to render.
enum DeviceCard {
    Unloaded,
    Loading,
    Ready {
        status: Box<DeviceStatus>,
        capture_supported: bool,
    },
    Failed(String),
}

/// What a key press resolved to. `Run`/`Rescan` are handled by the run loop
/// because they need a connected device or async re-enumeration; the rest are
/// pure navigation already applied to the state.
enum KeyOutcome {
    None,
    Quit,
    Run(DeviceAction),
    Rescan,
}

/// In-progress inline data load for a media-walk action (Analyze / Media). The
/// walk runs with the sidebar still up, its progress shown on the action's row,
/// so the user never drops into a bare-shell spinner before the result screen.
struct WalkLoad {
    action: DeviceAction,
    progress: Option<WalkProgress>,
}

/// Immutable-to-draw launcher model. Navigation mutates it; rendering only
/// reads it.
struct SidebarState {
    listings: Vec<DeviceListing>,
    cards: Vec<DeviceCard>,
    selected: usize,
    focus: Focus,
    action_cursor: usize,
    /// `Some` while a media-walk action is loading inline (drawn on its row).
    inline_load: Option<WalkLoad>,
}

impl SidebarState {
    fn new(listings: Vec<DeviceListing>) -> Self {
        let cards = listings.iter().map(|_| DeviceCard::Unloaded).collect();
        Self {
            listings,
            cards,
            selected: 0,
            focus: Focus::Devices,
            action_cursor: 0,
            inline_load: None,
        }
    }

    /// Actions for the currently selected device, or empty while it is not yet
    /// loaded (nothing to run against).
    fn current_actions(&self) -> Vec<DeviceAction> {
        match self.cards.get(self.selected) {
            Some(DeviceCard::Ready {
                capture_supported, ..
            }) => device_action::actions_for(*capture_supported),
            _ => Vec::new(),
        }
    }

    /// The next device that still needs connecting — the selected one first
    /// (fast first paint), then any other in order, so the whole sidebar
    /// eventually populates.
    fn next_to_load(&self) -> Option<usize> {
        if matches!(self.cards.get(self.selected), Some(DeviceCard::Unloaded)) {
            return Some(self.selected);
        }
        self.cards
            .iter()
            .position(|c| matches!(c, DeviceCard::Unloaded))
    }

    fn move_selection(&mut self, delta: isize) {
        self.selected = step(self.selected, delta, self.listings.len());
        // A new device may expose a different action set; keep the cursor valid.
        self.action_cursor = 0;
    }

    fn move_action(&mut self, delta: isize) {
        let len = self.current_actions().len();
        self.action_cursor = step(self.action_cursor, delta, len);
    }
}

/// Move `index` by `delta` within `0..len`, clamping at the ends (no wrap).
/// Returns 0 for an empty range.
fn step(index: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = len - 1;
    let next = index as isize + delta;
    next.clamp(0, max as isize) as usize
}

/// Entry point: run the sidebar over an already-enumerated, non-empty
/// `listings`. It is the default launcher view for any connected device — with
/// a single device the left pane just lists that one. Returns when the user
/// quits.
pub async fn run(listings: Vec<DeviceListing>) -> Result<()> {
    let mut state = SidebarState::new(listings);
    let mut devices: Vec<Option<Box<dyn Device>>> = state.listings.iter().map(|_| None).collect();
    let mut term = TerminalGuard::enter()?;
    let mut events = EventStream::new();
    let mut loading: Option<(usize, LoadFuture)> = None;

    loop {
        if loading.is_none() {
            if let Some(idx) = state.next_to_load() {
                state.cards[idx] = DeviceCard::Loading;
                loading = Some((idx, Box::pin(load_device(state.listings[idx].clone()))));
            }
        }

        term.0.draw(|f| draw(f, &state))?;

        let mut pending_action: Option<DeviceAction> = None;
        let mut pending_rescan = false;

        tokio::select! {
            biased;
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => match handle_key(key, &mut state) {
                        KeyOutcome::Quit => break,
                        KeyOutcome::Run(action) => pending_action = Some(action),
                        KeyOutcome::Rescan => pending_rescan = true,
                        KeyOutcome::None => {}
                    },
                    Some(Ok(_)) => {} // resize / mouse → redraw on next loop
                    Some(Err(_)) | None => break,
                }
            }
            Some((idx, result)) = next_load(loading.as_mut()) => {
                loading = None;
                apply_load(&mut state, &mut devices, idx, result);
            }
            _ = tokio::time::sleep(REDRAW_TICK) => {}
        }

        if pending_rescan {
            match rescan(&mut state, &mut devices).await {
                RescanOutcome::Continue => loading = None,
                RescanOutcome::Exit => break,
            }
        }

        if let Some(action) = pending_action {
            if devices[state.selected].is_none() {
                continue; // not connected yet — nothing to run against
            }

            // Phase 1: media-walk actions load inline with the sidebar still up
            // (progress on the row). Esc/q cancels and abandons the action.
            let prewalked = if is_media_walk(action) {
                let device = devices[state.selected].as_deref().expect("present above");
                match walk_inline(&mut term, &mut state, &mut events, device, action).await? {
                    Some(files) => Some(files),
                    None => continue,
                }
            } else {
                None
            };

            // Phase 2: tear the sidebar fully down — drop both the terminal and
            // the event stream — so the sub-command runs on a pristine terminal,
            // exactly as the single-device menu does. Nesting two ratatui
            // terminals / event streams was what blacked out the screen.
            drop(term);
            drop(events);
            let result = {
                let device = devices[state.selected].as_deref().expect("present above");
                dispatch(device, action, prewalked).await
            };
            // Keep printed output readable; TUI sub-commands need no pause.
            if result.is_ok() && prints_output(action) {
                wait_for_enter()?;
            }
            term = TerminalGuard::enter()?;
            events = EventStream::new();

            // A device-mutating action (uninstall, delete, power) may have
            // changed the status; re-read it on the next loop.
            if !matches!(action, DeviceAction::Info | DeviceAction::Card) {
                state.cards[state.selected] = DeviceCard::Unloaded;
            }
            result?;
        }
    }
    Ok(())
}

type LoadResult = Result<(Box<dyn Device>, DeviceStatus)>;
type LoadFuture = Pin<Box<dyn Future<Output = LoadResult>>>;

/// Connect to one device and read its status. Owns its inputs so it never
/// borrows the launcher state.
async fn load_device(listing: DeviceListing) -> LoadResult {
    // A sidebar row always names a specific device, so `select` is never
    // invoked here — but `connect` requires one.
    let device = device::connect(
        Some(&listing.udid),
        Some(listing.platform),
        &crate::ui::select_device,
    )
    .await?;
    let status = device.status().await?;
    Ok((device, status))
}

/// Await the in-flight load (if any), tagging the result with its device index.
/// Resolves to a never-ready future when nothing is loading so `select!` parks
/// on it harmlessly.
async fn next_load(loading: Option<&mut (usize, LoadFuture)>) -> Option<(usize, LoadResult)> {
    match loading {
        Some((idx, fut)) => Some((*idx, fut.await)),
        None => std::future::pending().await,
    }
}

fn apply_load(
    state: &mut SidebarState,
    devices: &mut [Option<Box<dyn Device>>],
    idx: usize,
    result: LoadResult,
) {
    match result {
        Ok((device, status)) => {
            let capture_supported = device.as_capture().is_some();
            devices[idx] = Some(device);
            state.cards[idx] = DeviceCard::Ready {
                status: Box::new(status),
                capture_supported,
            };
        }
        Err(e) => {
            devices[idx] = None;
            state.cards[idx] = DeviceCard::Failed(format!("{e}"));
        }
    }
}

enum RescanOutcome {
    Continue,
    Exit,
}

/// Re-enumerate connected devices, resetting the cache. Exits the sidebar when
/// nothing is left to show.
async fn rescan(
    state: &mut SidebarState,
    devices: &mut Vec<Option<Box<dyn Device>>>,
) -> RescanOutcome {
    let fresh = device::list_devices().await.unwrap_or_default();
    if fresh.is_empty() {
        return RescanOutcome::Exit;
    }
    *state = SidebarState::new(fresh);
    *devices = state.listings.iter().map(|_| None).collect();
    RescanOutcome::Continue
}

/// Whether an action loads its data with an AFC media walk. For these the
/// sidebar runs the walk inline (progress on the row) before suspending, so the
/// "scanning…" phase stays inside the TUI instead of flashing in a bare shell.
fn is_media_walk(action: DeviceAction) -> bool {
    matches!(action, DeviceAction::Analyze | DeviceAction::Media)
}

/// Whether an action prints to the terminal and returns, rather than running
/// its own full-screen TUI. The sidebar pauses for these so their output stays
/// readable; the TUI actions (Apps/Analyze/Logs/Capture) and the self-pausing
/// Card do not, so the user isn't left staring at a prompt over a blank screen.
fn prints_output(action: DeviceAction) -> bool {
    matches!(
        action,
        DeviceAction::Info
            | DeviceAction::Media
            | DeviceAction::Update
            | DeviceAction::Reboot
            | DeviceAction::Shutdown
    )
}

/// Dispatch the post-walk part of an action. Media-walk actions reuse the files
/// the sidebar already walked; everything else goes through the shared
/// [`device_action::run`].
async fn dispatch(
    device: &dyn Device,
    action: DeviceAction,
    prewalked: Option<Vec<MediaFile>>,
) -> Result<()> {
    match (action, prewalked) {
        (DeviceAction::Analyze, Some(files)) => analyze::pick_and_delete(device, files).await,
        (DeviceAction::Media, Some(files)) => {
            use std::io::Write;
            let mut out = anstream::stdout();
            write!(
                out,
                "{}",
                media::report(&files, false, device.media_roots())
            )?;
            Ok(())
        }
        (action, _) => device_action::run(device, action).await,
    }
}

/// Walk the device's media roots with the TUI up, streaming progress onto the
/// action's row. Returns the walked files, or `None` if the user cancelled the
/// scan with Esc / `q` — dropping the walk future stops the AFC traversal.
async fn walk_inline(
    term: &mut TerminalGuard,
    state: &mut SidebarState,
    events: &mut EventStream,
    device: &dyn Device,
    action: DeviceAction,
) -> Result<Option<Vec<MediaFile>>> {
    state.inline_load = Some(WalkLoad {
        action,
        progress: None,
    });
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WalkProgress>();
    let on_progress: WalkCallback = Box::new(move |p| {
        let _ = tx.send(p);
    });
    let mut walk = Box::pin(device.afc_walk(device.media_roots(), on_progress));

    let outcome = loop {
        term.0.draw(|f| draw(f, state))?;
        tokio::select! {
            biased;
            maybe_event = events.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if is_quit_key(&key) {
                        break None; // cancel: the walk future is dropped here
                    }
                }
            }
            result = &mut walk => break Some(result?),
            Some(p) = rx.recv() => {
                if let Some(load) = state.inline_load.as_mut() {
                    load.progress = Some(p);
                }
            }
            _ = tokio::time::sleep(REDRAW_TICK) => {}
        }
    };
    state.inline_load = None;
    Ok(outcome)
}

/// Keys that quit the sidebar or cancel an in-progress scan: `q`, Esc, Ctrl-C.
fn is_quit_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL)
}

fn handle_key(key: KeyEvent, state: &mut SidebarState) -> KeyOutcome {
    if is_quit_key(&key) {
        return KeyOutcome::Quit;
    }

    match key.code {
        KeyCode::Char('r') => return KeyOutcome::Rescan,
        KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
            state.focus = match state.focus {
                Focus::Devices => Focus::Actions,
                Focus::Actions => Focus::Devices,
            };
        }
        KeyCode::Up | KeyCode::Char('k') => match state.focus {
            Focus::Devices => state.move_selection(-1),
            Focus::Actions => state.move_action(-1),
        },
        KeyCode::Down | KeyCode::Char('j') => match state.focus {
            Focus::Devices => state.move_selection(1),
            Focus::Actions => state.move_action(1),
        },
        KeyCode::Enter => return handle_enter(state),
        _ => {}
    }
    KeyOutcome::None
}

/// Enter activates the focused pane: on the device list it moves focus to the
/// actions; on the actions it runs the highlighted one.
fn handle_enter(state: &mut SidebarState) -> KeyOutcome {
    let actions = state.current_actions();
    if actions.is_empty() {
        return KeyOutcome::None;
    }
    match state.focus {
        Focus::Devices => {
            state.focus = Focus::Actions;
            state.action_cursor = 0;
            KeyOutcome::None
        }
        Focus::Actions => actions
            .get(state.action_cursor)
            .copied()
            .map(KeyOutcome::Run)
            .unwrap_or(KeyOutcome::None),
    }
}

// ----- draw (pure) ---------------------------------------------------------

fn draw(frame: &mut Frame, state: &SidebarState) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(frame, area);
        return;
    }
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(SIDEBAR_WIDTH),
            Constraint::Min(RIGHT_MIN_WIDTH),
        ])
        .split(area);
    draw_devices(frame, cols[0], state);
    draw_right(frame, cols[1], state);
}

fn draw_too_small(frame: &mut Frame, area: Rect) {
    let msg = Paragraph::new("Terminal too small — enlarge the window.")
        .style(Style::default().fg(Color::Yellow));
    frame.render_widget(msg, area);
}

/// Style for a list's selected row. The selected item is ALWAYS visible (so the
/// user can read which device/action is current regardless of focus) — the
/// focused pane gets a solid reversed bar, the unfocused pane a plain bold row.
/// Never dimmed: a darker "selected" row is exactly what confused the eye.
fn selection_style(focused: bool) -> Style {
    if focused {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

/// Pane border style — the focused pane gets a cyan border, the other a dim one.
fn border_style(focused: bool) -> Style {
    Style::default().fg(if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    })
}

/// Pane title with a focus marker so the active pane is obvious even on
/// terminals that render border colours faintly.
fn pane_title(label: &str, focused: bool) -> String {
    if focused {
        format!("{label} ◂")
    } else {
        label.to_string()
    }
}

fn draw_devices(frame: &mut Frame, area: Rect, state: &SidebarState) {
    let focused = state.focus == Focus::Devices;
    let items: Vec<ListItem> = state
        .listings
        .iter()
        .zip(&state.cards)
        .map(|(listing, card)| device_item(listing, card))
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style(focused))
                .title(pane_title("Devices", focused)),
        )
        .highlight_style(selection_style(focused))
        .highlight_symbol("▸ ");
    let mut list_state = ListState::default();
    // Always mark the selected device, focused or not.
    list_state.select(Some(state.selected));
    frame.render_stateful_widget(list, area, &mut list_state);
}

/// One sidebar row: platform tag + name, model, and a battery/storage glance.
fn device_item<'a>(listing: &'a DeviceListing, card: &'a DeviceCard) -> ListItem<'a> {
    let tag = platform_tag(listing.platform);
    let name = listing
        .name
        .as_deref()
        .or(listing.model_friendly.as_deref())
        .unwrap_or("(untrusted)");
    let model = listing
        .model_friendly
        .as_deref()
        .or(listing.model_identifier.as_deref())
        .unwrap_or(DASH);

    ListItem::new(vec![
        Line::from(vec![
            Span::styled(
                format!("{tag} "),
                Style::default().fg(platform_color(listing.platform)),
            ),
            Span::raw(name.to_string()),
        ]),
        Line::from(Span::styled(
            format!("    {model}"),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            format!("    {}", card_glance(card)),
            Style::default().fg(Color::DarkGray),
        )),
    ])
}

/// The third sidebar line per device — a tiny battery/storage glance, or the
/// load state when not yet ready.
fn card_glance(card: &DeviceCard) -> String {
    match card {
        DeviceCard::Unloaded => DASH.to_string(),
        DeviceCard::Loading => "connecting…".to_string(),
        DeviceCard::Failed(_) => "unavailable".to_string(),
        DeviceCard::Ready { status, .. } => {
            let bat = format_percent(status.battery.level_percent);
            let disk = status
                .storage
                .map(|s| format!("{}%", s.used_percent()))
                .unwrap_or_else(|| DASH.to_string());
            format!("bat {bat}  disk {disk}")
        }
    }
}

fn draw_right(frame: &mut Frame, area: Rect, state: &SidebarState) {
    let actions = state.current_actions();
    let action_box_height = actions.len() as u16 + 2; // +2 for the border
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(7),
            Constraint::Length(action_box_height),
            Constraint::Length(1),
        ])
        .split(area);
    draw_summary(frame, rows[0], state);
    draw_actions(frame, rows[1], state, &actions);
    draw_hint(frame, rows[2], state);
}

fn draw_summary(frame: &mut Frame, area: Rect, state: &SidebarState) {
    let listing = &state.listings[state.selected];
    let title = listing.name.as_deref().unwrap_or("Device").to_string();
    let lines = match &state.cards[state.selected] {
        DeviceCard::Ready { status, .. } => summary_lines(status),
        DeviceCard::Loading => vec![Line::from("Connecting…")],
        DeviceCard::Unloaded => vec![Line::from("Select to load.")],
        DeviceCard::Failed(e) => vec![Line::from(Span::styled(
            e.clone(),
            Style::default().fg(Color::Red),
        ))],
    };
    let summary = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(summary, area);
}

/// Native (non-ANSI) compact device summary for the right pane. Mirrors the
/// dashboard's fields but as ratatui spans, since the dashboard renderer emits
/// terminal escape codes a `Paragraph` would print literally.
fn summary_lines(status: &DeviceStatus) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let model = status
        .model_friendly
        .as_deref()
        .or(status.model.as_deref())
        .unwrap_or(DASH);
    let os = status.os_name.as_deref().unwrap_or("iOS");
    let version = status.os_version.as_deref().unwrap_or(DASH);
    let os_line = match status.os_build.as_deref() {
        Some(b) => format!("{os} {version} (build {b})"),
        None => format!("{os} {version}"),
    };
    lines.push(Line::from(model.to_string()));
    lines.push(Line::from(Span::styled(
        os_line,
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));

    if let Some(storage) = status.storage {
        let pct = storage.used_percent();
        lines.push(Line::from(format!(
            "Storage  {bar}  {pct:>3}%  {used} / {total}",
            bar = format_bar(pct, SUMMARY_BAR_WIDTH),
            used = format_bytes(storage.used_bytes()),
            total = format_bytes(storage.total_bytes),
        )));
    }

    let battery = &status.battery;
    let level = format_percent(battery.level_percent);
    let charge = if battery.is_charging == Some(true) {
        " (charging)"
    } else {
        ""
    };
    let health = battery
        .health_percent
        .map(|p| format!("   Health {p}%"))
        .unwrap_or_default();
    lines.push(Line::from(format!("Battery  {level}{charge}{health}")));

    if let Some(count) = status.app_count {
        lines.push(Line::from(Span::styled(
            format!("{count} apps"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

fn draw_actions(frame: &mut Frame, area: Rect, state: &SidebarState, actions: &[DeviceAction]) {
    let focused = state.focus == Focus::Actions;
    let items: Vec<ListItem> = actions
        .iter()
        .map(|a| {
            // While an action loads, its live progress replaces the static
            // description so it stays legible in the narrow column instead of
            // being truncated off the right edge.
            let body = action_load_text(state, *a).unwrap_or_else(|| a.description().to_string());
            ListItem::new(format!("{:<10} {}", a.label(), body))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style(focused))
                .title(pane_title("Actions", focused)),
        )
        .highlight_style(selection_style(focused))
        .highlight_symbol("› ");
    let mut list_state = ListState::default();
    // Always mark the current action so it's visible from the device pane too.
    if !actions.is_empty() {
        list_state.select(Some(state.action_cursor.min(actions.len() - 1)));
    }
    frame.render_stateful_widget(list, area, &mut list_state);
}

/// Live progress text for an action's row while its media walk runs, or `None`
/// for every action except the one currently loading. Shown in place of the
/// description so it never gets truncated off the narrow column.
fn action_load_text(state: &SidebarState, action: DeviceAction) -> Option<String> {
    let load = state.inline_load.as_ref().filter(|l| l.action == action)?;
    Some(match load.progress {
        Some(p) => format!(
            "scanning {} files, {}",
            p.files_seen,
            format_bytes(p.bytes_seen)
        ),
        None => "loading…".to_string(),
    })
}

fn draw_hint(frame: &mut Frame, area: Rect, state: &SidebarState) {
    let text = if state.inline_load.is_some() {
        "Esc / q  cancel scan"
    } else {
        "↑↓ select · Tab focus · Enter run · r rescan · q quit"
    };
    let hint = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, area);
}

/// Three-letter platform tag for the narrow sidebar (`iOS` / `And`).
fn platform_tag(platform: Platform) -> &'static str {
    match platform {
        Platform::Ios => "iOS",
        Platform::Android => "And",
    }
}

fn platform_color(platform: Platform) -> Color {
    match platform {
        Platform::Ios => Color::Cyan,
        Platform::Android => Color::Green,
    }
}

/// Silences the process's stderr while alive, restoring it on drop. The
/// sidebar runs background device loads and an inline media walk while it owns
/// the alt screen; those log best-effort warnings to stderr (`afc_walk`
/// skips, enrichment failures) that would otherwise scribble over the ratatui
/// frame and desync its diff renderer. `None` (e.g. the redirect failed) is
/// harmless — the warnings just aren't suppressed.
///
/// On Unix this redirects fd 2 to `/dev/null`.
#[cfg(unix)]
struct SilencedStderr {
    /// A dup of the original stderr, restored over fd 2 on drop.
    original: i32,
}

#[cfg(unix)]
impl SilencedStderr {
    fn new() -> Option<Self> {
        // SAFETY: all calls operate on the process's own stderr fd. `dup`
        // saves it, `open` gets a /dev/null fd, `dup2` points fd 2 at it, then
        // the /dev/null fd is closed (fd 2 keeps the description alive).
        unsafe {
            let original = libc::dup(libc::STDERR_FILENO);
            if original < 0 {
                return None;
            }
            let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
            if devnull < 0 {
                libc::close(original);
                return None;
            }
            libc::dup2(devnull, libc::STDERR_FILENO);
            libc::close(devnull);
            Some(Self { original })
        }
    }
}

#[cfg(unix)]
impl Drop for SilencedStderr {
    fn drop(&mut self) {
        // SAFETY: restore the saved stderr over fd 2 and release the dup.
        unsafe {
            libc::dup2(self.original, libc::STDERR_FILENO);
            libc::close(self.original);
        }
    }
}

/// Silences the process's stderr while alive, restoring it on drop. See the
/// Unix variant above for why the sidebar needs this.
///
/// On Windows there is no fd 2 to `dup2` over: Rust's `eprintln!` writes
/// through the console handle it gets from `GetStdHandle(STD_ERROR_HANDLE)`
/// on every write, so pointing `STD_ERROR_HANDLE` at the `NUL` device via
/// `SetStdHandle` silences it immediately ([issue #13]).
///
/// [issue #13]: https://github.com/dutradotdev/quokka/issues/13
#[cfg(windows)]
struct SilencedStderr {
    /// The original `STD_ERROR_HANDLE` (owned by the console, never closed),
    /// restored on drop.
    original: windows_sys::Win32::Foundation::HANDLE,
    /// The open `NUL` handle stderr points at while silenced; closed on drop.
    devnull: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl SilencedStderr {
    fn new() -> Option<Self> {
        use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows_sys::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE};

        /// `"NUL"` as a NUL-terminated UTF-16 path for `CreateFileW`.
        const NUL_DEVICE: [u16; 4] = [b'N' as u16, b'U' as u16, b'L' as u16, 0];

        // SAFETY: all calls operate on the process's own std handles. The
        // original handle is saved but never closed (the console owns it);
        // the NUL handle stays open while it backs STD_ERROR_HANDLE and is
        // closed only after the original is restored on drop.
        unsafe {
            let original = GetStdHandle(STD_ERROR_HANDLE);
            let devnull = CreateFileW(
                NUL_DEVICE.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if devnull == INVALID_HANDLE_VALUE {
                return None;
            }
            if SetStdHandle(STD_ERROR_HANDLE, devnull) == 0 {
                CloseHandle(devnull);
                return None;
            }
            Some(Self { original, devnull })
        }
    }
}

#[cfg(windows)]
impl Drop for SilencedStderr {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};
        // SAFETY: restore the saved handle over STD_ERROR_HANDLE, then close
        // the NUL handle nothing references anymore.
        unsafe {
            SetStdHandle(STD_ERROR_HANDLE, self.original);
            CloseHandle(self.devnull);
        }
    }
}

/// RAII guard for the sidebar's terminal state: raw mode, the alt screen, and
/// stderr suppression (so device-layer `eprintln!` warnings can't scribble over
/// the frame). Created via [`enter`](Self::enter) when the sidebar takes the
/// screen and dropped — fully restoring all three — when it hands control to a
/// sub-command, which then owns a pristine terminal exactly as the single-device
/// menu does. The next sub-command return rebuilds a fresh guard.
struct TerminalGuard(
    Terminal<CrosstermBackend<io::Stdout>>,
    Option<SilencedStderr>,
);

impl TerminalGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, terminal::EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        Ok(Self(Terminal::new(backend)?, SilencedStderr::new()))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), terminal::LeaveAlternateScreen);
        // Drop the stderr silencer (restores fd 2). `take()` also reads the
        // field, which is otherwise only a drop-guard.
        self.1.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Battery, Storage};
    use ratatui::backend::TestBackend;

    fn listing(platform: Platform, udid: &str, name: &str, model: &str) -> DeviceListing {
        DeviceListing {
            platform,
            udid: udid.into(),
            connection: "USB",
            name: Some(name.into()),
            model_identifier: None,
            model_friendly: Some(model.into()),
        }
    }

    fn ready_status(name: &str, os: &str) -> DeviceStatus {
        DeviceStatus {
            name: Some(name.into()),
            model_friendly: Some("iPhone 15 Pro Max".into()),
            os_name: Some(os.into()),
            os_version: Some("18.2".into()),
            os_build: Some("22C152".into()),
            storage: Some(Storage {
                total_bytes: 256_000_000_000,
                free_bytes: 148_500_000_000,
                ..Default::default()
            }),
            battery: Battery {
                level_percent: Some(87),
                health_percent: Some(91),
                ..Default::default()
            },
            app_count: Some(47),
            ..Default::default()
        }
    }

    fn two_device_state() -> SidebarState {
        let mut state = SidebarState::new(vec![
            listing(
                Platform::Ios,
                "UDID-1",
                "Lucas's iPhone",
                "iPhone 15 Pro Max",
            ),
            listing(Platform::Android, "ABC123", "Pixel 8", "Pixel 8"),
        ]);
        state.cards[0] = DeviceCard::Ready {
            status: Box::new(ready_status("Lucas's iPhone", "iOS")),
            capture_supported: true,
        };
        state.cards[1] = DeviceCard::Ready {
            status: Box::new(ready_status("Pixel 8", "Android")),
            capture_supported: false,
        };
        state
    }

    fn render(state: &SidebarState, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, state)).unwrap();
        let buf = terminal.backend().buffer();
        let width = buf.area.width as usize;
        let mut out = String::new();
        for (i, cell) in buf.content().iter().enumerate() {
            if i > 0 && i % width == 0 {
                out.push('\n');
            }
            out.push_str(cell.symbol());
        }
        out.lines()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn step_clamps_without_wrapping() {
        assert_eq!(step(0, -1, 3), 0);
        assert_eq!(step(2, 1, 3), 2);
        assert_eq!(step(1, 1, 3), 2);
        assert_eq!(step(0, 5, 0), 0);
    }

    #[test]
    fn sidebar_lists_both_devices_with_platform_tags() {
        let out = render(&two_device_state(), 90, 24);
        assert!(out.contains("Devices"));
        assert!(out.contains("Lucas's iPhone"));
        assert!(out.contains("Pixel 8"));
        assert!(out.contains("iOS"));
        assert!(out.contains("And"));
    }

    #[test]
    fn right_pane_shows_selected_summary_and_actions() {
        let out = render(&two_device_state(), 90, 24);
        assert!(out.contains("Actions"));
        assert!(out.contains("Apps"));
        assert!(out.contains("iOS 18.2"));
        // iOS device is selected first → capture is offered.
        assert!(out.contains("Capture"));
    }

    #[test]
    fn renders_with_a_single_device() {
        // The sidebar is the default view even for one device: it must render
        // the lone device, its dashboard, and the action menu without panicking.
        let mut state = SidebarState::new(vec![listing(
            Platform::Ios,
            "UDID-1",
            "Lucas's iPhone",
            "iPhone 15 Pro Max",
        )]);
        state.cards[0] = DeviceCard::Ready {
            status: Box::new(ready_status("Lucas's iPhone", "iOS")),
            capture_supported: true,
        };
        let out = render(&state, 90, 24);
        assert!(out.contains("Lucas's iPhone"));
        assert!(out.contains("iOS 18.2"));
        assert!(out.contains("Apps"));
    }

    #[test]
    fn android_selection_hides_capture_action() {
        let mut state = two_device_state();
        state.move_selection(1); // select the Android device
        let out = render(&state, 90, 24);
        assert!(out.contains("Pixel 8"));
        assert!(!out.contains("Capture"));
    }

    #[test]
    fn too_small_terminal_shows_hint_instead_of_panicking() {
        let out = render(&two_device_state(), 30, 8);
        assert!(out.contains("too small"));
    }

    #[test]
    fn enter_on_devices_moves_focus_to_actions_then_runs() {
        let mut state = two_device_state();
        assert_eq!(state.focus, Focus::Devices);
        // First Enter focuses the action list.
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(handle_key(key, &mut state), KeyOutcome::None));
        assert_eq!(state.focus, Focus::Actions);
        // Second Enter runs the highlighted action (Apps is first).
        match handle_key(key, &mut state) {
            KeyOutcome::Run(DeviceAction::Apps) => {}
            _ => panic!("expected Run(Apps)"),
        }
    }

    #[test]
    fn unloaded_device_offers_no_actions() {
        let state = SidebarState::new(vec![
            listing(Platform::Ios, "UDID-1", "A", "iPhone 15"),
            listing(Platform::Android, "ABC", "B", "Pixel 8"),
        ]);
        assert!(state.current_actions().is_empty());
    }

    #[test]
    fn next_to_load_prefers_selected_then_fills_in() {
        let mut state = SidebarState::new(vec![
            listing(Platform::Ios, "UDID-1", "A", "iPhone 15"),
            listing(Platform::Android, "ABC", "B", "Pixel 8"),
        ]);
        // Selected (0) loads first.
        assert_eq!(state.next_to_load(), Some(0));
        state.cards[0] = DeviceCard::Ready {
            status: Box::new(ready_status("A", "iOS")),
            capture_supported: true,
        };
        // Then the remaining unloaded device.
        assert_eq!(state.next_to_load(), Some(1));
        state.cards[1] = DeviceCard::Loading;
        assert_eq!(state.next_to_load(), None);
    }

    #[test]
    fn only_analyze_and_media_load_inline() {
        assert!(is_media_walk(DeviceAction::Analyze));
        assert!(is_media_walk(DeviceAction::Media));
        assert!(!is_media_walk(DeviceAction::Info));
        assert!(!is_media_walk(DeviceAction::Apps));
    }

    #[test]
    fn only_printing_actions_pause_before_redraw() {
        // Print-and-return actions pause so their output stays readable.
        assert!(prints_output(DeviceAction::Info));
        assert!(prints_output(DeviceAction::Media));
        // Full-screen TUI actions (and the self-pausing Card) must not pause,
        // or the user is left at a prompt over a blank screen.
        assert!(!prints_output(DeviceAction::Apps));
        assert!(!prints_output(DeviceAction::Analyze));
        assert!(!prints_output(DeviceAction::Logs));
        assert!(!prints_output(DeviceAction::Capture));
        assert!(!prints_output(DeviceAction::Card));
    }

    #[test]
    fn focus_marker_moves_to_the_active_pane() {
        let mut state = two_device_state();
        // Default focus is the device list.
        let out = render(&state, 90, 24);
        assert!(out.contains("Devices ◂"));
        assert!(!out.contains("Actions ◂"));
        // Tabbing to the actions moves the marker.
        state.focus = Focus::Actions;
        let out = render(&state, 90, 24);
        assert!(out.contains("Actions ◂"));
        assert!(!out.contains("Devices ◂"));
    }

    #[test]
    fn selected_device_is_marked_even_when_actions_have_focus() {
        let mut state = two_device_state();
        state.focus = Focus::Actions;
        // The "▸" symbol marks the current device regardless of focus, so the
        // user never loses track of which device an action will run against.
        let out = render(&state, 90, 24);
        assert!(out.contains("▸"));
    }

    #[test]
    fn action_row_shows_inline_scan_progress() {
        let mut state = two_device_state();
        state.focus = Focus::Actions;
        state.inline_load = Some(WalkLoad {
            action: DeviceAction::Analyze,
            progress: Some(WalkProgress {
                files_seen: 23,
                bytes_seen: 30_600_000_000,
            }),
        });
        let out = render(&state, 90, 24);
        // Progress is shown on the Analyze row itself, not in a bare shell.
        assert!(out.contains("scanning 23 files"));
        assert!(out.contains("30.6 GB"));
    }

    #[test]
    fn quit_keys_cover_esc_q_and_ctrl_c() {
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let plain_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(is_quit_key(&q));
        assert!(is_quit_key(&esc));
        assert!(is_quit_key(&ctrl_c));
        assert!(!is_quit_key(&plain_c));
    }

    #[test]
    fn hint_offers_cancel_while_scanning() {
        let mut state = two_device_state();
        state.focus = Focus::Actions;
        // No scan in progress → normal hint.
        assert!(render(&state, 90, 24).contains("rescan"));
        // Scanning → the hint tells the user how to cancel.
        state.inline_load = Some(WalkLoad {
            action: DeviceAction::Media,
            progress: None,
        });
        assert!(render(&state, 90, 24).contains("cancel scan"));
    }
}
