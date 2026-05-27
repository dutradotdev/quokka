# `qk card` — design

**Status:** draft, awaiting approval
**Date:** 2026-05-27
**Owner:** Lucas Dutra

## Summary

A new subcommand `qk card` (alias `quokka card`) that reads the connected iPhone's state and writes a 1080×1080 PNG to disk. The PNG is a "neofetch for iPhone" — a shareable terminal-aesthetic flex card with the project repo URL in the footer. No signing, no upload, no verification: privacy and shareability are the only requirements.

The command reuses existing lockdown / installation_proxy data sources, adds five derived fields to `DeviceStatus` (chip name, storage breakdown by category, oldest installed app, jailbreak flag, beta build flag), renders an SVG from a pure function, rasterizes via `resvg` with an embedded JetBrains Mono font, and saves to `~/Desktop/qk-card-<YYYYMMDD-HHMM>.png` by default.

## Motivation

Every successful post of a `qk card` is an organic recruitment ad for quokka. Developers on Mac are the target audience and the target poster. The card needs to look like a beautifully-framed terminal screenshot — coherent with the existing dashboard aesthetic — and carry enough flex-worthy stats that posting it feels like a brag, not a chore.

## Scope

### In scope

- Subcommand `qk card` with `--output`, `--no-open`, `--redact` flags.
- Render to PNG via in-memory SVG → `resvg` → `tiny-skia`, with JetBrains Mono embedded in the binary.
- Five new derived fields on `DeviceStatus`, all via lockdown services already wired up.
- 15-entry badge catalog, top 3 selected by priority ranking.
- Twitter intent URL printed on success, with `#quokka` hashtag.
- Default `~/Desktop/` output path; opens in Preview unless `--no-open`.
- Snapshot tests on SVG output (golden files), unit tests on badge eligibility and ranking, integration test against `FakeDevice`, e2e test against real device.

### Out of scope (defer to future PRs)

- Multiple visual styles (`--style brutalist|retro`).
- Pokémon / Polaroid variants.
- Signing, verification, hash-based authenticity, QR codes.
- Backend service or upload of any kind.
- Customization beyond the listed flags.
- Uptime / "last reboot" — no cheap lockdown path. Cuts badges `clean_slate` and `iron_uptime` from the original spec sketch.
- Battery temperature on iOS 17+ — only via expensive `ioregistry` call, not worth it for a card.
- `og_app_install` badge — `InstallDate` resets on reinstall/restore, can't ground a "first install" claim. Spec keeps the *descriptive* "oldest app" line but no badge derived from it.

## CLI surface

### Synopsis

```
quokka card [--output <path>] [--no-open] [--redact]
```

- `--output <path>`: write PNG here. Default `~/Desktop/qk-card-<YYYYMMDD-HHMM>.png`.
- `--no-open`: skip opening the PNG in Preview after generation.
- `--redact`: mask anything potentially personal (see Privacy below).
- `--udid` works as the global flag, same as other subcommands.

### Behavior

1. Connect via `device::connect()`.
2. Spinner: `Reading device info…`.
3. Collect `CardData` from `device.status()` plus the new derived fields (single fetch).
4. Compute badge eligibility, sort by priority, take top 3.
5. Render SVG in memory from `CardData` (pure function).
6. Rasterize SVG to PNG (1080×1080) via `resvg`.
7. Write PNG to the output path.
8. Print success block:
   ```
   ✓ saved to /Users/lucasdutra/Desktop/qk-card-20260527-1632.png

   share it:
     https://twitter.com/intent/tweet?text=<encoded>

   suggested text:
     My iPhone, in one image.

     Generated with `qk card` 🐹 #quokka
     github.com/dutradotdev/quokka
   ```
9. On macOS, run `open <path>` unless `--no-open` was passed.

### Error UX

- No device connected: same friendly error as other commands (`DeviceError::NotPaired`, etc.), exit non-zero, no placeholder card.
- Device locked: existing pairing error path applies.
- `resvg` render failure: write the generated SVG to a temp file, print its path, suggest filing an issue with the SVG attached. Exit non-zero.
- Disk write failure: print the OS error verbatim, exit non-zero.

## Visual design

### Layout

