//! Pure `fn render_svg(&CardData) -> String`.
//!
//! No `SystemTime::now`, no IO. Every time-dependent value enters via
//! [`super::data::CardData`] as a pre-formatted string. Same input → same
//! output, byte-for-byte.

use std::borrow::Cow;
use std::fmt::Write;

use super::badges::{Badge, BadgeColor};
use super::data::{
    AppsJailbreakLabel, CardData, HealthTier, StorageBreakdownRows, StorageFallback, TopApp,
};

// ---- canvas geometry ----
//
// All spacing derives from `SPACE = 8`. Layout choices in this file should
// pick one of the named constants below; raw pixel literals are a smell
// (they break the rhythm and ship visual noise to the user).
//
// The card BG fills the canvas edge-to-edge; `PAD` is the inner margin
// around the content. All four sides use the same value so the frame
// breathes evenly.
const CANVAS: i32 = 1080;

/// Base unit. Every vertical / horizontal gap in the card is a multiple
/// of this. 4px gives the rhythm enough granularity to absorb the 22px
/// content type (which wants ~28px line-height) without breaking the
/// "everything snaps to a grid" property.
const SPACE: i32 = 4;

/// Outer pad on all four sides of the card content.
const PAD: i32 = 14 * SPACE; // 56

/// Space above AND below every divider line — same on both sides so the
/// horizontal rule sits in a symmetric pocket of whitespace.
const DIVIDER_GAP: i32 = 8 * SPACE; // 32

/// Section label baseline → first content baseline.
const LABEL_TO_CONTENT: i32 = 7 * SPACE; // 28

/// Intra-section vertical rhythm for 22px content lines.
const ROW_GAP: i32 = 7 * SPACE; // 28

/// Gap between the two columns of the [battery+storage] | [top apps] block.
const COLUMN_GAP: i32 = 10 * SPACE; // 40

/// Chrome dots bottom → header content top.
const CHROME_TO_HEADER: i32 = 6 * SPACE; // 24

/// Header model name → chip / storage / color sub line.
const HEADER_TITLE_GAP: i32 = 11 * SPACE; // 44

/// Header sub line → vibe tagline.
const HEADER_SUB_GAP: i32 = 7 * SPACE; // 28

/// Distance from a label-column anchor (e.g. "os", "apps") to where the
/// value text begins. Wide enough for the longest 6-char label plus air.
const INFO_VALUE_OFFSET: i32 = 36 * SPACE; // 144

/// Storage breakdown row: gap from the label column to the value column.
/// Tighter than `INFO_VALUE_OFFSET` because the labels here are all short
/// (`photos / apps / other / free`) — no need for the same generous room
/// the info table needs for `paired / backup`.
const STORAGE_VALUE_OFFSET: i32 = 32 * SPACE; // 128

/// Storage breakdown row: gap from the label column to the mini-bar. The
/// bar sits past the value column with enough air to read "label · value
/// · bar" as a single line.
const STORAGE_BAR_OFFSET: i32 = 60 * SPACE; // 240

/// Earned-row badge dimensions.
const BADGE_H: i32 = 16 * SPACE; // 64
const BADGE_GAP: i32 = 6 * SPACE; // 24

/// Transparent inset around the rounded card body. Picked so the macOS-style
/// window edge is visible without eating into the chrome dots' breathing room.
const WINDOW_MARGIN: i32 = 3 * SPACE; // 12
/// Corner radius of the card body. Matches the radius of recent macOS app
/// windows closely enough to read as "a window screenshot".
const WINDOW_RADIUS: i32 = 14;

const fn content_left() -> i32 {
    PAD
}
const fn content_right() -> i32 {
    CANVAS - PAD
}

// ---- palette ----
const BG: &str = "#1A1916";
const BORDER: &str = "#2C2C2A";
const TEXT_PRIMARY: &str = "#FAF9F5";
const TEXT_SECONDARY: &str = "#888780";

const ACCENT_GOOD: &str = "#1D9E75";
const ACCENT_GOOD_LIGHT: &str = "#C0DD97";
const ACCENT_GOOD_DIM: &str = "#97C459";
const ACCENT_WARN: &str = "#EF9F27";
const ACCENT_WARN_LIGHT: &str = "#FAC775";
const ACCENT_INFO: &str = "#B5D4F4";
const ACCENT_INFO_DIM: &str = "#85B7EB";
const ACCENT_BAD: &str = "#E24B4A";

