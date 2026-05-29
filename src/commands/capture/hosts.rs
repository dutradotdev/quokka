//! Per-process / per-remote-host aggregation for `qk capture --hosts`.
//!
//! Keys are sorted (`BTreeMap`) so the rendered output is stable across
//! refreshes — process X always lands above process Y, host A always
//! above B. Stable order matters more than insertion order for a top-
//! style display.

use std::collections::VecDeque;
use std::time::Instant;

use crate::device::Packet;

use super::{owner_label, Direction, Endpoint, ParsedPacket};

/// Cap on the per-host rolling tail of recent activity. Detail panes in
/// the TUI show the last ~8 entries; 20 leaves headroom for the spec's
/// "histórico recente" without unbounded growth.
pub const RECENT_CAP: usize = 20;

/// Cap on distinct processes tracked before extras fold into the overflow
/// bucket. Bounds memory on a long non-TTY `--hosts` capture where PID churn
/// would otherwise grow `per_proc` without limit.
const MAX_TRACKED_PROCS: usize = 256;
/// Cap on distinct remote endpoints tracked per process, same rationale.
const MAX_HOSTS_PER_PROC: usize = 256;

#[derive(Debug, Default)]
pub struct HostAggregator {
    pub(super) per_proc: std::collections::BTreeMap<
        (u32, String),
        std::collections::BTreeMap<(std::net::IpAddr, u16), HostStats>,
    >,
    /// Aggregate of traffic dropped once a cap is hit. Keeps the displayed
    /// totals honest without retaining a per-key breakdown.
    overflow: OverflowStats,
}

/// Folded counters for processes / endpoints beyond the display caps.
#[derive(Debug, Default)]
struct OverflowStats {
    packets: u64,
    bytes_out: u64,
    bytes_in: u64,
}

impl OverflowStats {
    fn record(&mut self, dir: Direction, bytes: u64) {
        self.packets += 1;
        match dir {
            Direction::Out => self.bytes_out += bytes,
            Direction::In => self.bytes_in += bytes,
        }
    }

    fn is_empty(&self) -> bool {
        self.packets == 0
    }
}

#[derive(Debug, Clone)]
pub struct HostStats {
    pub pkts: u64,
    pub bytes_out: u64,
    pub bytes_in: u64,
    /// Wall-clock-ish anchor for the first packet attributed to this host.
    /// Stored as [`Instant`] so we don't need a timezone crate; relative
    /// display lives in the TUI.
    pub first_seen: Instant,
    pub last_seen: Instant,
    /// Rolling tail of recent packets. Capped at [`RECENT_CAP`] so a
    /// chatty host doesn't grow without bound. The TUI shows the most
    /// recent N entries; the legacy text renderer ignores this field.
    pub recent: VecDeque<(Instant, Direction, u64)>,
}

impl HostStats {
    fn new_at(at: Instant) -> Self {
        Self {
            pkts: 0,
            bytes_out: 0,
            bytes_in: 0,
            first_seen: at,
            last_seen: at,
            recent: VecDeque::new(),
        }
    }
}

impl Default for HostStats {
    /// Only exists for callers that hold a `HostStats` value alone (tests,
    /// renderer scaffolding). Live aggregation goes through
    /// [`HostAggregator::add`] which uses [`HostStats::new_at`] so the
    /// timestamp is anchored to the packet, not the moment of insertion.
    fn default() -> Self {
        Self::new_at(Instant::now())
    }
}

impl HostAggregator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one packet at its arrival instant. Live ingest passes
    /// [`Instant::now()`]; filter replay passes the row's stored arrival
    /// so a `:app foo` toggle doesn't reset every host's `first_seen` to
    /// the moment the filter was applied.
    pub fn add_at(&mut self, p: &Packet, parsed: &ParsedPacket, at: Instant) {
        let dir = Direction::from_io_byte(p.io);
        let remote = match dir {
            Direction::Out => &parsed.dst,
            Direction::In => &parsed.src,
        };
        let Some(port) = remote.port else {
            return;
        };
        let bytes = p.data.len() as u64;
        let key = (p.pid, p.comm.clone());

        // Cap distinct processes: a brand-new process beyond the limit folds
        // into the overflow bucket instead of growing the map. Processes we
        // already track always proceed so their running stats stay accurate.
        // Cheap `len()` guard first so the `contains_key` traversal is
        // skipped entirely in the common under-cap case.
        if self.per_proc.len() >= MAX_TRACKED_PROCS && !self.per_proc.contains_key(&key) {
            self.overflow.record(dir, bytes);
            return;
        }
        let hosts = self.per_proc.entry(key).or_default();

        // Cap distinct endpoints per process the same way.
        let host_key = (remote.ip, port);
        if hosts.len() >= MAX_HOSTS_PER_PROC && !hosts.contains_key(&host_key) {
            self.overflow.record(dir, bytes);
            return;
        }
        let stats = hosts
            .entry(host_key)
            .or_insert_with(|| HostStats::new_at(at));
        stats.last_seen = at;
        stats.pkts += 1;
        match dir {
            Direction::Out => stats.bytes_out += bytes,
            Direction::In => stats.bytes_in += bytes,
        }
        stats.recent.push_back((at, dir, bytes));
        if stats.recent.len() > RECENT_CAP {
            stats.recent.pop_front();
        }
    }

    /// Convenience wrapper for live ingest paths that don't track an
    /// explicit arrival time. Defers to [`Self::add_at`] with
    /// `Instant::now()`. Kept for unit tests and any external caller.
    pub fn add(&mut self, p: &Packet, parsed: &ParsedPacket) {
        self.add_at(p, parsed, Instant::now());
    }

    pub fn is_empty(&self) -> bool {
        self.per_proc.is_empty()
    }

    /// Render a snapshot. `header_line` lets the live renderer prepend a
    /// "Last update: HH:MM:SS (capturing for ...)" line consistent with
    /// the spec example.
    pub fn render(&self, header_line: &str) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "{header_line}");
        let _ = writeln!(out);
        for ((pid, comm), hosts) in &self.per_proc {
            let owner = owner_label(*pid, comm);
            let _ = writeln!(out, "{owner}");
            // Sort hosts by descending traffic so the heavy hitters lead.
            let mut rows: Vec<_> = hosts.iter().collect();
            rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.bytes_out + s.bytes_in));
            for ((ip, port), stats) in rows {
                let endpoint = Endpoint {
                    ip: *ip,
                    port: Some(*port),
                };
                let _ = writeln!(
                    out,
                    "  {endpoint:<24}  {pkts} pkts   {out_h} out  /  {in_h} in",
                    pkts = stats.pkts,
                    out_h = crate::ui::format_bytes(stats.bytes_out),
                    in_h = crate::ui::format_bytes(stats.bytes_in),
                );
            }
            let _ = writeln!(out);
        }
        if !self.overflow.is_empty() {
            let _ = writeln!(
                out,
                "(+{} packets from capped processes/endpoints not shown — {} out / {} in)",
                self.overflow.packets,
                crate::ui::format_bytes(self.overflow.bytes_out),
                crate::ui::format_bytes(self.overflow.bytes_in),
            );
        }
        out
    }
}