Approved mockup is in `.superpowers/brainstorm/<session>/content/card-v1.html` (HTML/CSS rendition at scale 0.667). The 1080×1080 canvas is structured top-to-bottom as five blocks separated by a single 1px divider in `#2C2C2A`:

1. **Window chrome row** — three macOS dots (red `#ED6A5E`, amber `#F5BF4F`, green `#62C554`) on the left; centered title `— qk card —` in `#5F5E5A`.
2. **Header row** — ASCII quokka (left, reused from `dashboard.rs`) + identity stack (right):
   ```
   iPhone 14 Pro Max
   A16 Bionic · 256 GB · Deep Purple
   ▸ 4 yr 2 mo in service
   ```
3. **BATTERY** section (full-width) — label, `91% · 142 cycles · healthy`, full-width unicode bar colored by health tier.
4. **STORAGE** section (full-width) — label, then 3 rows + free:
   ```
   photos    84.0G   [███████░░░░]
   apps      18.7G   [██░░░░░░░░░]
   other      4.8G   [█░░░░░░░░░░]
   free     148.5G
   ```
   Per-row mini-bar is 11 cells wide, each cell tinted by category (photos = `good`, apps = `info`, other = `text secondary`).
5. **Info table** — two-column key/value, fixed labels:
   ```
   os          iOS 18.2 (22C152) · beta        <- "· beta" only when beta build detected
   apps        47 installed · pristine          <- " · jailbroken" if bundle ID match
   first seen  Mar 2022 · Spotify is your oldest
   backup      12 days ago
   ```
   When `oldest_app` is `None` (e.g. iOS returned no `LSInstallDate`), the line collapses to `first seen  Mar 2022`. When `last_backup_unix` is `None`, the `backup` row is omitted entirely.
6. **EARNED** — label and up to 3 centered badges (rounded rect, 160×50px in the 1080 canvas, fill + 0.5px stroke matching the accent family). Below 3 badges, center them.
7. **Footer** — single line, color `#444441`:
   ```
   $ qk card · github.com/dutradotdev/quokka · May 27
   ```

### Canvas

- 1080 × 1080 px.
- Background `#1A1916`. Card body has 40px outer margin, `rx=14`.
- Outer card border `#2C2C2A`, 1px.

### Typography

- **JetBrains Mono** only, two weights: Regular (400) and Medium (500).
- Sizes (px, 1:1 with canvas units): 11, 12, 13, 22, 38.
- Font files bundled via `include_bytes!` from `assets/fonts/JetBrainsMono-Regular.ttf` and `JetBrainsMono-Medium.ttf`. License: OFL 1.1. `OFL.txt` copied alongside; README acknowledgment added.

### Color palette (locked)

```
Background dark:   #1A1916
Card border:       #2C2C2A
Text primary:      #FAF9F5
Text secondary:    #888780
Text tertiary:     #5F5E5A
Text muted:        #444441

Accent good:       #1D9E75
Accent good light: #C0DD97
Accent good dim:   #97C459

Accent warn:       #EF9F27
Accent warn light: #FAC775

Accent info:       #B5D4F4
Accent info dim:   #85B7EB

Accent bad:        #E24B4A
```

### Bars

- Battery: full-width row, `█` filled, `░` empty, 40 cells. Color follows battery health tier.
- Storage mini-bars: 11 cells per row, colored per category.

### Badges

Rounded rect, 160×50 px in canvas units, `rx=6`. Fill is the accent family at ~10% opacity, stroke is the accent family at ~45% opacity. Title row uses an accent-family **light** color (e.g. `#C0DD97` on the green badge); subtitle uses the accent dim (e.g. `#97C459`).

Icon glyphs are **monospace unicode** — not emoji. Chosen to stay coherent with the terminal aesthetic and to dodge bundling a color emoji font.

### Color tier mapping per stat

**Battery health:** `≥ 85%` → good · `70–84%` → warn · `< 70%` → bad
**Storage used:** `< 60%` → good · `60–85%` → warn · `> 85%` → bad

## Badge catalog (v1, 15 entries)

Each badge is a check function `fn(&CardData) -> Option<Badge>`. Run all, sort qualifying badges by priority, take top 3. Lower numbers win.