// Chrome dots
const CHROME_RED: &str = "#ED6A5E";
const CHROME_AMBER: &str = "#F5BF4F";
const CHROME_GREEN: &str = "#62C554";

const FONT_FAMILY: &str = "JetBrains Mono";

const STORAGE_BAR_CELLS: u8 = 11;

/// Top-level renderer. Returns a complete `<svg>` document.
pub fn render_svg(card: &CardData) -> String {
    let mut svg = String::with_capacity(8 * 1024);
    writeln!(
        &mut svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{CANVAS}" height="{CANVAS}" viewBox="0 0 {CANVAS} {CANVAS}">"#
    )
    .unwrap();

    // Card body sits inside a small transparent margin so the rounded
    // corners read as a macOS-style window when the PNG is shared on any
    // backdrop. 1px stroke gives the window edge just enough definition
    // without competing with the chrome dots.
    let frame = CANVAS - 2 * WINDOW_MARGIN;
    writeln!(
        &mut svg,
        r#"<rect x="{WINDOW_MARGIN}" y="{WINDOW_MARGIN}" width="{frame}" height="{frame}" rx="{WINDOW_RADIUS}" ry="{WINDOW_RADIUS}" fill="{BG}" stroke="{BORDER}" stroke-width="1"/>"#
    )
    .unwrap();

    let mut y = PAD;
    y = render_chrome(&mut svg, y);
    y = render_header(&mut svg, card, y + CHROME_TO_HEADER);
    y = render_divider(&mut svg, y + DIVIDER_GAP);

    // ---- two-column block: [battery + storage]  |  [top apps] ----
    //
    // The split puts the two compact stat sections in a left column
    // and TOP APPS in a right column at the same vertical band. Saves
    // a lot of vertical real estate that the EARNED row needs below.
    let block_start_y = y + DIVIDER_GAP;
    let col_a_left = content_left();
    let col_a_right = (content_left() + content_right()) / 2 - COLUMN_GAP / 2;
    let col_b_left = (content_left() + content_right()) / 2 + COLUMN_GAP / 2;
    let col_b_right = content_right();

    // Left column
    let mut left_y = render_battery_section(&mut svg, card, block_start_y, col_a_left);
    left_y = render_divider_range(&mut svg, left_y + DIVIDER_GAP, col_a_left, col_a_right);
    left_y = render_storage_section(&mut svg, card, left_y + DIVIDER_GAP, col_a_left);

    // Right column
    let right_y = if card.top_apps.is_some() {
        render_top_apps_section(&mut svg, card, block_start_y, col_b_left, col_b_right)
    } else {
        block_start_y
    };

    y = left_y.max(right_y);
    // ---- back to full-width sections ----
    y = render_divider(&mut svg, y + DIVIDER_GAP);
    y = render_info_table(&mut svg, card, y + DIVIDER_GAP);
    y = render_divider(&mut svg, y + DIVIDER_GAP);
    let _ = render_earned(&mut svg, card, y + DIVIDER_GAP);

    // Footer always pinned to the bottom of the card.
    render_footer(&mut svg, card);

    svg.push_str("</svg>");
    svg
}

// ----------------------------------------------------------------------------
// Sections
// ----------------------------------------------------------------------------

/// Three macOS chrome dots in the top-left of the card. Returns the y
/// baseline of the row.
fn render_chrome(svg: &mut String, y: i32) -> i32 {
    let dot_r = 9;
    let dot_y = y;
    let dot_xs = [content_left() + 8, content_left() + 34, content_left() + 60];
    let dot_colors = [CHROME_RED, CHROME_AMBER, CHROME_GREEN];
    for (cx, fill) in dot_xs.iter().zip(dot_colors.iter()) {
        writeln!(
            svg,
            r#"<circle cx="{cx}" cy="{dot_y}" r="{dot_r}" fill="{fill}"/>"#
        )
        .unwrap();
    }
    dot_y + dot_r
}

// Card mascot — pixel-art quokka, embedded as a base64 PNG data URI so the
// generated SVG stays self-contained (no external file references when the
// rasteriser ingests it).
const MASCOT_PNG: &[u8] = include_bytes!("../../../assets/quokka.png");

