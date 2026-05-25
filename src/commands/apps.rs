use std::collections::HashSet;
use std::io::{BufRead, IsTerminal, Write};

use anyhow::{anyhow, bail, Result};
use dialoguer::{theme::ColorfulTheme, Confirm};
use owo_colors::OwoColorize;

use crate::device::{App, BatchCallback, Device};
use crate::ui::{format_bytes, spinner};

/// Run-time options for `quokka apps`.
pub struct Options {
    pub uninstall: Option<String>,
    pub assume_yes: bool,
}

pub async fn run(device: &dyn Device, opts: Options) -> Result<()> {
    match opts.uninstall {
        Some(bundle_id) => uninstall_flow(device, &bundle_id, opts.assume_yes).await,
        None => list_flow(device).await,
    }
}

async fn list_flow(device: &dyn Device) -> Result<()> {
    let bar = spinner("Loading apps...");
    let basic = device.apps().await;
    bar.finish_and_clear();
    let basic = user_apps_by_size(basic?);

    // Non-TTY (pipe, CI): no checkbox menu would be visible — enrich
    // silently and print the final list as text.
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if !interactive {
        let noop: BatchCallback = Box::new(|_| {});
        let full = user_apps_by_size(device.with_dynamic_sizes(basic, noop).await?);
        let mut out = anstream::stdout();
        writeln!(out, "{}", render_app_list(&full))?;
        return Ok(());
    }

    // First entry computes every app's dynamic size in the background.
    // After an uninstall the picker re-enters with the surviving apps;
    // `enriched` carries the bundle ids that already have a real size so
    // Phase 2 only processes what is still pending — progress from an
    // earlier round is never thrown away.
    let bundle_sizes: std::collections::HashMap<String, u64> = basic
        .iter()
        .map(|a| (a.bundle_id.clone(), a.size_bytes))
        .collect();
    let mut apps = basic;
    let mut enriched: HashSet<String> = HashSet::new();
    loop {
        let (outcome, current, now_enriched) =
            tui::run(device, apps, &bundle_sizes, &enriched).await?;
        apps = current;
        enriched = now_enriched;
        match outcome {
            tui::Outcome::Quit => return Ok(()),
            tui::Outcome::Picked(picked) => {
                if picked.is_empty() {
                    return Ok(());
                }
                let removed = confirm_and_uninstall(device, &picked).await?;
                if removed.is_empty() {
                    continue;
                }
                apps.retain(|a| !removed.contains(&a.bundle_id));
                if apps.is_empty() {
                    let mut out = anstream::stdout();
                    writeln!(out, "No more user apps installed.")?;
                    return Ok(());
                }
            }
        }
    }
}

async fn confirm_and_uninstall(device: &dyn Device, picked: &[App]) -> Result<Vec<String>> {
    let total: u64 = picked.iter().map(|a| a.size_bytes).sum();
    let mut out = anstream::stdout();
    let confirmed = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(format!(
            "Uninstall {} app(s) (~{})? App data will be permanently lost.",
            picked.len(),
            format_bytes(total),
        ))
        .default(true)
        .interact()?;
    if !confirmed {
        writeln!(out, "Aborted.")?;
        return Ok(Vec::new());
    }
    let mut removed = Vec::with_capacity(picked.len());
    for app in picked {
        match run_uninstall(device, app, &mut out).await {
            Ok(()) => removed.push(app.bundle_id.clone()),
            Err(e) => writeln!(
                out,
                "{} {} ({}) failed: {e}",
                "✗".red(),
                app.name.bold(),
                app.bundle_id.dimmed(),
            )?,
        }
    }
    Ok(removed)
}

async fn run_uninstall(device: &dyn Device, app: &App, out: &mut dyn Write) -> Result<()> {
    let bar = spinner(format!("Uninstalling {}...", app.name));
    let result = device.uninstall_app(&app.bundle_id).await;
    bar.finish_and_clear();
    result?;
    writeln!(
        out,
        "{} {} ({}) removed.",
        "✓".green(),
        app.name.bold(),
        app.bundle_id.dimmed(),
    )?;
    Ok(())
}