| Priority | Id | Glyph + title | Subtitle | Condition | Color |
|---|---|---|---|---|---|
| 1 | battery_champ | `▲ Battery Champ` | 90%+ after 3+ years | health ≥ 90 ∧ age ≥ 3y | good |
| 2 | og_owner | `○ OG Owner` | first paired in 2020 or earlier | paired_year ≤ 2020 | warn |
| 3 | survivor | `◆ Survivor` | 4+ years, going strong | age ≥ 4y | warn |
| 4 | veteran | `◇ Veteran` | 3+ years in service | 3y ≤ age < 4y | warn |
| 5 | storage_titan | `▣ Storage Titan` | 1TB device | total ≥ 1000G | info |
| 6 | maxed_out | `■ Maxed Out` | over 90% storage used | used ≥ 90% | bad |
| 7 | heavy_cycle | `↻ Heavy Cycle` | 300+ battery cycles | cycles ≥ 300 | warn |
| 8 | beta_tester | `β Beta Tester` | running iOS beta | build matches beta pattern | info |
| 9 | backup_overdue | `! Backup Overdue` | last backup > 30 days | backup_age_days > 30 | warn |
| 10 | minimalist | `· Minimalist` | fewer than 25 apps | app_count < 25 | info |
| 11 | app_collector | `≡ App Collector` | 150+ apps installed | app_count ≥ 150 | info |
| 12 | pro_max_club | `★ Pro Max Club` | top-tier model | model name contains "Pro Max" | info |
| 13 | tidy_hoarder | `▢ Tidy Hoarder` | under 50% storage used | used < 50% ∧ total ≥ 256G | good |
| 14 | backup_fresh | `✓ Backup Fresh` | backed up this week | backup_age_days ≤ 7 | good |
| 15 | speed_demon | `↑ Speed Demon` | latest iOS major | running latest stable major | info |