/// Edge of the square box the mascot is rendered into. Matches the vertical
/// band the old 7-line ASCII art reserved (7 × 28 = 196), so the surrounding
/// layout constants don't need to move.
const MASCOT_BOX: i32 = 196;

/// Lazy base64-encoded `data:image/png;base64,…` URI. Encoded once on first
/// render and reused for every subsequent call.
fn mascot_data_uri() -> &'static str {
    use base64::Engine as _;
    use std::sync::LazyLock;
    static URI: LazyLock<String> = LazyLock::new(|| {
        let b64 = base64::engine::general_purpose::STANDARD.encode(MASCOT_PNG);
        format!("data:image/png;base64,{b64}")
    });
    URI.as_str()
}

fn render_header(svg: &mut String, card: &CardData, y: i32) -> i32 {
    let mascot_x = content_left();
    let mascot_y = y;
    // Pixel-art mascot — `image-rendering: pixelated` keeps the chunky pixels
    // crisp instead of letting resvg smooth them into mush at our render size.
    writeln!(
        svg,
        r#"<image x="{mascot_x}" y="{mascot_y}" width="{MASCOT_BOX}" height="{MASCOT_BOX}" preserveAspectRatio="xMidYMid meet" style="image-rendering:pixelated" href="{uri}"/>"#,
        uri = mascot_data_uri(),
    )
    .unwrap();

    let identity_x = mascot_x + MASCOT_BOX + 24;
    let mut identity_y = y + 50;

    if let Some(model) = card.model_friendly.as_deref() {
        text(
            svg,
            identity_x,
            identity_y,
            38,
            Weight::Medium,
            TEXT_PRIMARY,
            "start",
            model,
        );
        identity_y += HEADER_TITLE_GAP;
    }

    let sub = header_sub_line(card);
    if !sub.is_empty() {
        // Bumped from TEXT_SECONDARY — at 22px under a 38px primary line
        // it needs to read as a "subtitle", not a tooltip. Regular weight
        // keeps it from competing with the model name above.
        text(
            svg,
            identity_x,
            identity_y,
            22,
            Weight::Regular,
            TEXT_PRIMARY,
            "start",
            &sub,
        );
        identity_y += HEADER_SUB_GAP;
    }

    // Caption resolution (vibe vs identity, fallback chain) is handled in
    // `data::resolve_header_caption` so the rule is testable as a pure
    // function and the renderer stays a dumb formatter.
    if let Some(line) = card.header_caption.as_deref() {
        text(
            svg,
            identity_x,
            identity_y,
            22,
            Weight::Regular,
            ACCENT_GOOD,
            "start",
            &format!("> {line}"),
        );
    }

    // Header block reserved height. Anchored to the mascot box so the row
    // is always at least tall enough for the image without overlapping the
    // next section, regardless of how many identity lines the card has.
    y + MASCOT_BOX + 16
}

fn header_sub_line(card: &CardData) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = card.chip_name.as_deref() {
        parts.push(c.to_string());
    }
    if let Some(s) = card.storage_label.as_deref() {
        parts.push(s.to_string());
    }
    if let Some(c) = card.enclosure_color.as_deref() {
        parts.push(c.to_string());
    }
    parts.join(" · ")
}

/// Width of the battery / storage bars in a half-column layout. Picked so
/// 28 cells × ~13.2px (JetBrains Mono 22px advance) ≈ 370px which fits in
/// the ~450px column comfortably.
const COL_BAR_CELLS: u8 = 28;

fn render_battery_section(svg: &mut String, card: &CardData, y: i32, x: i32) -> i32 {
    section_label_at(svg, y, "BATTERY", x);
    let line_y = y + LABEL_TO_CONTENT;
    let line = format!(
        "{level} · {cycles} · {tier}",
        level = card
            .battery_level_percent
            .map(|l| format!("{l}%"))
            .unwrap_or_else(|| "—".to_string()),
        cycles = card
            .battery_cycle_count
            .map(|c| if c == 1 {
                "1 cycle".to_string()
            } else {
                format!("{c} cycles")
            })
            .unwrap_or_else(|| "— cycles".to_string()),
        tier = match card.battery_health_tier {
            HealthTier::Good => "healthy",
            HealthTier::Warn => "fair",
            HealthTier::Bad => "needs service",
            HealthTier::Unknown => "—",
        },
    );
    text(
        svg,
        x,
        line_y,
        22,
        Weight::Regular,
        TEXT_PRIMARY,
        "start",
        &line,
    );

    let bar_y = line_y + ROW_GAP;
    let percent = card.battery_health_percent.unwrap_or(0).min(100);
    let filled = ((percent as u32 * COL_BAR_CELLS as u32) / 100).min(COL_BAR_CELLS as u32) as u8;
    let bar_color = match card.battery_health_tier {
        HealthTier::Good => ACCENT_GOOD,
        HealthTier::Warn => ACCENT_WARN,
        HealthTier::Bad => ACCENT_BAD,
        HealthTier::Unknown => TEXT_SECONDARY,
    };
    bar_text(svg, x, bar_y, filled, COL_BAR_CELLS, bar_color);

    bar_y
}

