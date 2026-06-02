//! Capture-file writer: pcap and pcapng. Format is picked from the file
//! extension at [`CaptureFile::open`] time. Buffered so short bursts don't
//! hit disk on every packet; the buffer flushes on `Drop` via the inner
//! `BufWriter`.

use std::borrow::Cow;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use pcap_file::pcap::{PcapPacket, PcapWriter};
use pcap_file::pcapng::blocks::enhanced_packet::{EnhancedPacketBlock, EnhancedPacketOption};
use pcap_file::pcapng::blocks::interface_description::InterfaceDescriptionBlock;
use pcap_file::pcapng::PcapNgWriter;
use pcap_file::DataLink;

use crate::device::Packet;

pub enum CaptureFile {
    /// Classic pcap. No room for per-packet process info; the comm/pid
    /// columns we render to stdout don't make it into the file.
    Pcap(PcapWriter<BufWriter<File>>),
    /// pcapng. Every packet gets an EPB `opt_comment` of the form
    /// `pid=N comm=NAME iface=IFACE io=I` so Wireshark's `frame.comment`
    /// filter (`frame.comment contains "Instagram"`) lights up.
    PcapNg(PcapNgWriter<BufWriter<File>>),
}

/// Distinct enum (not just an extension check at the call site) so the
/// classifier is testable in isolation and the format pick has a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFormat {
    Pcap,
    PcapNg,
}

impl SaveFormat {
    /// Pick a format from the file extension. `.pcap` → classic pcap;
    /// anything else (including no extension) defaults to pcapng — pcapng
    /// is strictly more capable, and we want the comment metadata by
    /// default.
    pub fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("pcap") => SaveFormat::Pcap,
            _ => SaveFormat::PcapNg,
        }
    }
}

impl CaptureFile {
    pub fn open(path: &Path) -> Result<Self> {
        let file = BufWriter::new(File::create(path)?);
        Ok(match SaveFormat::from_path(path) {
            SaveFormat::Pcap => CaptureFile::Pcap(PcapWriter::new(file)?),
            SaveFormat::PcapNg => {
                let mut w = PcapNgWriter::new(file)?;
                // pcapng requires at least one Interface Description Block
                // before any Enhanced Packet Block. We only ever write
                // Ethernet-link packets (pcapd's `normalize_data` makes
                // sure of that), so a single IDB with linktype 1 covers
                // everything we'll emit.
                let idb = InterfaceDescriptionBlock {
                    linktype: DataLink::ETHERNET,
                    snaplen: 0xFFFF,
                    options: vec![],
                };
                w.write_pcapng_block(idb)?;
                CaptureFile::PcapNg(w)
            }
        })
    }

    pub fn write(&mut self, p: &Packet) -> Result<()> {
        let ts = packet_timestamp(p);
        let len = p.data.len() as u32;
        match self {
            CaptureFile::Pcap(w) => {
                w.write_packet(&PcapPacket {
                    timestamp: ts,
                    orig_len: len,
                    data: Cow::Borrowed(&p.data),
                })?;
            }
            CaptureFile::PcapNg(w) => {
                let comment = packet_comment(p);
                let block = EnhancedPacketBlock {
                    interface_id: 0,
                    timestamp: ts,
                    original_len: len,
                    data: Cow::Borrowed(&p.data),
                    options: vec![EnhancedPacketOption::Comment(Cow::Owned(comment))],
                };
                w.write_pcapng_block(block)?;
            }
        }
        Ok(())
    }
}

/// pcapng EPB comment text — Wireshark surfaces this as `frame.comment`.
/// Format is intentionally `key=value` pairs (not free text) so people can
/// grep / filter on it after the fact.
pub(super) fn packet_comment(p: &Packet) -> String {
    format!(
        "pid={} comm={} iface={} io={}",
        p.pid, p.comm, p.interface, p.io,
    )
}

/// Build a `Duration` from pcapd's split seconds/microseconds fields. Used
/// for both pcap (legacy μs resolution) and pcapng (which can carry higher
/// resolution but we have none to give).
fn packet_timestamp(p: &Packet) -> Duration {
    Duration::from_secs(p.seconds as u64) + Duration::from_micros(p.microseconds as u64)
}
