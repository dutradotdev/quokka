//! `quokka capture` — stream network packets from `com.apple.pcapd`.
//!
//! Phase 2: parses IP/TCP/UDP/ICMP out of the raw bytes with `etherparse`,
//! prints a pretty per-packet line (timestamp, direction, comm, protocol,
//! src/dst IP:port, byte count), and respects the producer's drop counter
//! so a slow renderer can't cause silent loss inside the device-side task.
//!
//! Background on pcapd (idevice crate 0.1.61, `services::pcapd`):
//! - `PcapdClient::connect(&provider)` opens lockdown service
//!   `com.apple.pcapd`.
//! - `client.next_packet().await` blocks until the device sends a frame.
//! - The crate's `normalize_data()` prepends a synthetic 14-byte Ethernet
//!   header with ethertype hard-coded to `0x0800` (IPv4). For IPv6 packets
//!   that lies — we skip Ethernet entirely and parse from the IP layer,
//!   letting etherparse pick IPv4/IPv6 from the first nibble.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::device::{Device, Packet, PacketStream};

mod hosts;
mod parser;
mod pcap_io;
mod style;
mod tui;

pub use hosts::{HostAggregator, HostStats};
pub use parser::{
    extract_sni, parse_dns_query, parse_summary, Direction, DnsQuery, Endpoint, ParsedPacket,
    Protocol,
};
pub use pcap_io::{CaptureFile, SaveFormat};

use parser::{try_extract_tcp_payload, try_extract_udp_payload};

/// Interval between "X packets dropped" notices on stderr when drops are
/// happening. Cheap enough that a quiet capture spends almost nothing on it.
const DROP_REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// How often `--hosts` clears the screen and reprints the aggregator
/// snapshot. Matches the spec's "atualiza periodicamente (a cada N
/// segundos, padrão 3)".
const HOSTS_REFRESH_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Stop after this many packets (useful for smoke-testing without
    /// piping Ctrl-C every time). `None` means "run until interrupted".
    pub max: Option<usize>,
    /// Optional file to write every captured packet to, in addition to the
    /// stdout summary. Extension picks the format — `.pcap` for classic,
    /// anything else (or no extension) for pcapng with per-packet process
    /// info smuggled in via the EPB comment option.
    pub save: Option<PathBuf>,
    /// Filters applied to every packet before printing or saving. AND-ed
    /// together: a packet must match every populated field.
    pub filter: Filter,
    /// Render mode. [`Mode::Stream`] is the default (per-packet lines from
    /// Phase 2); the others swap the rendering and add their own
    /// pre-filters (DNS = UDP/53, SNI = TCP/443).
    pub mode: Mode,
}

/// Output mode for `qk capture`. The four user-facing variants are
/// mutually exclusive at the clap layer; [`Mode::Headless`] is internal
/// to integration tests so they can drive the App's ingest path without
/// requiring a TTY.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// Default: interactive TUI streaming per-packet rows. Stream view.
    #[default]
    Stream,
    /// Interactive TUI opened directly on the Hosts (aggregated) view.
    Hosts,
    /// Extract DNS queries from UDP/53 payloads. One line per query.
    Dns,
    /// Extract TLS SNI from TCP/443 ClientHellos. One line per hello.
    Sni,
    /// Drain packets through the TUI's `App::ingest` without a terminal.
    /// Not user-facing — integration tests select it to exercise the same
    /// state machine the live TUI uses, without needing a TTY.
    Headless,
}

/// AND-combined per-packet filters. Empty fields are wildcards.
///
/// Fields fall into two layers: the cheap pre-parse ones
/// ([`matches_packet`](Self::matches_packet)) — comm / pid / interface —
/// gate whether we bother parsing IP at all, and the post-parse ones
/// ([`matches_parsed`](Self::matches_parsed)) — port / protocol — which
/// require a successful parse. The two-step split is what lets a heavy
/// `--app instagram` capture avoid running etherparse on irrelevant
/// packets.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Case-insensitive substring against [`Packet::comm`]. `--app instagram`
    /// matches `Instagram` and `InstagramShare` per the spec.
    pub app: Option<String>,
    /// Exact PID match.
    pub pid: Option<u32>,
    /// Port match against src OR dst, after parsing.
    pub port: Option<u16>,
    /// Protocol after parsing. ICMP collapses v4 and v6 into a single
    /// variant — see [`Protocol`].
    pub proto: Option<Protocol>,
    /// Case-insensitive substring against [`Packet::interface`]. Substring
    /// instead of exact so `--interface utun` matches `utun4`, `utun7`,
    /// etc. without forcing the user to know the suffix.
    pub interface: Option<String>,
}

impl Filter {
    /// Cheap pre-parse check using only fields pcapd provides directly.
    /// Returns `false` to skip the packet before any IP decoding work.
    pub fn matches_packet(&self, p: &Packet) -> bool {
        if let Some(needle) = self.app.as_deref() {
            if !p
                .comm
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
            {
                return false;
            }
        }
        if let Some(want) = self.pid {
            if p.pid != want {
                return false;
            }
        }
        if let Some(iface) = self.interface.as_deref() {
            if !p
                .interface
                .to_ascii_lowercase()
                .contains(&iface.to_ascii_lowercase())
            {
                return false;
            }
        }
        true
    }