fn render_storage_section(svg: &mut String, card: &CardData, y: i32, x: i32) -> i32 {
    section_label_at(svg, y, "STORAGE", x);
    let line_y = y + LABEL_TO_CONTENT;
    if let Some(rows) = card.storage_breakdown_rows.as_ref() {
        // Half-column breakdown — fits in the narrower width because
        // the value column and mini-bar are repositioned.
        render_breakdown_rows_at(svg, rows, line_y, x)
    } else if let Some(fb) = card.storage_fallback.as_ref() {
        render_storage_fallback_at(svg, fb, line_y, x)
    } else {
        text(
            svg,
            x,
            line_y,
            22,
            Weight::Regular,
            TEXT_SECONDARY,
            "start",
            "—",
        );
        line_y
    }
}

/// Half-column breakdown rows. Same visual grammar as the full-width
/// version but with tighter `value_x` / `bar_x` offsets.
fn render_breakdown_rows_at(
    svg: &mut String,
    rows: &StorageBreakdownRows,
    mut y: i32,
    x: i32,
) -> i32 {
    let label_x = x;
    let value_x = x + STORAGE_VALUE_OFFSET;
    let bar_x = x + STORAGE_BAR_OFFSET;
    let line_spacing = ROW_GAP;
    let categories: [(&str, &str, u8, &str); 3] = [
        (
            "photos",
            rows.camera_label.as_str(),
            rows.camera_cells,
            ACCENT_GOOD,
        ),
        (
            "apps",
            rows.apps_label.as_str(),
            rows.apps_cells,
            ACCENT_INFO,
        ),
        (
            "other",
            rows.other_label.as_str(),
            rows.other_cells,
            TEXT_SECONDARY,
        ),
    ];
    for (label, value, cells, color) in categories.iter() {
        text(
            svg,
            label_x,
            y,
            22,
            Weight::Regular,
            TEXT_SECONDARY,
            "start",
            label,
        );
        text(
            svg,
            value_x,
            y,
            22,
            Weight::Regular,
            TEXT_PRIMARY,
            "start",
            value,
        );
        bar_text(svg, bar_x, y, *cells, STORAGE_BAR_CELLS, color);
        y += line_spacing;
    }
    text(
        svg,
        label_x,
        y,
        22,
        Weight::Regular,
        TEXT_SECONDARY,
        "start",
        "free",
    );
    text(
        svg,
        value_x,
        y,
        22,
        Weight::Regular,
        TEXT_PRIMARY,
        "start",
        &rows.free_label,
    );
    y + line_spacing
}

/// Half-column fallback when iOS didn't return per-category storage.
/// Three lines: "used of total", "free", bar — fits the narrow column.
fn render_storage_fallback_at(svg: &mut String, fb: &StorageFallback, y: i32, x: i32) -> i32 {
    let line1 = format!("{} of {}", fb.used_label, fb.total_label);
    let line2 = format!("{} free", fb.free_label);
    text(
        svg,
        x,
        y,
        22,
        Weight::Regular,
        TEXT_PRIMARY,
        "start",
        &line1,
    );
    let line2_y = y + ROW_GAP;
    text(
        svg,
        x,
        line2_y,
        22,
        Weight::Regular,
        TEXT_SECONDARY,
        "start",
        &line2,
    );
    let bar_y = line2_y + ROW_GAP;
    let filled =
        ((fb.used_percent as u32 * COL_BAR_CELLS as u32) / 100).min(COL_BAR_CELLS as u32) as u8;
    bar_text(svg, x, bar_y, filled, COL_BAR_CELLS, ACCENT_WARN);
    bar_y
}