`age` = years since `paired_since_unix` (capped at the device's iOS install date if newer). `backup_age_days` from `last_backup_unix`. Latest iOS major is a constant in code, bumped manually on each Apple release.

Rationale for ordering: rarer / harder-to-fake badges win. `beta_tester` is high (priority 8) because beta users amplify dev tools on Twitter. `backup_overdue` is `warn` instead of `bad` so the red doesn't discourage posting.

## Data sources

All values read via lockdown-classic services already wired up. Zero new `idevice` services.

### Reused from `DeviceStatus`

- `model_friendly`, `ios_version`, `ios_build`, `enclosure_color`
- `storage.total_bytes`, `storage.free_bytes`
- `battery.level_percent`, `battery.cycle_count`, `battery.health_percent`
- `app_count`
- `paired_since_unix` → `age_years`, `first_seen` line
- `last_backup_unix` → `backup_age_days`, backup line, `backup_fresh`/`backup_overdue`

### New fields on `DeviceStatus`

```rust
pub chip_name: Option<String>,
pub storage_breakdown: Option<StorageBreakdown>,
pub oldest_app: Option<OldestApp>,
pub jailbreak_detected: bool,
pub is_beta_build: bool,

pub struct StorageBreakdown {
    pub camera_bytes: u64,
    pub apps_bytes: u64,
    pub other_bytes: u64,
}

pub struct OldestApp {
    pub bundle_id: String,
    pub display_name: String,
    pub install_date_unix: i64,
}
```

#### `chip_name`

Read lockdown key `HardwarePlatform` (existing read path in `RealDevice::status()` extends it). Map via new table `src/device/chip_names.rs`:

```rust
pub fn chip_name(platform: &str) -> Option<&'static str> {
    match platform {
        "t8030" => Some("A13 Bionic"),
        "t8101" => Some("A14 Bionic"),
        "t8110" => Some("A15 Bionic"),
        "t8120" => Some("A16 Bionic"),
        "t8130" => Some("A17 Pro"),
        // extend as iPhones ship
        _ => None,
    }
}
```

#### `storage_breakdown`

Read three additional u64 keys from the existing `com.apple.disk_usage` lockdown domain:

- `CameraUsage` → `camera_bytes`
- `MobileApplicationUsage` → `apps_bytes`
- `OtherUsage` → `other_bytes` (if absent, derive: `used - camera - apps`)

Any missing key leaves the whole breakdown `None` and the card falls back to a single full-width storage bar.

#### `oldest_app`

`installation_proxy.browse` already iterates apps. Read the `LSInstallDate` per app, take the user-app with the minimum date. Caveat documented in code: `LSInstallDate` reflects the **current install record**, not "first ever on this device" — it resets on reinstall, restore, and iCloud sync of a new install. The card phrasing is **descriptive** ("Spotify is your oldest"), never **historical** ("Spotify was first"), and there is no badge derived from this signal.

#### `jailbreak_detected`

Match user-app bundle IDs against a small constant list of known jailbreak store/launcher IDs in `src/commands/card/jailbreak.rs`:

```rust
const JAILBREAK_BUNDLE_IDS: &[&str] = &[
    "org.coolstar.SileoStore",
    "xyz.willy.Zebra",
    "org.checkra1n.layout",
    "com.opa334.Dopamine",
    // ...curated short list
];
```

Exact match (no substring) to avoid false positives. Surfaced as a flag on the apps line, not a badge — jailbreaking is a state, not a flex.

#### `is_beta_build`

Regex on `ios_build`: developer / public betas have a final-letter suffix pattern that differs from stable (e.g. `22C152` is stable, `22D5034e` is a beta). Conservative pattern; on `None` build, default to `false`.

## Privacy defaults

**Always omitted** (default mode, no flag needed):

- Device name (e.g. "Lucas's iPhone")
- UDID, serial number, IMEI, MAC addresses
- App list (count only; the oldest-app's display name is shown by default — see `--redact` for the opt-out)
- Carrier, phone number, Apple ID
- Photo / media metadata, location

`--redact` additionally:

- iOS build hidden, only major.minor shown (`iOS 18.2` instead of `iOS 18.2 (22C152)`)
- "First seen" shows year only (`2022`) instead of `Mar 2022`
- Oldest-app name replaced with `—`
- Backup age bucketed: `recent` (≤ 7d), `a few weeks ago` (≤ 30d), `a while ago` (> 30d)

## Architecture

### Crate layout

```
src/commands/card/
  mod.rs       — clap args, dispatch (collect → render → png → write → share → open)
  data.rs      — CardData struct + fn collect_card_data(&dyn Device, now: i64, redact: bool) -> Result<CardData>
  badges.rs    — Badge enum, eligibility functions, priority ranking
  render.rs    — fn render_svg(&CardData) -> String  (pure; no I/O)
  png.rs       — fn svg_to_png(svg: &str) -> Result<Vec<u8>>  (resvg + embedded fontdb)
  share.rs     — fn tweet_intent_url(&CardData) -> String + fn tweet_text(&CardData) -> String
  jailbreak.rs — known jailbreak bundle IDs constant
```

`run()` lives in `mod.rs` and is invoked from `src/lib.rs`'s dispatch alongside `status`, `apps`, `analyze`, `menu`.

### Dispatch wiring

`src/lib.rs` adds a `Card { output, no_open, redact }` variant to the `Command` enum and a match arm:

```rust
Command::Card { output, no_open, redact } => {
    commands::card::run(&*device, now_unix(), CardArgs { output, no_open, redact }).await
}
```

`now_unix()` is the existing helper in `src/ui.rs`. Passing it as a parameter (rather than calling `SystemTime::now()` inside `card::run`) is what makes tests deterministic.

### Determinism

Original spec asked for "byte-identical PNGs across runs". That's unreachable while the footer date and `backup_age_days` depend on wall-clock time. Restated as: **idempotent within the same day given identical device state, with `now` captured once at the top of `run()` and threaded through all dependencies**.

`render_svg` is pure: it consumes a `CardData` that already carries a pre-formatted `footer_date: String` and `backup_age_label: Option<String>` derived upstream from `now`. The renderer never calls `SystemTime::now()` itself.

Snapshot tests compare the rendered SVG **string** (not PNG bytes) against golden files — `resvg` may produce microscopic float differences between minor versions, but the SVG string is fully deterministic.

### Font handling

`resvg` does not consult macOS's font book. Bundle JetBrains Mono via `include_bytes!` and register into `usvg::Options::fontdb`:

```rust
let mut fontdb = fontdb::Database::new();
fontdb.load_font_data(include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf").to_vec());
fontdb.load_font_data(include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf").to_vec());
let opts = usvg::Options { fontdb: Arc::new(fontdb), ..Default::default() };
```

CI must pass on a runner without JetBrains Mono installed — the e2e and snapshot tests confirm this implicitly.

### SVG construction

Build the SVG as a `String` via `write!`/`writeln!` into a `String` buffer. No `svg` or `xml` crate — keeps the binary small. A tiny XML-escape helper (`fn xml_escape(s: &str) -> Cow<str>`) sanitizes the few user-derived strings (model name, oldest-app name).

### Dependencies

Add to `Cargo.toml`:

```toml
resvg = "0.45"
usvg = "0.45"
tiny-skia = "0.11"
fontdb = "0.21"
```

All four are MIT or Apache-2.0 / BSD — `deny.toml` allow list already covers them. JetBrains Mono is OFL 1.1 but `cargo-deny` only scans crate graph licenses, not bundled assets, so no `deny.toml` change is needed. README gets an acknowledgment line.

### Device trait extension

`device::Device` gets no new methods. Existing `status()` returns `DeviceStatus`, which gains the five new fields. `RealDevice::status()` is extended to populate them; `FakeDevice` gets a builder that lets tests populate them too.

`App` struct gains `install_date_unix: Option<i64>`. `RealDevice`'s `installation_proxy.browse` parse path is extended to read `LSInstallDate`. `FakeDevice::with_apps` updated to accept the new field (default `None`).

## Tests

### Unit

- One test per badge eligibility function with synthetic `CardData` covering both sides of every threshold (89 vs 90 health, 1094 vs 1095 days, 89% vs 90% storage, etc.).
- Ranking test: 10 hand-picked `CardData` profiles, each asserting the expected top-3.
- Bundle ID matcher: positive case for each known jailbreak ID, negative case for similarly-named legitimate apps (e.g. an app whose name contains "cydia" but bundle ID does not match).
- Beta-build regex: positive cases (one developer beta, one public beta build string), negative cases (recent stable builds).
- `chip_name` mapping: at least one positive case per A13–A17, plus an unknown platform string.
- `xml_escape`: `<`, `>`, `&`, `"`, `'`.

### Integration (`tests/integration.rs`, no device)

- `card::run` against `FakeDevice` populated with a realistic `CardData`. Asserts: PNG written, file size > 50 KB, PNG signature (`\x89PNG\r\n\x1a\n`), dimensions parsed from IHDR are 1080×1080.
- Same flow with `--redact` asserts: SVG string contains `2022` but not `Mar 2022`, build number not present, no oldest-app name.
- `--no-open` path: no `open` invocation (mock the open call via a trait seam or just check the command returns `Ok` without trying to spawn).

### SVG snapshot

Five `CardData` fixtures, each with a checked-in `.svg` golden under `tests/fixtures/card/`. The test reads the golden and asserts `assert_eq!(render_svg(&fixture), golden)`. Update via `UPDATE_GOLDEN=1` env var. No `insta` (not in the project).

### E2E (`tests/e2e_card.rs`, `--features e2e`)

- Run `card::run` against a real connected device. Assert PNG ≥ 50 KB and 1080×1080.
- Never executes in CI; only compile-checked there.

### Manual

- Lucas runs `qk card` on his iPhone 14 Pro Max, inspects the PNG in Preview, posts on Twitter.

## Documentation

- `README.md` — add a row for `qk card` in the Commands table; add a "Show off your iPhone" subsection under Examples with an embedded sample PNG (`docs/qk-card-sample.png`) and 2–3 command examples.
- `README.md` — Acknowledgments section: JetBrains Mono (OFL 1.1).
- `CHANGELOG.md` — Unreleased section gains a `card` entry.
- `docs/ARCHITECTURE.md` — extend the command list, mention the SVG → PNG render pipeline.
- `CLAUDE.md` — add `qk card` to the data-source notes (which lockdown domains it reads), if any are non-obvious.

## Definition of done

- `qk card` end-to-end on a real iPhone writes a PNG that matches the approved mockup.
- All 15 badges implemented and unit-tested; ranking test passes.
- SVG snapshot tests pass; PNG integration test passes.
- `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test` clean.
- `cargo test --features e2e` passes against Lucas's device.
- README + CHANGELOG + ARCHITECTURE updated.
- Sample PNG committed to `docs/qk-card-sample.png`.
- Lucas posts the result and the post looks good.