    /// Post-parse check for filters that need the parsed summary
    /// (port / protocol). When the user asks for port/proto but the parse
    /// failed, we reject — claiming a match would be lying about a field
    /// we couldn't read.
    pub fn matches_parsed(&self, parsed: Option<&ParsedPacket>) -> bool {
        if self.port.is_none() && self.proto.is_none() {
            return true;
        }
        let Some(s) = parsed else {
            return false;
        };
        if let Some(want) = self.proto {
            if s.protocol != want {
                return false;
            }
        }
        if let Some(want) = self.port {
            let src_match = s.src.port == Some(want);
            let dst_match = s.dst.port == Some(want);
            if !src_match && !dst_match {
                return false;
            }
        }
        true
    }
}

pub async fn run(device: &dyn Device, opts: Options) -> Result<()> {
    use std::sync::atomic::Ordering;

    // Stream / Hosts normally use the interactive TUI (Phase 6). DNS / SNI
    // stay on the legacy line-based renderer — those modes are
    // intentionally pipe-friendly and don't need a terminal. Headless
    // also stays here as a test-only ingest drain.
    //
    // If stdout isn't a TTY (CI, pipe, `--max` smoke test, `--save` to
    // file) we fall through to the line renderer for Stream/Hosts too,
    // so the smoke-test and pipeline cases keep working without an
    // interactive shell.
    let tty = crate::ui::stdout_is_interactive();
    if tty && matches!(opts.mode, Mode::Stream | Mode::Hosts) {
        return tui::run(device, opts).await;
    }

    let stream = device.capture_packets().await?;
    let mut writer = match &opts.save {
        Some(path) => Some(
            CaptureFile::open(path)
                .with_context(|| format!("failed to open {} for writing", path.display()))?,
        ),
        None => None,
    };
    match opts.mode {
        Mode::Stream => eprintln!("Capturing packets. Press Ctrl-C to stop."),
        Mode::Hosts => {
            eprintln!("Aggregating by process + host (top-style). Press Ctrl-C to stop.")
        }
        Mode::Dns => eprintln!("Capturing DNS queries (UDP/53). Press Ctrl-C to stop."),
        Mode::Sni => eprintln!("Capturing TLS SNI (TCP/443). Press Ctrl-C to stop."),
        // Headless is test-only — no stderr banner, no final summary.
        Mode::Headless => {}
    }

    let started_at = std::time::Instant::now();
    let mut count: usize = 0;
    let mut last_reported_drops: u64 = 0;
    let mut aggregator = HostAggregator::new();
    let is_hosts = opts.mode == Mode::Hosts;

    let mut drop_tick = tokio::time::interval(DROP_REPORT_INTERVAL);
    drop_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    drop_tick.tick().await;

    // Only consulted in hosts mode (gated on `is_hosts` in the select arm
    // below). Always created so the select! arms can name it
    // unconditionally; the unused interval costs nothing.
    let mut hosts_tick = tokio::time::interval(HOSTS_REFRESH_INTERVAL);
    hosts_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    hosts_tick.tick().await;

    let PacketStream { mut rx, dropped } = stream;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    loop {
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                if is_hosts && !aggregator.is_empty() {
                    let _ = write!(out, "{}", aggregator.render(&hosts_header(started_at)));
                }
                let final_drops = dropped.load(Ordering::Relaxed);
                drop(writer);
                eprintln!("\nStopped after {count} packets ({final_drops} dropped).");
                if let Some(path) = &opts.save {
                    eprintln!("Saved to {}", path.display());
                }
                return Ok(());
            }
            _ = hosts_tick.tick(), if is_hosts => {
                if !aggregator.is_empty() {
                    // ANSI clear-screen + home. Pipe consumers see the
                    // codes; `--hosts` is interactive by design.
                    write!(out, "\x1b[2J\x1b[H")?;
                    write!(out, "{}", aggregator.render(&hosts_header(started_at)))?;
                    out.flush()?;
                }
            }
            _ = drop_tick.tick() => {
                let now = dropped.load(Ordering::Relaxed);
                let delta = now.saturating_sub(last_reported_drops);
                if delta > 0 {
                    eprintln!("warning: {delta} packets dropped (consumer too slow)");
                    last_reported_drops = now;
                }
            }
            maybe = rx.recv() => {
                let pkt = match maybe {
                    Some(Ok(p)) => p,
                    Some(Err(e)) => {
                        eprintln!("capture ended: {e}");
                        return Ok(());
                    }
                    None => {
                        let final_drops = dropped.load(Ordering::Relaxed);
                        eprintln!(
                            "capture ended: device stream closed after {count} packets ({final_drops} dropped)."
                        );
                        return Ok(());
                    }
                };
                if !opts.filter.matches_packet(&pkt) {
                    continue;
                }
                let parsed = parse_summary(&pkt);
                if !opts.filter.matches_parsed(parsed.as_ref()) {
                    continue;
                }
                // Each mode decides whether the packet "counted" toward
                // --max. DNS/SNI miss most packets (only the matching ones
                // count), so otherwise `--max 10` against a DNS capture
                // could stop after 10 random TCP frames.
                let handled = match opts.mode {
                    Mode::Stream => {
                        writeln!(out, "{}", format_line_with(&pkt, parsed.as_ref()))?;
                        if let Some(w) = writer.as_mut() {
                            if let Err(e) = w.write(&pkt) {
                                eprintln!("warning: capture file write failed: {e}");
                            }
                        }
                        true
                    }
                    Mode::Hosts => match parsed.as_ref() {
                        Some(s) => {
                            aggregator.add(&pkt, s);
                            true
                        }
                        None => false,
                    },
                    Mode::Dns => match render_dns_line(&pkt, parsed.as_ref()) {
                        Some(line) => {
                            writeln!(out, "{line}")?;
                            true
                        }
                        None => false,
                    },
                    Mode::Sni => match render_sni_line(&pkt, parsed.as_ref()) {
                        Some(line) => {
                            writeln!(out, "{line}")?;
                            true
                        }
                        None => false,
                    },
                    // Headless drains packets through filter+parse only.
                    // No stdout, no aggregator side-effect; --max still
                    // counts so tests exit deterministically.
                    Mode::Headless => true,
                };
                if handled {
                    count += 1;
                    if let Some(limit) = opts.max {
                        if count >= limit {
                            if is_hosts && !aggregator.is_empty() {
                                write!(out, "{}", aggregator.render(&hosts_header(started_at)))?;
                            }
                            let final_drops = dropped.load(Ordering::Relaxed);
                            drop(writer);
                            eprintln!("Reached --max {limit}, stopping ({final_drops} dropped).");
                            if let Some(path) = &opts.save {
                                eprintln!("Saved to {}", path.display());
                            }
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

/// PID sentinel pcapd uses when the kernel attributes the packet to no
/// specific process — typically broadcast / multicast / receive-side
/// kernel paths. Surfacing it as `4294967295` next to an empty `comm`
/// reads as a bug; we render `—` instead.
const NO_OWNER_PID: u32 = u32::MAX;

/// Render the "who" column of a packet line. Handles three quirks from
/// real captures: PID `u32::MAX` (no kernel owner), PID `0` (kernel
/// itself), and empty `comm` strings that pcapd emits for both of those
/// cases as well as some receive-side packets.
pub fn owner_label(pid: u32, comm: &str) -> String {
    let comm = comm.trim();
    match (pid, comm) {
        (NO_OWNER_PID, _) => "—".to_string(),
        (0, "") => "kernel".to_string(),
        (0, c) => format!("{} (kernel)", truncate_comm(c)),
        (pid, "") => format!("pid {pid}"),
        (pid, c) => format!("{} ({pid})", truncate_comm(c)),
    }
}

/// Trim `comm` to ≤15 chars, adding `..` when it would otherwise exceed.
/// 15 is the column width the spec example uses (`mDNSRespond..`).
pub fn truncate_comm(comm: &str) -> String {
    const MAX: usize = 15;
    let count = comm.chars().count();
    if count <= MAX {
        return comm.to_string();
    }
    let prefix: String = comm.chars().take(MAX - 2).collect();
    format!("{prefix}..")
}

/// Format `seconds`/`microseconds` (UTC, no timezone deps) as `HH:MM:SS.mmm`.
/// We deliberately stay on UTC to avoid pulling in a timezone crate; local
/// time can come later if it turns out to matter.
fn format_time(seconds: u32, microseconds: u32) -> String {
    let secs = seconds as u64;
    let hh = (secs / 3600) % 24;
    let mm = (secs / 60) % 60;
    let ss = secs % 60;
    let ms = (microseconds / 1000) % 1000;
    format!("{hh:02}:{mm:02}:{ss:02}.{ms:03}")
}

/// Phase 2 output. On parse failure, drops to a compact diagnostic line so
/// the stream stays useful — the spec disallows crashing on malformed input.
///
/// The fast-path caller in [`run`] already has a parsed summary (it needed
/// one for filter evaluation), so it uses [`format_line_with`] directly to
/// avoid parsing the same packet twice. This wrapper exists for the test
/// suite and any external caller that just wants a one-shot render.
pub fn format_line(p: &Packet) -> String {
    let parsed = parse_summary(p);
    format_line_with(p, parsed.as_ref())
}

/// Variant of [`format_line`] that takes the parsed summary instead of
/// computing it. Lets the hot loop parse once and reuse the result for
/// both filtering and rendering.
pub fn format_line_with(p: &Packet, parsed: Option<&ParsedPacket>) -> String {
    let time = format_time(p.seconds, p.microseconds);
    let arrow = Direction::from_io_byte(p.io).arrow();
    let owner = owner_label(p.pid, &p.comm);
    let bytes = p.data.len();
    match parsed {
        Some(s) => format!(
            "{time} {arrow} {owner} {proto} {src} → {dst}  {bytes}b",
            proto = s.protocol.as_str(),
            src = s.src,
            dst = s.dst,
        ),
        None => format!(
            "{time} {arrow} {owner} <parse error> iface={iface} bytes={bytes}",
            iface = p.interface,
        ),
    }
}

/// Line for `--dns` mode. `None` when this packet isn't a UDP/53 query
/// we can decode — mode dispatch skips it without incrementing `--max`.
fn render_dns_line(p: &Packet, parsed: Option<&ParsedPacket>) -> Option<String> {
    let s = parsed?;
    if s.protocol != Protocol::Udp {
        return None;
    }
    if s.src.port != Some(53) && s.dst.port != Some(53) {
        return None;
    }
    let payload = try_extract_udp_payload(p)?;
    let query = parse_dns_query(payload)?;
    let time = format_time(p.seconds, p.microseconds);
    let owner = owner_label(p.pid, &p.comm);
    Some(format!(
        "{time}  {owner:<22}  {qtype:<5}  {qname}",
        qtype = query.qtype,
        qname = query.qname,
    ))
}

/// Line for `--sni` mode. Same skip semantics as [`render_dns_line`].
fn render_sni_line(p: &Packet, parsed: Option<&ParsedPacket>) -> Option<String> {
    let s = parsed?;
    if s.protocol != Protocol::Tcp {
        return None;
    }
    if s.src.port != Some(443) && s.dst.port != Some(443) {
        return None;
    }
    let payload = try_extract_tcp_payload(p)?;
    let sni = extract_sni(payload)?;
    let time = format_time(p.seconds, p.microseconds);
    let owner = owner_label(p.pid, &p.comm);
    Some(format!("{time}  {owner:<22}  {sni}"))
}

/// Header line for the `--hosts` snapshot. Format follows the spec
/// example: `Last update: HH:MM:SS (capturing for Xm Ys)`.
fn hosts_header(started_at: std::time::Instant) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let hh = (secs / 3600) % 24;
    let mm = (secs / 60) % 60;
    let ss = secs % 60;
    // UTC suffix is explicit so users in non-UTC timezones can't confuse
    // this header with their local wall clock (C5 from the Phase 6 review).
    // Pulling chrono/time just for this is heavier than the bug warrants.
    format!(
        "Last update: {hh:02}:{mm:02}:{ss:02} UTC (capturing for {elapsed})",
        elapsed = format_elapsed(started_at.elapsed()),
    )
}

fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etherparse::PacketBuilder;
    use pcap_file::pcap::PcapWriter;
    use pcap_file::pcapng::blocks::enhanced_packet::EnhancedPacketOption;
    use std::fs::File;
    use std::io::BufWriter;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::path::Path;

    use super::pcap_io::packet_comment;

    /// Build a [`Packet`] whose `data` mimics what the device layer hands
    /// us: 14 bytes of synthetic Ethernet header followed by the real
    /// IP-layer payload. The Ethernet bytes are pure padding here —
    /// `parse_summary` skips them and reads from byte 14 onward.
    fn packet_with_payload(io: u8, comm: &str, payload: Vec<u8>) -> Packet {
        // 14 bytes of synthetic Ethernet header — the most common offset
        // candidate. Tests stay anchored on the en0 path; utun coverage
        // lives in a dedicated test below.
        let mut data = vec![0u8; 14];
        data.extend_from_slice(&payload);
        Packet {
            pid: 4521,
            comm: comm.into(),
            epid: 0,
            ecomm: String::new(),
            interface: "en0".into(),
            seconds: 12 * 3600 + 34 * 60 + 56, // 12:34:56 UTC
            microseconds: 789_000,
            io,
            data,
        }
    }

    fn ipv4_tcp_payload() -> Vec<u8> {
        let builder =
            PacketBuilder::ipv4([192, 168, 1, 42], [31, 13, 65, 36], 64).tcp(54321, 443, 0, 1000);
        let mut buf = Vec::with_capacity(builder.size(0));
        builder.write(&mut buf, &[]).unwrap();
        buf
    }

    fn ipv4_udp_payload() -> Vec<u8> {
        let builder = PacketBuilder::ipv4([192, 168, 1, 42], [1, 1, 1, 1], 64).udp(5353, 53);
        let mut buf = Vec::with_capacity(builder.size(0));
        builder.write(&mut buf, &[]).unwrap();
        buf
    }

    fn ipv6_tcp_payload() -> Vec<u8> {
        let builder = PacketBuilder::ipv6(
            [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2],
            64,
        )
        .tcp(80, 443, 0, 1000);
        let mut buf = Vec::with_capacity(builder.size(0));
        builder.write(&mut buf, &[]).unwrap();
        buf
    }

    #[test]
    fn parse_summary_extracts_ipv4_tcp() {
        let p = packet_with_payload(1, "mobilesafari", ipv4_tcp_payload());
        let s = parse_summary(&p).expect("should parse");
        assert_eq!(s.protocol, Protocol::Tcp);
        assert_eq!(s.src.ip, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)));
        assert_eq!(s.src.port, Some(54321));
        assert_eq!(s.dst.ip, IpAddr::V4(Ipv4Addr::new(31, 13, 65, 36)));
        assert_eq!(s.dst.port, Some(443));
    }

    #[test]
    fn parse_summary_extracts_ipv4_udp() {
        let p = packet_with_payload(1, "mDNSResponder", ipv4_udp_payload());
        let s = parse_summary(&p).expect("should parse");
        assert_eq!(s.protocol, Protocol::Udp);
        assert_eq!(s.src.port, Some(5353));
        assert_eq!(s.dst.port, Some(53));
    }

    #[test]
    fn parse_summary_extracts_ipv6_tcp_despite_lying_ethertype() {
        // The synthetic Ethernet header in `packet_with_payload` is all
        // zeros (not 0x0800/0x86DD), and `parse_summary` ignores it
        // entirely — it parses from byte 14 onward and lets etherparse
        // pick v4/v6 from the IP version nibble. This is the IPv6
        // regression test for the hard-coded ethertype quirk noted in
        // the module docs.
        let p = packet_with_payload(0, "wifid", ipv6_tcp_payload());
        let s = parse_summary(&p).expect("should parse");
        assert_eq!(s.protocol, Protocol::Tcp);
        assert_eq!(
            s.src.ip,
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1))
        );
        assert_eq!(s.dst.port, Some(443));
    }

    #[test]
    fn parse_summary_handles_utun_with_bsd_loopback_prefix() {
        // utun interfaces carry RemotePairing traffic on iOS 17+. The
        // idevice crate doesn't prepend an Ethernet header for them, but
        // the device hands us 4 bytes of BSD loopback framing before
        // the IP header. We expect parse_summary to fall back through
        // offset 14 → 4 → 0 and find the IP layer at offset 4.
        let mut data = vec![0u8; 4]; // BSD loopback header (protocol family)
        data.extend_from_slice(&ipv6_tcp_payload());
        let p = Packet {
            interface: "utun4".into(),
            data,
            ..packet_with_payload(0, "remotepairingd", vec![])
        };
        let s = parse_summary(&p).expect("utun packet should parse via fallback");
        assert_eq!(s.protocol, Protocol::Tcp);
        assert_eq!(s.dst.port, Some(443));
    }

    #[test]
    fn parse_summary_returns_none_on_truncated_payload() {
        // 10 bytes of Ethernet + nothing → less than the 14-byte synthetic
        // header the idevice crate prepends. We can't even strip the
        // Ethernet padding, let alone parse IP.
        let p = Packet {
            data: vec![0u8; 10],
            ..packet_with_payload(0, "x", vec![])
        };
        assert!(parse_summary(&p).is_none());
    }

    #[test]
    fn parse_summary_returns_none_on_garbage_after_ethernet() {
        // 14 bytes of Ethernet + 5 random bytes — not a parseable IP header.
        let p = packet_with_payload(0, "x", vec![0xff; 5]);
        assert!(parse_summary(&p).is_none());
    }

    #[test]
    fn format_line_renders_ipv4_tcp_pretty() {
        let p = packet_with_payload(1, "mobilesafari", ipv4_tcp_payload());
        let line = format_line(&p);
        // Time + arrow + comm + pid + protocol + endpoints + size. Locks
        // in the exact column shape so future tweaks fail loudly.
        assert!(line.starts_with("12:34:56.789 ↑ mobilesafari (4521) TCP "));
        assert!(line.contains("192.168.1.42:54321 → 31.13.65.36:443"));
        assert!(line.ends_with(&format!("  {}b", p.data.len())));
    }

    #[test]
    fn format_line_renders_ipv6_with_brackets() {
        let p = packet_with_payload(0, "wifid", ipv6_tcp_payload());
        let line = format_line(&p);
        assert!(line.contains("↓ "), "inbound arrow expected, got: {line}");
        assert!(line.contains("[2001:db8::1]:80 → [2001:db8::2]:443"));
    }

    #[test]
    fn format_line_falls_back_on_parse_error() {
        let p = packet_with_payload(1, "x", vec![0xff, 0xff]);
        let line = format_line(&p);
        assert!(line.contains("<parse error>"));
        assert!(line.contains("iface=en0"));
    }

    #[test]
    fn owner_label_handles_pcapd_sentinels() {
        // Real packets from the wire — u32::MAX (-1) is the "no owner"
        // sentinel pcapd uses for broadcast/multicast and some
        // receive-side kernel paths.
        assert_eq!(owner_label(u32::MAX, ""), "—");
        assert_eq!(owner_label(u32::MAX, "anything"), "—");
        // PID 0 with no comm = kernel proper.
        assert_eq!(owner_label(0, ""), "kernel");
        assert_eq!(owner_label(0, "kernel_task"), "kernel_task (kernel)");
        // Real PID, no comm — happens for some packets in tunnel paths.
        assert_eq!(owner_label(385, ""), "pid 385");
        // Normal: "comm (pid)", with truncation when needed.
        assert_eq!(owner_label(4521, "mobilesafari"), "mobilesafari (4521)");
        assert_eq!(
            owner_label(198, "notification_proxyd"),
            "notification_.. (198)"
        );
    }

    #[test]
    fn truncate_comm_keeps_short_names_unchanged_and_caps_long_ones() {
        assert_eq!(truncate_comm("safari"), "safari");
        // 15 chars exactly → unchanged.
        assert_eq!(truncate_comm("123456789012345"), "123456789012345");
        // 16 chars → 13 + ".." = 15.
        assert_eq!(truncate_comm("mDNSResponderXY"), "mDNSResponderXY");
        assert_eq!(truncate_comm("mDNSResponderXYZ"), "mDNSResponder..");
    }

    #[test]
    fn direction_from_io_byte_maps_as_documented() {
        assert_eq!(Direction::from_io_byte(0), Direction::In);
        assert_eq!(Direction::from_io_byte(1), Direction::Out);
        // Other values fall back to In; if pcapd ever uses an extra value
        // for "loopback" or similar, we'll need a new variant — for now
        // treat unknown as inbound rather than mis-attributing as outbound.
        assert_eq!(Direction::from_io_byte(7), Direction::In);
    }

    #[test]
    fn format_time_zero_pads_components() {
        assert_eq!(format_time(0, 0), "00:00:00.000");
        assert_eq!(format_time(3661, 1_000), "01:01:01.001");
        // Microseconds > 1s overflow into a millisecond modulo, never panic.
        assert_eq!(format_time(0, 2_500_000), "00:00:00.500");
    }

    #[test]
    fn save_format_from_path_defaults_to_pcapng() {
        // .pcap is the only thing that should produce classic pcap; we
        // default to pcapng so the comment metadata path is the easy one.
        assert_eq!(
            SaveFormat::from_path(Path::new("out.pcap")),
            SaveFormat::Pcap
        );
        assert_eq!(
            SaveFormat::from_path(Path::new("OUT.PCAP")),
            SaveFormat::Pcap
        );
        assert_eq!(
            SaveFormat::from_path(Path::new("out.pcapng")),
            SaveFormat::PcapNg
        );
        // No extension → pcapng.
        assert_eq!(
            SaveFormat::from_path(Path::new("capture")),
            SaveFormat::PcapNg
        );
        // Unknown extension → pcapng (richer is safer than dropping metadata).
        assert_eq!(
            SaveFormat::from_path(Path::new("out.cap")),
            SaveFormat::PcapNg
        );
    }

    #[test]
    fn packet_comment_uses_key_value_pairs() {
        let p = packet_with_payload(1, "instagram", vec![]);
        let c = packet_comment(&p);
        // Lock in the key=value shape — Wireshark filters depend on it
        // (`frame.comment contains "comm=instagram"`).
        assert!(c.contains("pid=4521"));
        assert!(c.contains("comm=instagram"));
        assert!(c.contains("iface=en0"));
        assert!(c.contains("io=1"));
    }

    #[test]
    fn capture_file_writes_pcap_roundtrip() {
        use pcap_file::pcap::PcapReader;
        let p = packet_with_payload(1, "mobilesafari", ipv4_tcp_payload());
        let path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let target: PathBuf = path.to_path_buf();
        // Force the .pcap branch even though tempfile names have no
        // extension — go through CaptureFile directly with a Pcap variant.
        {
            let mut w = CaptureFile::Pcap(
                PcapWriter::new(BufWriter::new(File::create(&target).unwrap())).unwrap(),
            );
            w.write(&p).unwrap();
        }
        let file = File::open(&target).unwrap();
        let mut reader = PcapReader::new(file).unwrap();
        let read_pkt = reader.next_packet().unwrap().unwrap();
        assert_eq!(read_pkt.orig_len as usize, p.data.len());
        assert_eq!(&read_pkt.data[..], &p.data[..]);
        // Only one packet was written; the next read should be None.
        assert!(reader.next_packet().is_none());
    }

    #[test]
    fn capture_file_writes_pcapng_with_process_comment() {
        use pcap_file::pcapng::blocks::Block;
        use pcap_file::pcapng::PcapNgReader;
        let p = packet_with_payload(1, "mobilesafari", ipv4_tcp_payload());
        let path = tempfile::NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .with_extension("pcapng");
        {
            let mut w = CaptureFile::open(&path).unwrap();
            w.write(&p).unwrap();
        }
        let file = File::open(&path).unwrap();
        let mut reader = PcapNgReader::new(file).unwrap();
        let mut epb_seen = false;
        while let Some(block) = reader.next_block() {
            if let Block::EnhancedPacket(epb) = block.unwrap() {
                epb_seen = true;
                assert_eq!(epb.original_len as usize, p.data.len());
                assert_eq!(&epb.data[..], &p.data[..]);
                let comment = epb
                    .options
                    .iter()
                    .find_map(|o| match o {
                        EnhancedPacketOption::Comment(c) => Some(c.to_string()),
                        _ => None,
                    })
                    .expect("EPB should carry a comment option");
                assert!(comment.contains("comm=mobilesafari"));
                assert!(comment.contains("pid=4521"));
                break;
            }
        }
        assert!(epb_seen, "expected at least one EnhancedPacket block");
    }

    fn filter_pkt(pid: u32, comm: &str, iface: &str, payload: Vec<u8>) -> Packet {
        Packet {
            pid,
            comm: comm.into(),
            interface: iface.into(),
            ..packet_with_payload(1, comm, payload)
        }
    }

    #[test]
    fn filter_empty_matches_everything() {
        let f = Filter::default();
        let p = filter_pkt(100, "anything", "en0", ipv4_tcp_payload());
        assert!(f.matches_packet(&p));
        assert!(f.matches_parsed(parse_summary(&p).as_ref()));
    }

    #[test]
    fn filter_app_is_case_insensitive_substring() {
        let f = Filter {
            app: Some("instagram".into()),
            ..Default::default()
        };
        assert!(f.matches_packet(&filter_pkt(1, "Instagram", "en0", vec![])));
        assert!(f.matches_packet(&filter_pkt(1, "InstagramShare", "en0", vec![])));
        assert!(!f.matches_packet(&filter_pkt(1, "SpringBoard", "en0", vec![])));
    }

    #[test]
    fn filter_pid_is_exact() {
        let f = Filter {
            pid: Some(4521),
            ..Default::default()
        };
        assert!(f.matches_packet(&filter_pkt(4521, "x", "en0", vec![])));
        assert!(!f.matches_packet(&filter_pkt(4522, "x", "en0", vec![])));
    }

    #[test]
    fn filter_interface_is_substring_so_utun_matches_numbered() {
        let f = Filter {
            interface: Some("utun".into()),
            ..Default::default()
        };
        assert!(f.matches_packet(&filter_pkt(1, "x", "utun4", vec![])));
        assert!(f.matches_packet(&filter_pkt(1, "x", "UTUN7", vec![])));
        assert!(!f.matches_packet(&filter_pkt(1, "x", "en0", vec![])));
    }

    #[test]
    fn filter_port_matches_src_or_dst() {
        let f = Filter {
            port: Some(443),
            ..Default::default()
        };
        // ipv4_tcp_payload() builds 54321 → 443, so 443 matches as dst.
        let p = filter_pkt(1, "x", "en0", ipv4_tcp_payload());
        assert!(f.matches_parsed(parse_summary(&p).as_ref()));
        // 54321 matches as src.
        let f2 = Filter {
            port: Some(54321),
            ..Default::default()
        };
        assert!(f2.matches_parsed(parse_summary(&p).as_ref()));
        // 80 matches neither.
        let f3 = Filter {
            port: Some(80),
            ..Default::default()
        };
        assert!(!f3.matches_parsed(parse_summary(&p).as_ref()));
    }

    #[test]
    fn filter_proto_distinguishes_tcp_udp() {
        let tcp_pkt = filter_pkt(1, "x", "en0", ipv4_tcp_payload());
        let udp_pkt = filter_pkt(1, "x", "en0", ipv4_udp_payload());
        let f_tcp = Filter {
            proto: Some(Protocol::Tcp),
            ..Default::default()
        };
        assert!(f_tcp.matches_parsed(parse_summary(&tcp_pkt).as_ref()));
        assert!(!f_tcp.matches_parsed(parse_summary(&udp_pkt).as_ref()));
    }

    #[test]
    fn filter_proto_or_port_with_failed_parse_rejects() {
        // Need-to-parse filters can't claim a match when parse fails — we
        // don't know the proto, so we have to drop the packet.
        let bad = filter_pkt(1, "x", "en0", vec![0xff; 5]);
        let f = Filter {
            proto: Some(Protocol::Tcp),
            ..Default::default()
        };
        assert!(!f.matches_parsed(parse_summary(&bad).as_ref()));
    }

    #[test]
    fn filter_combines_fields_with_and() {
        let p = filter_pkt(4521, "Instagram", "en0", ipv4_tcp_payload());
        let parsed = parse_summary(&p);
        // All fields match.
        let f = Filter {
            app: Some("instagram".into()),
            pid: Some(4521),
            port: Some(443),
            proto: Some(Protocol::Tcp),
            interface: Some("en0".into()),
        };
        assert!(f.matches_packet(&p));
        assert!(f.matches_parsed(parsed.as_ref()));
        // One field off → reject.
        let f_bad_pid = Filter {
            pid: Some(9999),
            ..f.clone()
        };
        assert!(!f_bad_pid.matches_packet(&p));
    }

    // -----------------------------------------------------------------
    // Phase 5: DNS / SNI parsers + hosts aggregator.
    // -----------------------------------------------------------------

    /// Build a DNS query message for one question. Header is RFC 1035
    /// §4.1.1; QR=0 (query), QDCOUNT=1, rest zero.
    fn build_dns_query(qname: &str, qtype: u16) -> Vec<u8> {
        let mut buf = Vec::new();
        // ID
        buf.extend_from_slice(&[0xab, 0xcd]);
        // Flags: standard query, recursion desired (0x0100).
        buf.extend_from_slice(&[0x01, 0x00]);
        // QDCOUNT=1, ANCOUNT/NSCOUNT/ARCOUNT=0
        buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        for label in qname.split('.') {
            buf.push(label.len() as u8);
            buf.extend_from_slice(label.as_bytes());
        }
        buf.push(0); // terminator
        buf.extend_from_slice(&qtype.to_be_bytes());
        buf.extend_from_slice(&[0x00, 0x01]); // QCLASS=IN
        buf
    }

    #[test]
    fn parse_dns_query_decodes_a_record() {
        let msg = build_dns_query("graph.instagram.com", 1);
        let q = parse_dns_query(&msg).expect("should parse A query");
        assert_eq!(q.qtype, "A");
        assert_eq!(q.qname, "graph.instagram.com");
    }

    #[test]
    fn parse_dns_query_decodes_aaaa_and_ptr() {
        let q = parse_dns_query(&build_dns_query("apple.com", 28)).unwrap();
        assert_eq!(q.qtype, "AAAA");
        let q = parse_dns_query(&build_dns_query("_apple-mobdev2._tcp.local", 12)).unwrap();
        assert_eq!(q.qtype, "PTR");
        assert_eq!(q.qname, "_apple-mobdev2._tcp.local");
    }

    #[test]
    fn parse_dns_query_rejects_responses_and_truncation() {
        // QR=1 (response) — must be skipped, we only render queries.
        let mut msg = build_dns_query("x.com", 1);
        msg[2] = 0x81; // set QR bit
        assert!(parse_dns_query(&msg).is_none());
        // Truncated header.
        assert!(parse_dns_query(&[0u8; 5]).is_none());
        // Garbage past header.
        assert!(
            parse_dns_query(&[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff])
                .is_none()
        );
    }

    /// Minimal TLS ClientHello with a single SNI extension.
    fn build_tls_client_hello(host: &str) -> Vec<u8> {
        // ClientHello body (everything after the 4-byte handshake header).
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]); // version: TLS 1.2
        body.extend_from_slice(&[0u8; 32]); // random
        body.push(0); // session_id_length
        body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]); // cipher_suites: 1 entry
        body.push(1); // compression_methods_length
        body.push(0); // null compression

        // server_name extension content.
        let host_bytes = host.as_bytes();
        let mut server_name = Vec::new();
        server_name.push(0); // name_type = host_name
        server_name.extend_from_slice(&(host_bytes.len() as u16).to_be_bytes());
        server_name.extend_from_slice(host_bytes);
        let mut sni_ext = Vec::new();
        sni_ext.extend_from_slice(&(server_name.len() as u16).to_be_bytes()); // server_name_list length
        sni_ext.extend_from_slice(&server_name);

        let mut ext_block = Vec::new();
        ext_block.extend_from_slice(&[0x00, 0x00]); // ext_type = server_name
        ext_block.extend_from_slice(&(sni_ext.len() as u16).to_be_bytes());
        ext_block.extend_from_slice(&sni_ext);

        body.extend_from_slice(&(ext_block.len() as u16).to_be_bytes()); // extensions length
        body.extend_from_slice(&ext_block);

        // Handshake header: msg_type=1, length(3) = body.len().
        let body_len = body.len();
        let mut hs = Vec::new();
        hs.push(1);
        hs.extend_from_slice(&[
            ((body_len >> 16) & 0xff) as u8,
            ((body_len >> 8) & 0xff) as u8,
            (body_len & 0xff) as u8,
        ]);
        hs.extend_from_slice(&body);

        // TLS record header: type=22, version=0x0301, length=hs.len().
        let hs_len = hs.len();
        let mut record = Vec::new();
        record.push(22);
        record.extend_from_slice(&[0x03, 0x01]);
        record.extend_from_slice(&(hs_len as u16).to_be_bytes());
        record.extend_from_slice(&hs);
        record
    }

    #[test]
    fn extract_sni_pulls_hostname_from_client_hello() {
        let payload = build_tls_client_hello("graph.instagram.com");
        let sni = extract_sni(&payload).expect("should find SNI");
        assert_eq!(sni, "graph.instagram.com");
    }

    #[test]
    fn extract_sni_returns_none_for_non_tls_and_truncated() {
        // Not a handshake record.
        let bad = vec![0x17, 0x03, 0x03, 0x00, 0x05, 0xde, 0xad, 0xbe, 0xef, 0xff];
        assert!(extract_sni(&bad).is_none());
        // Truncated.
        assert!(extract_sni(&[]).is_none());
        assert!(extract_sni(&[22, 3, 1]).is_none());
    }

    #[test]
    fn host_aggregator_buckets_by_direction() {
        // Build two packets to/from the same (remote) host. Out goes to
        // dst, In comes from src — different IPs in `parsed`, but our
        // aggregator key is "the remote one", so they should land in
        // the same bucket.
        let mut out_pkt = packet_with_payload(1, "Safari", ipv4_tcp_payload());
        out_pkt.pid = 10;
        let mut in_pkt = packet_with_payload(0, "Safari", ipv4_tcp_payload());
        in_pkt.pid = 10;

        let out_parsed = parse_summary(&out_pkt).unwrap();
        let in_parsed = ParsedPacket {
            // Swap src/dst so this looks like an inbound reply.
            src: out_parsed.dst.clone(),
            dst: out_parsed.src.clone(),
            protocol: out_parsed.protocol,
        };
        let mut agg = HostAggregator::new();
        agg.add(&out_pkt, &out_parsed);
        agg.add(&in_pkt, &in_parsed);

        // Single (pid, comm) bucket; single remote endpoint.
        assert_eq!(agg.per_proc.len(), 1);
        let hosts = agg.per_proc.values().next().unwrap();
        assert_eq!(hosts.len(), 1);
        let stats = hosts.values().next().unwrap();
        assert_eq!(stats.pkts, 2);
        assert!(stats.bytes_out > 0);
        assert!(stats.bytes_in > 0);
    }

    #[test]
    fn host_aggregator_render_includes_header_and_endpoint() {
        let pkt = {
            let mut p = packet_with_payload(1, "mobilesafari", ipv4_tcp_payload());
            p.pid = 4521;
            p
        };
        let parsed = parse_summary(&pkt).unwrap();
        let mut agg = HostAggregator::new();
        agg.add(&pkt, &parsed);
        let rendered = agg.render("Last update: 12:00:00 (capturing for 1s)");
        assert!(rendered.contains("Last update: 12:00:00"));
        assert!(rendered.contains("mobilesafari (4521)"));
        // Endpoint format: "IP:port" with byte stats trailing.
        assert!(rendered.contains("31.13.65.36:443"));
        assert!(rendered.contains("pkts"));
        assert!(rendered.contains(" out  /  "));
    }

    #[test]
    fn host_aggregator_caps_processes_and_folds_overflow() {
        // One shared remote endpoint, many distinct pids — well past the
        // internal process cap so the aggregator must stop growing and fold
        // the rest into the overflow bucket instead of leaking memory.
        let parsed = {
            let pkt = packet_with_payload(1, "proc", ipv4_tcp_payload());
            parse_summary(&pkt).unwrap()
        };
        let mut agg = HostAggregator::new();
        for pid in 0..300u32 {
            let mut pkt = packet_with_payload(1, "proc", ipv4_tcp_payload());
            pkt.pid = pid;
            agg.add(&pkt, &parsed);
        }
        assert_eq!(agg.per_proc.len(), 256, "process map must be capped");
        let rendered = agg.render("Last update: 12:00:00 (capturing for 1s)");
        assert!(
            rendered.contains("not shown"),
            "overflow note must appear once the cap is exceeded"
        );
    }

    #[test]
    fn render_dns_line_extracts_query_from_udp53() {
        // Build a UDP packet whose payload is a real DNS query.
        let dns = build_dns_query("apple.com", 1);
        let builder =
            etherparse::PacketBuilder::ipv4([192, 168, 1, 42], [1, 1, 1, 1], 64).udp(5353, 53);
        let mut payload = Vec::with_capacity(builder.size(dns.len()));
        builder.write(&mut payload, &dns).unwrap();
        let mut data = vec![0u8; 14];
        data.extend_from_slice(&payload);
        let pkt = Packet {
            data,
            ..packet_with_payload(1, "mDNSResponder", vec![])
        };
        let parsed = parse_summary(&pkt);
        let line = render_dns_line(&pkt, parsed.as_ref()).expect("should render");
        assert!(line.contains("mDNSResponder"));
        assert!(line.contains("A"));
        assert!(line.contains("apple.com"));
    }

    #[test]
    fn render_dns_line_skips_non_udp53() {
        let pkt = packet_with_payload(1, "mobilesafari", ipv4_tcp_payload());
        let parsed = parse_summary(&pkt);
        assert!(render_dns_line(&pkt, parsed.as_ref()).is_none());
    }
}