// Old full-width breakdown / fallback renderers removed — the
// two-column layout uses `render_breakdown_rows_at` /
// `render_storage_fallback_at`.

/// TOP APPS — 5 rows with `name · size · mini-bar` each, scaled
/// relative to the heaviest. Positioned by `x` and bounded by `right_x`
/// so it fits the right column of the two-column block.
fn render_top_apps_section(svg: &mut String, card: &CardData, y: i32, x: i32, right_x: i32) -> i32 {
    let apps = match card.top_apps.as_ref() {
        Some(a) if !a.is_empty() => a,
        _ => return y,
    };
    section_label_at(svg, y, "TOP APPS", x);
    let mut row_y = y + LABEL_TO_CONTENT;
    let line_spacing = ROW_GAP;
    // 3 columns: name (left), value (right of name), bar (snapped to
    // right_x so it shares the right edge with the card's other bars).
    let name_x = x;
    let bar_x = right_x - (STORAGE_BAR_CELLS as i32) * 13;
    let value_x = bar_x - (30 * SPACE);
    for app in apps {
        render_top_app_row(svg, app, name_x, value_x, bar_x, row_y);
        row_y += line_spacing;
    }
    row_y
}

fn render_top_app_row(
    svg: &mut String,
    app: &TopApp,
    name_x: i32,
    value_x: i32,
    bar_x: i32,
    y: i32,
) {
    text(
        svg,
        name_x,
        y,
        22,
        Weight::Regular,
        TEXT_PRIMARY,
        "start",
        &app.display_name,
    );
    text(
        svg,
        value_x,
        y,
        22,
        Weight::Regular,
        TEXT_SECONDARY,
        "start",
        &app.size_label,
    );
    bar_text(svg, bar_x, y, app.bar_cells, 11, ACCENT_INFO);
}

fn render_info_table(svg: &mut String, card: &CardData, y: i32) -> i32 {
    let label_x = content_left();
    let value_x = content_left() + INFO_VALUE_OFFSET;
    let mut row_y = y;
    let line_spacing = ROW_GAP;

    // os
    let os = match card.ios_beta_suffix {
        Some(suffix) => format!("{}{suffix}", card.ios_label),
        None => card.ios_label.clone(),
    };
    text(
        svg,
        label_x,
        row_y,
        22,
        Weight::Regular,
        TEXT_SECONDARY,
        "start",
        "os",
    );
    info_value(
        svg,
        value_x,
        row_y,
        &os,
        card.ios_beta_suffix.map(|_| ACCENT_INFO),
    );
    row_y += line_spacing;

    // apps
    text(
        svg,
        label_x,
        row_y,
        22,
        Weight::Regular,
        TEXT_SECONDARY,
        "start",
        "apps",
    );
    let (apps_value, apps_tail_color) = match card.apps_jailbreak_label {
        AppsJailbreakLabel::Pristine => (
            format!(
                "{} installed · no jailbreak",
                card.app_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".into())
            ),
            Some(ACCENT_GOOD),
        ),
        AppsJailbreakLabel::Jailbroken => (
            format!(
                "{} installed · jailbroken",
                card.app_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".into())
            ),
            Some(ACCENT_BAD),
        ),
        AppsJailbreakLabel::None => ("— installed".into(), None),
    };
    info_value(svg, value_x, row_y, &apps_value, apps_tail_color);
    row_y += line_spacing;

    if let Some(line) = card.first_seen_line.as_deref() {
        text(
            svg,
            label_x,
            row_y,
            22,
            Weight::Regular,
            TEXT_SECONDARY,
            "start",
            "paired",
        );
        text(
            svg,
            value_x,
            row_y,
            22,
            Weight::Regular,
            TEXT_PRIMARY,
            "start",
            line,
        );
        row_y += line_spacing;
    }

    if let Some(age) = card.backup_age_label.as_deref() {
        text(
            svg,
            label_x,
            row_y,
            22,
            Weight::Regular,
            TEXT_SECONDARY,
            "start",
            "backup",
        );
        text(
            svg,
            value_x,
            row_y,
            22,
            Weight::Regular,
            TEXT_PRIMARY,
            "start",
            age,
        );
        row_y += line_spacing;
    }

    row_y
}