async fn uninstall_flow(device: &dyn Device, bundle_id: &str, assume_yes: bool) -> Result<()> {
    let bar = spinner("Locating app...");
    let lookup = device.app(bundle_id).await;
    bar.finish_and_clear();

    let target = lookup?.ok_or_else(|| {
        anyhow!(
            "No app with bundle id `{bundle_id}` is installed. Run `quokka apps` to see what's there."
        )
    })?;
    let target = &target;

    let is_interactive = std::io::stdin().is_terminal();
    let mut input = std::io::stdin().lock();
    let mut output = anstream::stdout();
    confirm_uninstall(target, assume_yes, is_interactive, &mut input, &mut output)?;
    run_uninstall(device, target, &mut output).await
}

fn confirm_uninstall(
    app: &App,
    assume_yes: bool,
    is_interactive: bool,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<()> {
    if assume_yes {
        return Ok(());
    }
    if !is_interactive {
        bail!(
            "`--uninstall` is destructive — pass `--yes` to confirm non-interactively, or run \
             this in an interactive terminal."
        );
    }
    writeln!(
        output,
        "About to uninstall {} ({}), {}.",
        app.name.bold(),
        app.bundle_id.dimmed(),
        format_bytes(app.size_bytes).yellow(),
    )?;
    writeln!(
        output,
        "{}",
        "App data on the device will be permanently lost.".yellow()
    )?;
    write!(output, "Type `y` to confirm: ")?;
    output.flush()?;

    let mut answer = String::new();
    input.read_line(&mut answer)?;
    if !parse_confirmation(&answer) {
        bail!("Aborted.");
    }
    Ok(())
}

fn parse_confirmation(input: &str) -> bool {
    matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

pub(crate) fn user_apps_by_size(mut apps: Vec<App>) -> Vec<App> {
    apps.retain(|a| !a.is_system);
    apps.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
    apps
}

/// Apps whose dynamic (cache + data) size has not been computed yet — the
/// ones whose bundle id is absent from `enriched`. When the picker is
/// re-entered after an uninstall, Phase 2 only needs to process these, so
/// the sizes already computed in an earlier round are preserved.
pub(crate) fn unenriched_apps(apps: &[App], enriched: &HashSet<String>) -> Vec<App> {
    apps.iter()
        .filter(|a| !enriched.contains(&a.bundle_id))
        .cloned()
        .collect()
}

pub(crate) fn render_app_list(apps: &[App]) -> String {
    if apps.is_empty() {
        return "No user apps installed.\n".to_string();
    }
    let name_width = apps
        .iter()
        .map(|a| a.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(4);
    let bundle_width = apps
        .iter()
        .map(|a| a.bundle_id.chars().count())
        .max()
        .unwrap_or(0)
        .max(9);
    let mut out = String::new();
    for app in apps {
        let size = format_bytes(app.size_bytes);
        out.push_str(&format!(
            "{name:<name_width$}  {bundle:<bundle_width$}  {size:>10}\n",
            name = app.name,
            bundle = app.bundle_id.dimmed(),
            size = size,
        ));
    }
    let total: u64 = apps.iter().map(|a| a.size_bytes).sum();
    let count = apps.len();
    let plural = if count == 1 { "app" } else { "apps" };
    out.push_str(&format!(
        "{count} {plural} · {total} total\n",
        total = format_bytes(total).bold()
    ));
    out
}

mod tui {
    use std::collections::{HashMap, HashSet};
    use std::io;
    use std::time::Duration;

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

    use super::{unenriched_apps, user_apps_by_size};
    use crate::device::{App, BatchCallback, BatchUpdate, Device};
    use crate::ui::format_bytes;

    pub enum Outcome {
        Quit,
        Picked(Vec<App>),
    }

    const DASH: &str = "—";

    /// Run the picker. `enriched` holds the bundle ids whose dynamic size
    /// was already computed in an earlier round; Phase 2 runs in the
    /// background for the remaining apps only, and is skipped entirely
    /// when none are left. Returns the outcome, the current app list, and
    /// the updated set of enriched bundle ids so the caller can re-enter
    /// the picker after an uninstall without discarding progress.
    pub async fn run(
        device: &dyn Device,
        initial: Vec<App>,
        bundle_sizes: &HashMap<String, u64>,
        enriched: &HashSet<String>,
    ) -> Result<(Outcome, Vec<App>, HashSet<String>)> {
        if initial.is_empty() {
            println!("No user apps installed.");
            return Ok((Outcome::Quit, initial, enriched.clone()));
        }

        let mut state = State::new(initial, bundle_sizes.clone(), enriched);
        let mut term = TerminalGuard::enter()?;
        let mut events = EventStream::new();

        // Phase 2 only processes apps without a computed size yet — on a
        // re-entry after an uninstall that is just the still-pending ones.
        // Skip the plumbing entirely when nothing is left to enrich.
        let pending = unenriched_apps(&state.apps, enriched);
        let (mut rx, mut enrich_fut, mut enrich_done) = if pending.is_empty() {
            (None, None, true)
        } else {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<BatchUpdate>();
            let on_batch: BatchCallback = Box::new(move |u| {
                let _ = tx.send(u);
            });
            let fut = Box::pin(device.with_dynamic_sizes(pending, on_batch));
            (Some(rx), Some(fut), false)
        };

        let outcome = 'main: loop {
            term.0.draw(|f| draw(f, &mut state))?;

            tokio::select! {
                biased;
                maybe_event = events.next() => {
                    if let Some(Ok(event)) = maybe_event {
                        if let Some(out) = handle_event(event, &mut state) {
                            break 'main out;
                        }
                    }
                }
                Some(update) = async {
                    match rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    state.apply(update);
                }
                result = async {
                    match enrich_fut.as_mut() {
                        Some(fut) => fut.await,
                        None => std::future::pending().await,
                    }
                }, if !enrich_done => {
                    result?;
                    enrich_done = true;
                    state.computing = None;
                }
                _ = tokio::time::sleep(Duration::from_millis(120)) => {}
            }
        };

        let final_enriched = state.enriched_ids();
        Ok((outcome, state.apps, final_enriched))
    }

    struct State {
        apps: Vec<App>,
        bundle_sizes: HashMap<String, u64>,
        selected: Vec<bool>,
        enriched: Vec<bool>,
        list: ListState,
        computing: Option<(usize, usize)>,
        sorted_by_total: bool,
        search_query: String,
        search_active: bool,
    }

    impl State {
        fn new(
            apps: Vec<App>,
            bundle_sizes: HashMap<String, u64>,
            enriched_ids: &HashSet<String>,
        ) -> Self {
            let len = apps.len();
            let mut list = ListState::default();
            list.select(Some(0));
            let enriched: Vec<bool> = apps
                .iter()
                .map(|a| enriched_ids.contains(&a.bundle_id))
                .collect();
            let pending = enriched.iter().filter(|&&done| !done).count();
            Self {
                apps,
                bundle_sizes,
                selected: vec![false; len],
                enriched,
                list,
                computing: if pending > 0 {
                    Some((0, pending))
                } else {
                    None
                },
                sorted_by_total: false,
                search_query: String::new(),
                search_active: false,
            }
        }

        fn visible(&self) -> Vec<usize> {
            if self.search_query.is_empty() {
                return (0..self.apps.len()).collect();
            }
            let needle = self.search_query.to_lowercase();
            self.apps
                .iter()
                .enumerate()
                .filter(|(_, a)| {
                    a.name.to_lowercase().contains(&needle)
                        || a.bundle_id.to_lowercase().contains(&needle)
                })
                .map(|(i, _)| i)
                .collect()
        }

        fn cursor_app_index(&self) -> Option<usize> {
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

        fn apply(&mut self, update: BatchUpdate) {
            for app in update.apps {
                if let Some(i) = self.apps.iter().position(|a| a.bundle_id == app.bundle_id) {
                    self.apps[i] = app;
                    self.enriched[i] = true;
                }
            }
            // Positions stay stable during streaming updates — re-sorting
            // would jump rows under the user's cursor. Sort is opt-in via
            // the `s` keybind once enrichment is done.
            self.computing = if update.done >= update.total {
                None
            } else {
                Some((update.done, update.total))
            };
        }

        fn sort_by_total(&mut self) {
            // Only meaningful once every row has a real total.
            if self.computing.is_some() || self.sorted_by_total {
                return;
            }
            let picked: HashSet<String> = self
                .apps
                .iter()
                .enumerate()
                .filter(|&(i, _)| self.selected[i])
                .map(|(_, a)| a.bundle_id.clone())
                .collect();
            self.apps.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
            self.selected = self
                .apps
                .iter()
                .map(|a| picked.contains(&a.bundle_id))
                .collect();
            // All rows are enriched at this point (gated above), so no
            // need to re-thread `enriched`.
            self.enriched = vec![true; self.apps.len()];
            self.list.select(Some(0));
            self.sorted_by_total = true;
        }

        fn bundle_of(&self, app: &App) -> u64 {
            self.bundle_sizes
                .get(&app.bundle_id)
                .copied()
                .unwrap_or(app.size_bytes)
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
            if let Some(i) = self.cursor_app_index() {
                if let Some(slot) = self.selected.get_mut(i) {
                    *slot = !*slot;
                }
            }
        }

        fn picked(&self) -> Vec<App> {
            self.selected
                .iter()
                .enumerate()
                .filter(|&(_, &sel)| sel)
                .map(|(i, _)| self.apps[i].clone())
                .collect()
        }

        /// Bundle ids whose dynamic size has been computed — handed back to
        /// the caller so a later picker re-entry can skip them.
        fn enriched_ids(&self) -> HashSet<String> {
            self.apps
                .iter()
                .zip(&self.enriched)
                .filter(|(_, &done)| done)
                .map(|(a, _)| a.bundle_id.clone())
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

        // Search-input mode swallows most keys so the user can type a
        // query without triggering nav/quit shortcuts.
        if state.search_active {
            match code {
                KeyCode::Esc => {
                    state.search_active = false;
                    state.search_query.clear();
                    state.clamp_cursor();
                }
                KeyCode::Enter => {
                    state.search_active = false;
                }
                KeyCode::Backspace => {
                    state.search_query.pop();
                    state.clamp_cursor();
                }
                KeyCode::Char(c) if modifiers == KeyModifiers::CONTROL && c == 'c' => {
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
            (KeyCode::Enter, _) => {
                let picked = user_apps_by_size(state.picked());
                Some(Outcome::Picked(picked))
            }
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
            (KeyCode::Char('s'), _) => {
                state.sort_by_total();
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
                Constraint::Length(1),                               // status
                Constraint::Length(if show_search { 1 } else { 0 }), // search bar
                Constraint::Length(1),                               // column header
                Constraint::Min(1),                                  // list
                Constraint::Length(1),                               // footer
            ])
            .split(f.area());

        // Status line
        let (status, status_style) = match state.computing {
            Some((done, total)) => {
                let pct = (done * 100).checked_div(total).unwrap_or(100);
                (
                    format!(" Computing real sizes (cache & data)... {done}/{total} ({pct}%)"),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            }
            None => {
                let suffix = if state.sorted_by_total {
                    " · sorted by total"
                } else {
                    ""
                };
                (
                    format!(" Real sizes computed.{suffix}"),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
            }
        };
        f.render_widget(Paragraph::new(status).style(status_style), chunks[0]);

        // Search bar (rendered only when active or non-empty query).
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

        // Column widths sized to the visible (filtered) rows so the
        // table stays compact when filtering down to a few apps.
        let visible = state.visible();
        let total_w = visible
            .iter()
            .map(|&i| format_bytes(state.apps[i].size_bytes).chars().count())
            .max()
            .unwrap_or(0)
            .max("total".len());
        let bundle_w = visible
            .iter()
            .map(|&i| {
                format_bytes(state.bundle_of(&state.apps[i]))
                    .chars()
                    .count()
            })
            .max()
            .unwrap_or(0)
            .max("bundle".len());
        let cache_w = visible
            .iter()
            .map(|&i| {
                if state.enriched[i] {
                    let bundle = state.bundle_of(&state.apps[i]);
                    format_bytes(state.apps[i].size_bytes.saturating_sub(bundle))
                        .chars()
                        .count()
                } else {
                    DASH.chars().count()
                }
            })
            .max()
            .unwrap_or(0)
            .max("cache".len());

        // Column header. The leading indent matches the list rows:
        // 1 (highlight symbol column) + 4 (checkbox "[ ] ").
        let header_indent = " ".repeat(1 + 4);
        let header_line = format!(
            "{header_indent}{:>total_w$}  {:>bundle_w$}  {:>cache_w$}  app",
            "total", "bundle", "cache",
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
            .map(|&i| row(&state.apps[i], i, state, total_w, bundle_w, cache_w))
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

        // Footer: keybinds. In search-input mode we swap to input hints
        // since most normal keys are captured by the field.
        let footer_spans: Vec<Span> = if state.search_active {
            vec![
                Span::raw(" type to filter · "),
                Span::styled("enter", Style::default().fg(Color::Cyan)),
                Span::raw(" apply · "),
                Span::styled("esc", Style::default().fg(Color::Cyan)),
                Span::raw(" clear"),
            ]
        } else {
            let mut spans = vec![
                Span::styled(" ↑↓", Style::default().fg(Color::Cyan)),
                Span::raw(" nav · "),
                Span::styled("space", Style::default().fg(Color::Cyan)),
                Span::raw(" toggle · "),
                Span::styled("enter", Style::default().fg(Color::Cyan)),
                Span::raw(" uninstall · "),
                Span::styled("/", Style::default().fg(Color::Cyan)),
                Span::raw(" search · "),
                Span::styled("q", Style::default().fg(Color::Cyan)),
                Span::raw(" quit"),
            ];
            if state.computing.is_none() && !state.sorted_by_total {
                spans.push(Span::raw(" · "));
                spans.push(Span::styled("s", Style::default().fg(Color::Cyan)));
                spans.push(Span::raw(" sort by total"));
            }
            spans
        };
        f.render_widget(
            Paragraph::new(Line::from(footer_spans)).style(Style::default().fg(Color::Gray)),
            chunks[4],
        );
    }

    fn row(
        app: &App,
        i: usize,
        state: &State,
        total_w: usize,
        bundle_w: usize,
        cache_w: usize,
    ) -> ListItem<'static> {
        let checkbox = if state.selected[i] { "[x]" } else { "[ ]" };
        let bundle = state.bundle_of(app);
        let total_str = format_bytes(app.size_bytes);
        let bundle_str = format_bytes(bundle);
        let cache_str = if state.enriched[i] {
            let cache = app.size_bytes.saturating_sub(bundle);
            if cache == 0 {
                DASH.to_string()
            } else {
                format_bytes(cache)
            }
        } else {
            DASH.to_string()
        };

        // Pre-enrichment rows render total + cache dimmed (the total is
        // still bundle-only and cache is unknown).
        let pending = !state.enriched[i];
        let dim = Style::default().fg(Color::DarkGray);
        let normal = Style::default();

        ListItem::new(Line::from(vec![
            Span::raw(format!(" {checkbox}  ")),
            Span::styled(
                format!("{total_str:>total_w$}"),
                if pending { dim } else { normal },
            ),
            Span::raw("  "),
            Span::styled(format!("{bundle_str:>bundle_w$}"), normal),
            Span::raw("  "),
            Span::styled(
                format!("{cache_str:>cache_w$}"),
                if pending { dim } else { normal },
            ),
            Span::raw("  "),
            Span::raw(app.name.clone()),
        ]))
    }

    /// RAII guard for terminal raw mode + alt screen. Restores on drop so
    /// a panic mid-TUI doesn't leave the terminal scrambled.
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
    use std::collections::HashSet;
    use std::io::Cursor;

    fn sample() -> Vec<App> {
        vec![
            App {
                bundle_id: "com.user.small".into(),
                name: "Small".into(),
                size_bytes: 50_000_000,
                is_system: false,
            },
            App {
                bundle_id: "com.apple.System".into(),
                name: "System".into(),
                size_bytes: 9_999_999_999,
                is_system: true,
            },
            App {
                bundle_id: "com.user.big".into(),
                name: "Big".into(),
                size_bytes: 500_000_000,
                is_system: false,
            },
        ]
    }

    #[test]
    fn user_apps_filters_system_and_sorts_descending() {
        let sorted = user_apps_by_size(sample());
        let names: Vec<_> = sorted.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Big", "Small"]);
    }

    #[test]
    fn user_apps_by_size_is_stable_for_equal_sizes() {
        // Stable sort is part of the TUI contract: a re-render with the same
        // sizes must not visually reshuffle equal-sized rows under the cursor.
        // `sort_by_key` is stable; a careless switch to `sort_unstable_by_key`
        // would regress this.
        let apps = vec![
            App {
                bundle_id: "com.user.a".into(),
                name: "A".into(),
                size_bytes: 100,
                is_system: false,
            },
            App {
                bundle_id: "com.user.b".into(),
                name: "B".into(),
                size_bytes: 100,
                is_system: false,
            },
            App {
                bundle_id: "com.user.c".into(),
                name: "C".into(),
                size_bytes: 100,
                is_system: false,
            },
        ];
        let sorted = user_apps_by_size(apps);
        let bundle_ids: Vec<_> = sorted.iter().map(|a| a.bundle_id.as_str()).collect();
        assert_eq!(
            bundle_ids,
            vec!["com.user.a", "com.user.b", "com.user.c"],
            "equal sizes must keep input order"
        );
    }

    #[test]
    fn user_apps_by_size_yields_empty_when_only_system_apps() {
        let only_system = vec![App {
            bundle_id: "com.apple.Foo".into(),
            name: "Foo".into(),
            size_bytes: 1,
            is_system: true,
        }];
        assert!(user_apps_by_size(only_system).is_empty());
    }

    #[test]
    fn unenriched_apps_returns_only_apps_without_a_computed_size() {
        let apps = user_apps_by_size(sample());
        let enriched: HashSet<String> = ["com.user.big".to_string()].into_iter().collect();
        let pending = unenriched_apps(&apps, &enriched);
        // Regression guard: re-entering the picker after an uninstall must
        // still treat a not-yet-computed app as pending — never as already
        // enriched just because an earlier round was in progress.
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].bundle_id, "com.user.small");
    }

    #[test]
    fn unenriched_apps_returns_all_when_nothing_is_enriched() {
        let apps = user_apps_by_size(sample());
        let pending = unenriched_apps(&apps, &HashSet::new());
        assert_eq!(pending.len(), apps.len());
    }

    #[test]
    fn unenriched_apps_is_empty_when_every_app_is_enriched() {
        let apps = user_apps_by_size(sample());
        let enriched: HashSet<String> = apps.iter().map(|a| a.bundle_id.clone()).collect();
        assert!(unenriched_apps(&apps, &enriched).is_empty());
    }

    #[test]
    fn render_app_list_shows_total_and_each_row() {
        let apps = user_apps_by_size(sample());
        let out = render_app_list(&apps);
        assert!(out.contains("Big"));
        assert!(out.contains("Small"));
        assert!(!out.contains("System"));
        assert!(out.contains("com.user.big"));
        assert!(out.contains("500.0 MB"));
        assert!(out.contains("50.0 MB"));
        assert!(out.contains("2 apps"));
        assert!(out.contains("550.0 MB"));
    }

    #[test]
    fn render_app_list_handles_empty() {
        let out = render_app_list(&[]);
        assert!(out.contains("No user apps"));
    }

    #[test]
    fn parse_confirmation_accepts_y_yes_case_insensitive() {
        assert!(parse_confirmation("y"));
        assert!(parse_confirmation("Y"));
        assert!(parse_confirmation("yes"));
        assert!(parse_confirmation("YES"));
        assert!(parse_confirmation("  yes  \n"));
    }

    #[test]
    fn parse_confirmation_rejects_everything_else() {
        assert!(!parse_confirmation(""));
        assert!(!parse_confirmation("n"));
        assert!(!parse_confirmation("no"));
        assert!(!parse_confirmation("sure"));
        assert!(!parse_confirmation("\n"));
    }

    fn dummy_app() -> App {
        App {
            bundle_id: "com.x".into(),
            name: "X".into(),
            size_bytes: 1,
            is_system: false,
        }
    }

    #[test]
    fn confirm_uninstall_skips_prompt_with_assume_yes() {
        let mut input: Cursor<&[u8]> = Cursor::new(b"");
        let mut out: Vec<u8> = Vec::new();
        confirm_uninstall(&dummy_app(), true, false, &mut input, &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn confirm_uninstall_aborts_when_non_interactive_without_yes() {
        let mut input: Cursor<&[u8]> = Cursor::new(b"");
        let mut out: Vec<u8> = Vec::new();
        let err = confirm_uninstall(&dummy_app(), false, false, &mut input, &mut out).unwrap_err();
        assert!(err.to_string().contains("--yes"));
        assert!(out.is_empty());
    }

    #[test]
    fn confirm_uninstall_proceeds_when_user_types_y() {
        let mut input: Cursor<&[u8]> = Cursor::new(b"y\n");
        let mut out: Vec<u8> = Vec::new();
        confirm_uninstall(&dummy_app(), false, true, &mut input, &mut out).unwrap();
        let prompt = String::from_utf8(out).unwrap();
        assert!(prompt.contains("About to uninstall"));
        assert!(prompt.contains("permanently lost"));
    }

    #[test]
    fn confirm_uninstall_aborts_when_user_does_not_confirm() {
        let mut input: Cursor<&[u8]> = Cursor::new(b"n\n");
        let mut out: Vec<u8> = Vec::new();
        let err = confirm_uninstall(&dummy_app(), false, true, &mut input, &mut out).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("abort"));
    }
}
