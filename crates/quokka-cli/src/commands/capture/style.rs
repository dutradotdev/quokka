//! Single palette for the capture TUI. Every coloured span across the
//! draw functions pulls from here, so a theme change is one file.
//!
//! Choices intentionally match the rest of the project (cyan for
//! interactive hints, yellow for warnings, green/red for direction
//! arrows) — see `commands/apps.rs` for the parallel palette in the
//! app picker.

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};

/// Outbound packet arrow (↑).
pub const ARROW_OUT: Style = Style::new().fg(Color::Green);

/// Inbound packet arrow (↓).
pub const ARROW_IN: Style = Style::new().fg(Color::Red);

/// Top-bar drop counter when non-zero. Stays neutral at zero to keep the
/// happy path quiet.
pub const DROP_WARN: Style = Style::new().fg(Color::Yellow);

/// Header row of the packet table.
pub const TABLE_HEADER: Style = Style::new().add_modifier(Modifier::BOLD);

/// Currently selected packet in the stream view.
pub const ROW_SELECTED: Style = Style::new().bg(Color::DarkGray);

/// Hotkey letters in the footer (`[a]pp`, `[q]uit`, ...).
pub const HOTKEY_LABEL: Style = Style::new().fg(Color::Cyan);

/// The `:` prefix of the inline filter prompt.
pub const PROMPT_PREFIX: Style = Style::new().fg(Color::Yellow);

/// Inline validation error suffix on the prompt row.
pub const PROMPT_ERROR: Style = Style::new().fg(Color::Red);

/// Active filter chip in the filter row.
pub const ACTIVE_FILTER: Style = Style::new().fg(Color::Cyan);

/// The non-active view tab in the top-bar selector.
pub const VIEW_TAB_INACTIVE: Style = Style::new().fg(Color::DarkGray);

/// The active view tab in the top-bar selector.
pub const VIEW_TAB_ACTIVE: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);

/// Detail pane heading ("Selected packet").
pub const DETAIL_HEADING: Style = Style::new().add_modifier(Modifier::BOLD);

/// Soft hint text — used for empty state and the "no packet selected"
/// placeholder in the detail pane.
pub const HINT: Style = Style::new().fg(Color::DarkGray);