/// Render the value column for an info-table row. If `tail_accent` is
/// `Some`, the trailing `· …` segment is recoloured in the accent. Uses
/// an inline `<tspan>` so resvg measures glyph advances itself — no
/// hand-rolled px offset that can drift with font metrics.
fn info_value(svg: &mut String, x: i32, y: i32, value: &str, tail_accent: Option<&str>) {
    match tail_accent.and_then(|color| value.rfind(" · ").map(|i| (i, color))) {
        Some((i, color)) => {
            let head = xml_escape(&value[..i]);
            let tail = xml_escape(&value[i..]);
            // `xml:space="preserve"` keeps the leading space of `tail`
            // (" · …") — without it SVG collapses inter-tspan whitespace
            // and the `·` ends up flush against the previous word.
            writeln!(
                svg,
                r#"<text x="{x}" y="{y}" font-family="{FONT_FAMILY}" font-size="22" font-weight="400" fill="{TEXT_PRIMARY}" text-anchor="start" xml:space="preserve">{head}<tspan fill="{color}">{tail}</tspan></text>"#,
            )
            .unwrap();
        }
        None => {
            text(svg, x, y, 22, Weight::Regular, TEXT_PRIMARY, "start", value);
        }
    }
}

fn render_earned(svg: &mut String, card: &CardData, y: i32) -> i32 {
    section_label(svg, y, "EARNED");
    let badge_y = y + LABEL_TO_CONTENT;
    if card.badges.is_empty() {
        text(
            svg,
            content_left(),
            badge_y + (3 * SPACE),
            22,
            Weight::Regular,
            TEXT_SECONDARY,
            "start",
            "— no badges yet —",
        );
        return badge_y + BADGE_H;
    }
    // Divide the content width evenly between three badges with one gap
    // between each — keeps the row symmetric inside the card's margins
    // regardless of how many fit.
    let total_w = content_right() - content_left();
    let badge_w = (total_w - 2 * BADGE_GAP) / 3;
    // Left-align with the rest of the card content. Previously the badges
    // were centred which left a visible gap on the left that didn't match
    // the BATTERY / STORAGE / EARNED label edge.
    let mut bx = content_left();
    for badge in &card.badges {
        render_badge(svg, bx, badge_y, badge_w, BADGE_H, badge);
        bx += badge_w + BADGE_GAP;
    }
    let mut y_after = badge_y + BADGE_H;
    // Pull-back hook: one-line teaser pointing at the closest reachable badge.
    // Skipped when the projector found nothing within nudging distance — no
    // empty slot, no "Earn more badges!" filler.
    if let Some(hint) = card.next_badge_hint.as_deref() {
        let hint_y = y_after + (5 * SPACE);
        text(
            svg,
            content_left(),
            hint_y,
            18,
            Weight::Regular,
            TEXT_SECONDARY,
            "start",
            hint,
        );
        y_after = hint_y + (4 * SPACE);
    }
    y_after
}

fn render_badge(svg: &mut String, x: i32, y: i32, w: i32, h: i32, badge: &Badge) {
    let (fill, stroke, title_color, sub_color) = match badge.color {
        BadgeColor::Good => (
            "rgba(29,158,117,0.10)",
            "rgba(29,158,117,0.45)",
            ACCENT_GOOD_LIGHT,
            ACCENT_GOOD_DIM,
        ),
        BadgeColor::Warn => (
            "rgba(239,159,39,0.10)",
            "rgba(239,159,39,0.45)",
            ACCENT_WARN_LIGHT,
            ACCENT_WARN,
        ),
        BadgeColor::Info => (
            "rgba(181,212,244,0.07)",
            "rgba(181,212,244,0.30)",
            ACCENT_INFO,
            ACCENT_INFO_DIM,
        ),
        BadgeColor::Bad => (
            "rgba(226,75,74,0.10)",
            "rgba(226,75,74,0.45)",
            "#F2A6A5",
            ACCENT_BAD,
        ),
    };
    writeln!(
        svg,
        r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="6" fill="{fill}" stroke="{stroke}" stroke-width="0.5"/>"#
    )
    .unwrap();
    // Emoji icon (Twemoji SVG, 32×32) inside a 14px left padding. The
    // title and subtitle shift right to make room.
    let emoji_size = 32;
    let emoji_x = x + 14;
    let emoji_y = y + (h - emoji_size as i32) / 2;
    svg.push_str(&super::emoji::render(
        badge.id, emoji_x, emoji_y, emoji_size,
    ));
    let text_x = emoji_x + emoji_size as i32 + 10;
    // Title at 20px (down from 22) gives the 64px badge breathing room
    // without enlarging the row. Baselines at 27 / 47 balance the
    // top/bottom padding at ~13-14px each.
    text(
        svg,
        text_x,
        y + 27,
        20,
        Weight::Medium,
        title_color,
        "start",
        badge.title,
    );
    text(
        svg,
        text_x,
        y + 47,
        13,
        Weight::Regular,
        sub_color,
        "start",
        badge.subtitle,
    );
}

fn render_footer(svg: &mut String, card: &CardData) {
    // Pinned to the bottom of the card. Single line, 22px so it's readable
    // as a "terminal prompt" without dominating the layout.
    // Baseline sits exactly PAD below the canvas bottom: the bottom of
    // the x-height aligns with the inner pad.
    let footer_y = CANVAS - PAD;
    let footer = format!("$ qk card · {} · {}", card.footer_cta, card.footer_date);
    text(
        svg,
        content_left(),
        footer_y,
        22,
        Weight::Regular,
        TEXT_SECONDARY,
        "start",
        &footer,
    );
}

// ----------------------------------------------------------------------------
// Primitives
// ----------------------------------------------------------------------------

fn render_divider(svg: &mut String, y: i32) -> i32 {
    render_divider_range(svg, y, content_left(), content_right())
}

fn render_divider_range(svg: &mut String, y: i32, x1: i32, x2: i32) -> i32 {
    writeln!(
        svg,
        r#"<line x1="{x1}" y1="{y}" x2="{x2}" y2="{y}" stroke="{BORDER}" stroke-width="1"/>"#
    )
    .unwrap();
    y
}

fn section_label_at(svg: &mut String, y: i32, label: &str, x: i32) {
    // Bumped from TEXT_TERTIARY (#5F5E5A) — that tone reads as ~AA-level
    // contrast on the dark card and looked like a print defect rather than
    // a label. TEXT_SECONDARY is the labels-in-tables tone elsewhere.
    text(
        svg,
        x,
        y,
        13,
        Weight::Medium,
        TEXT_SECONDARY,
        "start",
        label,
    );
}

fn section_label(svg: &mut String, y: i32, label: &str) {
    // Bumped from TEXT_TERTIARY (#5F5E5A) — that tone reads as ~AA-level
    // contrast on the dark card and looked like a print defect rather than
    // a label. TEXT_SECONDARY is the labels-in-tables tone elsewhere.
    text(
        svg,
        content_left(),
        y,
        13,
        Weight::Medium,
        TEXT_SECONDARY,
        "start",
        label,
    );
}

#[derive(Clone, Copy)]
enum Weight {
    Regular,
    Medium,
}

impl Weight {
    fn css(self) -> &'static str {
        match self {
            Weight::Regular => "400",
            Weight::Medium => "500",
        }
    }
}

#[allow(clippy::too_many_arguments)] // 8 args is the minimum the helper needs; bundling them into a struct would obscure call sites.
fn text(
    svg: &mut String,
    x: i32,
    y: i32,
    size: u32,
    weight: Weight,
    fill: &str,
    anchor: &str,
    body: &str,
) {
    writeln!(
        svg,
        r#"<text x="{x}" y="{y}" font-family="{FONT_FAMILY}" font-size="{size}" font-weight="{w}" fill="{fill}" text-anchor="{anchor}">{body}</text>"#,
        w = weight.css(),
        body = xml_escape(body),
    )
    .unwrap();
}

/// Build a `[█████░░░░░░]`-style monospace bar as a single `<text>` with
/// inline `<tspan>`s. Filled portion uses `color`; empty portion uses
/// `BORDER` (very subtle, matches the card edge). Letting resvg measure
/// the advances is more reliable than a hand-tuned px offset.
fn bar_text(svg: &mut String, x: i32, y: i32, filled: u8, total: u8, color: &str) {
    let mut filled_body = String::with_capacity(filled as usize * 3);
    for _ in 0..filled {
        filled_body.push('█');
    }
    let empty_count = total.saturating_sub(filled);
    let mut empty_body = String::with_capacity(empty_count as usize * 3);
    for _ in 0..empty_count {
        empty_body.push('░');
    }
    writeln!(
        svg,
        r#"<text x="{x}" y="{y}" font-family="{FONT_FAMILY}" font-size="22" font-weight="400" text-anchor="start"><tspan fill="{color}">{filled_body}</tspan><tspan fill="{BORDER}">{empty_body}</tspan></text>"#,
    )
    .unwrap();
}

/// Escape the five XML-significant characters. Returns `Cow::Borrowed` when
/// no escaping is needed so the hot path is allocation-free.
pub fn xml_escape(s: &str) -> Cow<'_, str> {
    if !s
        .as_bytes()
        .iter()
        .any(|&b| matches!(b, b'<' | b'>' | b'&' | b'"' | b'\''))
    {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::card::badges::{Badge, BadgeColor, BadgeId};
    use crate::commands::card::data::{
        AppsJailbreakLabel, CardData, HealthTier, StorageBreakdownRows,
    };

    fn richtest_card() -> CardData {
        CardData {
            model_friendly: Some("iPhone 14 Pro Max".into()),
            chip_name: Some("A16 Bionic".into()),
            storage_label: Some("256 GB".into()),
            enclosure_color: Some("Deep Purple".into()),
            header_caption: Some("Spotify since 2022 · 47 apps · iOS 18".into()),
            battery_level_percent: Some(91),
            battery_cycle_count: Some(142),
            battery_health_percent: Some(91),
            battery_health_tier: HealthTier::Good,
            storage_breakdown_rows: Some(StorageBreakdownRows {
                camera_label: "84.0G".into(),
                apps_label: "18.7G".into(),
                other_label: "4.8G".into(),
                free_label: "148.5G".into(),
                camera_cells: 7,
                apps_cells: 2,
                other_cells: 1,
            }),
            storage_fallback: None,
            ios_label: "iOS 18.2 (22C152)".into(),
            ios_beta_suffix: None,
            app_count: Some(47),
            apps_jailbreak_label: AppsJailbreakLabel::Pristine,
            first_seen_line: Some("Mar 2022 · Spotify is your oldest".into()),
            backup_age_label: Some("12 days ago".into()),
            badges: vec![
                Badge {
                    id: BadgeId::BatteryChamp,
                    title: "Battery Champ",
                    subtitle: "90%+ after 3+ years",
                    color: BadgeColor::Good,
                },
                Badge {
                    id: BadgeId::Veteran,
                    title: "Veteran",
                    subtitle: "3+ years in service",
                    color: BadgeColor::Warn,
                },
                Badge {
                    id: BadgeId::ProMaxClub,
                    title: "Pro Max Club",
                    subtitle: "top-tier model",
                    color: BadgeColor::Info,
                },
            ],
            next_badge_hint: None,
            top_apps: None,
            footer_date: "May 27".into(),
            footer_cta: "star us: github.com/dutradotdev/quokka",
            redact: false,
        }
    }

    #[test]
    fn render_is_deterministic_for_identical_input() {
        let card = richtest_card();
        let a = render_svg(&card);
        let b = render_svg(&card);
        assert_eq!(a, b);
    }

    #[test]
    fn render_emits_expected_structural_anchors() {
        let svg = render_svg(&richtest_card());
        // High-level invariants — every regression should trip at least one.
        for needle in [
            "1080",           // canvas
            "JetBrains Mono", // font registered
            "BATTERY",
            "STORAGE",
            "EARNED",
            "iPhone 14 Pro Max",
            "A16 Bionic",
            "256 GB",
            "Deep Purple",
            "Spotify since 2022 · 47 apps · iOS 18",
            "Spotify is your oldest",
            "Battery Champ",
            "github.com/dutradotdev/quokka",
            "May 27",
        ] {
            assert!(svg.contains(needle), "rendered SVG missing `{needle}`");
        }
    }

    #[test]
    fn xml_escape_handles_five_xml_chars() {
        assert_eq!(xml_escape("plain"), "plain");
        assert_eq!(xml_escape("a<b").as_ref(), "a&lt;b");
        assert_eq!(xml_escape("a>b").as_ref(), "a&gt;b");
        assert_eq!(xml_escape("a&b").as_ref(), "a&amp;b");
        assert_eq!(xml_escape("a\"b").as_ref(), "a&quot;b");
        assert_eq!(xml_escape("a'b").as_ref(), "a&apos;b");
        assert_eq!(xml_escape("<&>").as_ref(), "&lt;&amp;&gt;");
    }
}
