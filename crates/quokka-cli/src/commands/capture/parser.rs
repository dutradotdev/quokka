//! Pure packet parsers: IP / TCP / UDP / ICMP summary, DNS queries, TLS SNI.
//!
//! Lifted out of `mod.rs` so each parser can be tested with hand-crafted
//! fixtures without touching the device layer. Every function here is
//! pure: input is bytes, output is `Option<...>`.

use std::net::IpAddr;

use etherparse::{NetSlice, SlicedPacket, TransportSlice};

use crate::device::Packet;

/// Offsets we try, in order, to locate the IP header inside `Packet::data`.
///
/// The idevice crate's `normalize_data()` only prepends the 14-byte
/// synthetic Ethernet header when `frame_pre_length == 0`, and only
/// strips the 4-byte BSD loopback prefix for `pdp_ip*` interfaces.
/// Anything else (notably `utun*`, used by RemotePairing on iOS 17+)
/// arrives raw — Ethernet not added, BSD loopback prefix not stripped.
/// We don't have `frame_pre_length` at this layer, so we attempt parses
/// at each plausible offset and accept the first one that succeeds.
///
/// Order matters for ambiguous payloads: 14 wins for the common case
/// (en0, pdp_ip0 after normalization), then 4 for utun BSD-loopback
/// framing, then 0 for raw IP as a last-resort.
const IP_OFFSET_CANDIDATES: &[usize] = &[14, 4, 0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Outbound, written from the device.
    Out,
    /// Inbound, received by the device.
    In,
}

impl Direction {
    /// Map the raw pcapd `io` byte. The upstream crate doesn't document the
    /// semantics; we follow the convention used by macOS BPF (`PKTAP_FLAG_
    /// DIR_OUT`-style) where `1` is outbound. **Validate empirically** with
    /// known traffic and flip this if needed — the test suite locks in
    /// whichever direction we commit to.
    pub fn from_io_byte(io: u8) -> Self {
        if io == 1 {
            Direction::Out
        } else {
            Direction::In
        }
    }

    pub(super) fn arrow(self) -> &'static str {
        match self {
            Direction::Out => "↑",
            Direction::In => "↓",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
    Icmp,
    Other,
}

impl Protocol {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Protocol::Tcp => "TCP",
            Protocol::Udp => "UDP",
            Protocol::Icmp => "ICMP",
            Protocol::Other => "OTHER",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub ip: IpAddr,
    /// `None` for ICMP / "Other" — those have no L4 port concept here.
    pub port: Option<u16>,
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // IPv6 needs brackets to disambiguate the colon-separated port.
        match (self.ip, self.port) {
            (IpAddr::V6(v6), Some(p)) => write!(f, "[{v6}]:{p}"),
            (IpAddr::V6(v6), None) => write!(f, "[{v6}]"),
            (IpAddr::V4(v4), Some(p)) => write!(f, "{v4}:{p}"),
            (IpAddr::V4(v4), None) => write!(f, "{v4}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedPacket {
    pub protocol: Protocol,
    pub src: Endpoint,
    pub dst: Endpoint,
}

/// Pure parsing function. Skips the synthetic 14-byte Ethernet header the
/// idevice crate prepended, then lets etherparse decide IPv4 vs IPv6 by
/// peeking at the IP version nibble. Returns `None` on any parse failure
/// (truncated payload, unknown protocol, malformed header) — the caller
/// is expected to fall back to a `<parse error>` line rather than crash.
pub fn parse_summary(packet: &Packet) -> Option<ParsedPacket> {
    let parsed = IP_OFFSET_CANDIDATES
        .iter()
        .filter(|&&off| packet.data.len() > off)
        .find_map(|&off| SlicedPacket::from_ip(&packet.data[off..]).ok())?;

    let (src_ip, dst_ip) = match parsed.net.as_ref()? {
        NetSlice::Ipv4(s) => (
            IpAddr::V4(s.header().source_addr()),
            IpAddr::V4(s.header().destination_addr()),
        ),
        NetSlice::Ipv6(s) => (
            IpAddr::V6(s.header().source_addr()),
            IpAddr::V6(s.header().destination_addr()),
        ),
        // etherparse may add new NetSlice variants (e.g. ARP); treat them
        // as unparseable rather than guessing.
        _ => return None,
    };

    let (protocol, src_port, dst_port) = match parsed.transport.as_ref() {
        Some(TransportSlice::Tcp(t)) => (
            Protocol::Tcp,
            Some(t.source_port()),
            Some(t.destination_port()),
        ),
        Some(TransportSlice::Udp(u)) => (
            Protocol::Udp,
            Some(u.source_port()),
            Some(u.destination_port()),
        ),
        Some(TransportSlice::Icmpv4(_)) | Some(TransportSlice::Icmpv6(_)) => {
            (Protocol::Icmp, None, None)
        }
        // Unknown transport (or none) — still useful to log src/dst IP.
        _ => (Protocol::Other, None, None),
    };

    Some(ParsedPacket {
        protocol,
        src: Endpoint {
            ip: src_ip,
            port: src_port,
        },
        dst: Endpoint {
            ip: dst_ip,
            port: dst_port,
        },
    })
}

/// Walk the same offset candidates as [`parse_summary`], stopping at the
/// first slice that decodes as UDP, and return the L7 payload.
pub(super) fn try_extract_udp_payload(p: &Packet) -> Option<&[u8]> {
    for &off in IP_OFFSET_CANDIDATES {
        let slice = p.data.get(off..)?;
        if let Ok(parsed) = SlicedPacket::from_ip(slice) {
            if let Some(TransportSlice::Udp(udp)) = parsed.transport {
                return Some(udp.payload());
            }
        }
    }
    None
}

/// Same shape as [`try_extract_udp_payload`] but for TCP. Returns the
/// segment payload (post-options), which is what TLS ClientHello sits in.
pub(super) fn try_extract_tcp_payload(p: &Packet) -> Option<&[u8]> {
    for &off in IP_OFFSET_CANDIDATES {
        let slice = p.data.get(off..)?;
        if let Ok(parsed) = SlicedPacket::from_ip(slice) {
            if let Some(TransportSlice::Tcp(tcp)) = parsed.transport {
                return Some(tcp.payload());
            }
        }
    }
    None
}

/// One parsed DNS query — only the bits the `--dns` renderer needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsQuery {
    /// Record type as a string (`"A"`, `"AAAA"`, `"PTR"`, ...). Unknown
    /// codes render as `"TYPE<n>"` so we don't drop the line silently.
    pub qtype: String,
    /// Fully-qualified-ish name, dot-joined from the wire labels.
    pub qname: String,
}

/// Parse a UDP payload as a DNS *query* message. Returns `None` for
/// responses, malformed packets, or anything that doesn't look like DNS.
pub fn parse_dns_query(payload: &[u8]) -> Option<DnsQuery> {
    // RFC 1035 §4.1.1 — fixed 12-byte header.
    if payload.len() < 12 {
        return None;
    }
    // Flags: QR bit is the top bit of byte 2. We only want queries (0).
    let qr_is_response = payload[2] & 0x80 != 0;
    if qr_is_response {
        return None;
    }
    let qdcount = u16::from_be_bytes([payload[4], payload[5]]);
    if qdcount == 0 {
        return None;
    }
    let mut idx = 12usize;
    let qname = read_dns_name(payload, &mut idx, 0)?;
    if idx + 4 > payload.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([payload[idx], payload[idx + 1]]);
    Some(DnsQuery {
        qtype: dns_qtype_name(qtype),
        qname,
    })
}

/// Walk a DNS name with bounded compression-pointer recursion. The depth
/// guard keeps a malicious or buggy packet from looping forever via
/// pointers that reference each other.
fn read_dns_name(buf: &[u8], idx: &mut usize, depth: u8) -> Option<String> {
    if depth > 5 {
        return None;
    }
    let mut labels: Vec<String> = Vec::new();
    loop {
        if *idx >= buf.len() {
            return None;
        }
        let len = buf[*idx];
        if len == 0 {
            *idx += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            // Pointer: two bottom bits of `len` + next byte = absolute
            // offset within the message. Pointers don't usually appear in
            // queries, but we handle them defensively.
            if *idx + 1 >= buf.len() {
                return None;
            }
            let offset = (((len & 0x3F) as usize) << 8) | (buf[*idx + 1] as usize);
            *idx += 2;
            let mut sub = offset;
            let tail = read_dns_name(buf, &mut sub, depth + 1)?;
            labels.push(tail);
            break;
        }
        let len = len as usize;
        *idx += 1;
        if *idx + len > buf.len() {
            return None;
        }
        let label = std::str::from_utf8(&buf[*idx..*idx + len]).ok()?;
        labels.push(label.to_string());
        *idx += len;
    }
    Some(labels.join("."))
}

fn dns_qtype_name(t: u16) -> String {
    match t {
        1 => "A".into(),
        2 => "NS".into(),
        5 => "CNAME".into(),
        6 => "SOA".into(),
        12 => "PTR".into(),
        15 => "MX".into(),
        16 => "TXT".into(),
        28 => "AAAA".into(),
        33 => "SRV".into(),
        65 => "HTTPS".into(),
        257 => "CAA".into(),
        other => format!("TYPE{other}"),
    }
}

/// Extract SNI hostname from a TCP payload that starts with a TLS record.
/// Returns `None` for non-TLS payloads, non-ClientHello records, or any
/// truncation that prevents reading the SNI extension.
///
/// Hand-rolled instead of pulling in `tls-parser` (heavy, parses things
/// we don't need). RFC 8446 §4.1.2 + RFC 6066 §3 cover the format.
pub fn extract_sni(payload: &[u8]) -> Option<String> {
    // TLS record: ContentType(1) ProtocolVersion(2) Length(2) Fragment(N).
    if payload.len() < 5 || payload[0] != 22 {
        return None;
    }
    // Bound the handshake fragment by the record-layer Length field so a
    // ClientHello that was fragmented across multiple TCP segments doesn't
    // get parsed past its captured boundary — reading the tail of an
    // unrelated segment would happily land on an ext_type=0 by coincidence
    // and emit a hostname built from garbage bytes.
    let rec_len = u16::from_be_bytes([payload[3], payload[4]]) as usize;
    let hs_end = 5usize.checked_add(rec_len)?.min(payload.len());
    if hs_end < 5 + 4 {
        return None;
    }
    let hs = &payload[5..hs_end];
    // Handshake message: msg_type(1) length(3) ClientHello{...}.
    if hs[0] != 1 {
        return None;
    }
    let hs_body_len = ((hs[1] as usize) << 16) | ((hs[2] as usize) << 8) | (hs[3] as usize);
    let body_end = 4usize.checked_add(hs_body_len)?.min(hs.len());
    if body_end < 4 {
        return None;
    }
    let body = &hs[4..body_end];
    // ClientHello: version(2) random(32) session_id<u8> cipher_suites<u16>
    // compression_methods<u8> extensions<u16>.
    let mut i = 2 + 32;
    let sid_len = *body.get(i)? as usize;
    i = i.checked_add(1)?.checked_add(sid_len)?;
    let cs_len = u16::from_be_bytes([*body.get(i)?, *body.get(i + 1)?]) as usize;
    i = i.checked_add(2)?.checked_add(cs_len)?;
    let cm_len = *body.get(i)? as usize;
    i = i.checked_add(1)?.checked_add(cm_len)?;
    let ext_total = u16::from_be_bytes([*body.get(i)?, *body.get(i + 1)?]) as usize;
    i = i.checked_add(2)?;
    let end = (i.checked_add(ext_total)?).min(body.len());
    while i + 4 <= end {
        let ext_type = u16::from_be_bytes([body[i], body[i + 1]]);
        let ext_len = u16::from_be_bytes([body[i + 2], body[i + 3]]) as usize;
        i += 4;
        if ext_type == 0 {
            // server_name extension: list_len(u16) [name_type(u8)
            // host_name<u16>]
            if i + 2 > end {
                return None;
            }
            let _list_len = u16::from_be_bytes([body[i], body[i + 1]]);
            i += 2;
            if i + 3 > end || body[i] != 0 {
                return None;
            }
            let name_len = u16::from_be_bytes([body[i + 1], body[i + 2]]) as usize;
            i += 3;
            if i + name_len > end {
                return None;
            }
            return std::str::from_utf8(&body[i..i + name_len])
                .ok()
                .map(str::to_string);
        }
        i += ext_len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// Build a `Packet` whose `data` is `bytes`. The other pcapd fields are
    /// irrelevant to the pure parsers, so they get neutral values.
    fn pkt(bytes: Vec<u8>) -> Packet {
        Packet {
            pid: 0,
            comm: String::new(),
            epid: 0,
            ecomm: String::new(),
            interface: String::new(),
            seconds: 0,
            microseconds: 0,
            io: 0,
            data: bytes,
        }
    }

    /// Prepend a 14-byte synthetic Ethernet header so `parse_summary` finds the
    /// IP header at its first (offset 14) candidate — the common normalized case.
    fn with_eth(ip_packet: Vec<u8>) -> Vec<u8> {
        let mut data = vec![0u8; 14];
        data.extend_from_slice(&ip_packet);
        data
    }

    fn ipv4_tcp(src: [u8; 4], sp: u16, dst: [u8; 4], dp: u16) -> Vec<u8> {
        let builder = etherparse::PacketBuilder::ipv4(src, dst, 64).tcp(sp, dp, 0, 1024);
        let mut out = Vec::new();
        builder.write(&mut out, &[]).expect("build ipv4/tcp");
        out
    }

    fn ipv6_udp(src: [u8; 16], sp: u16, dst: [u8; 16], dp: u16) -> Vec<u8> {
        let builder = etherparse::PacketBuilder::ipv6(src, dst, 64).udp(sp, dp);
        let mut out = Vec::new();
        builder.write(&mut out, &[]).expect("build ipv6/udp");
        out
    }

    #[test]
    fn direction_from_io_byte_treats_one_as_outbound() {
        assert_eq!(Direction::from_io_byte(1), Direction::Out);
        assert_eq!(Direction::from_io_byte(0), Direction::In);
        assert_eq!(Direction::from_io_byte(2), Direction::In);
        assert_eq!(Direction::Out.arrow(), "↑");
        assert_eq!(Direction::In.arrow(), "↓");
    }

    #[test]
    fn protocol_as_str_covers_every_variant() {
        assert_eq!(Protocol::Tcp.as_str(), "TCP");
        assert_eq!(Protocol::Udp.as_str(), "UDP");
        assert_eq!(Protocol::Icmp.as_str(), "ICMP");
        assert_eq!(Protocol::Other.as_str(), "OTHER");
    }

    #[test]
    fn endpoint_display_brackets_ipv6_and_omits_missing_port() {
        let v4 = Endpoint {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            port: Some(443),
        };
        assert_eq!(v4.to_string(), "10.0.0.1:443");

        let v6 = Endpoint {
            ip: IpAddr::V6(Ipv6Addr::LOCALHOST),
            port: Some(53),
        };
        assert_eq!(v6.to_string(), "[::1]:53");

        let v6_no_port = Endpoint {
            ip: IpAddr::V6(Ipv6Addr::LOCALHOST),
            port: None,
        };
        assert_eq!(v6_no_port.to_string(), "[::1]");
    }

    #[test]
    fn parse_summary_extracts_ipv4_tcp_endpoints() {
        let data = with_eth(ipv4_tcp([10, 0, 0, 1], 51000, [93, 184, 216, 34], 443));
        let parsed = parse_summary(&pkt(data)).expect("ipv4/tcp parses");
        assert_eq!(parsed.protocol, Protocol::Tcp);
        assert_eq!(parsed.src.to_string(), "10.0.0.1:51000");
        assert_eq!(parsed.dst.to_string(), "93.184.216.34:443");
    }

    #[test]
    fn parse_summary_extracts_ipv6_udp_endpoints() {
        let src = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1).octets();
        let dst = Ipv6Addr::new(0x2606, 0x4700, 0, 0, 0, 0, 0, 0x1111).octets();
        let data = with_eth(ipv6_udp(src, 5353, dst, 53));
        let parsed = parse_summary(&pkt(data)).expect("ipv6/udp parses");
        assert_eq!(parsed.protocol, Protocol::Udp);
        assert_eq!(parsed.src.port, Some(5353));
        assert_eq!(parsed.dst.port, Some(53));
    }

    #[test]
    fn parse_summary_falls_back_to_raw_ip_offset_zero() {
        // No Ethernet prefix: only the offset-0 candidate can match.
        let data = ipv4_tcp([192, 168, 0, 2], 1234, [192, 168, 0, 1], 80);
        let parsed = parse_summary(&pkt(data)).expect("raw ip parses at offset 0");
        assert_eq!(parsed.dst.to_string(), "192.168.0.1:80");
    }

    #[test]
    fn parse_summary_returns_none_for_garbage() {
        assert!(parse_summary(&pkt(vec![])).is_none());
        assert!(parse_summary(&pkt(vec![0xff; 8])).is_none());
    }

    /// Assemble a minimal DNS query message for `name` with qtype 1 (A).
    fn dns_query(name: &str, qr_response: bool, qdcount: u16) -> Vec<u8> {
        let mut p = vec![
            0x12,
            0x34, // ID
            if qr_response { 0x80 } else { 0x00 },
            0x00, // flags
            (qdcount >> 8) as u8,
            qdcount as u8, // QDCOUNT
            0,
            0,
            0,
            0,
            0,
            0, // AN/NS/AR
        ];
        for label in name.split('.') {
            p.push(label.len() as u8);
            p.extend_from_slice(label.as_bytes());
        }
        p.push(0); // root label
        p.extend_from_slice(&[0x00, 0x01]); // QTYPE = A
        p.extend_from_slice(&[0x00, 0x01]); // QCLASS = IN
        p
    }

    #[test]
    fn parse_dns_query_reads_name_and_type() {
        let q = parse_dns_query(&dns_query("example.com", false, 1)).expect("dns query parses");
        assert_eq!(q.qname, "example.com");
        assert_eq!(q.qtype, "A");
    }

    #[test]
    fn parse_dns_query_rejects_responses_and_empty_question() {
        assert!(parse_dns_query(&dns_query("a.b", true, 1)).is_none());
        assert!(parse_dns_query(&dns_query("a.b", false, 0)).is_none());
        assert!(parse_dns_query(&[0u8; 4]).is_none()); // shorter than header
    }

    #[test]
    fn dns_qtype_name_maps_known_and_unknown_codes() {
        assert_eq!(dns_qtype_name(1), "A");
        assert_eq!(dns_qtype_name(28), "AAAA");
        assert_eq!(dns_qtype_name(65), "HTTPS");
        assert_eq!(dns_qtype_name(9999), "TYPE9999");
    }

    /// Assemble a minimal TLS ClientHello record carrying `host` in SNI.
    fn client_hello_with_sni(host: &str) -> Vec<u8> {
        let host = host.as_bytes();
        // server_name extension data: list_len(2) name_type(1) name_len(2) host
        let mut ext_data = Vec::new();
        let sni_entry_len = (1 + 2 + host.len()) as u16;
        ext_data.extend_from_slice(&sni_entry_len.to_be_bytes());
        ext_data.push(0); // name_type = host_name
        ext_data.extend_from_slice(&(host.len() as u16).to_be_bytes());
        ext_data.extend_from_slice(host);

        let mut extensions = Vec::new();
        extensions.extend_from_slice(&[0x00, 0x00]); // ext_type = server_name
        extensions.extend_from_slice(&(ext_data.len() as u16).to_be_bytes());
        extensions.extend_from_slice(&ext_data);

        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]); // legacy_version
        body.extend_from_slice(&[0u8; 32]); // random
        body.push(0); // session_id length
        body.extend_from_slice(&[0x00, 0x02, 0x00, 0x2f]); // cipher suites
        body.extend_from_slice(&[0x01, 0x00]); // compression methods
        body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        body.extend_from_slice(&extensions);

        let mut handshake = vec![0x01]; // ClientHello
        let blen = body.len();
        handshake.extend_from_slice(&[(blen >> 16) as u8, (blen >> 8) as u8, blen as u8]);
        handshake.extend_from_slice(&body);

        let mut record = vec![0x16, 0x03, 0x03]; // handshake, TLS 1.2 record
        record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
        record.extend_from_slice(&handshake);
        record
    }

    #[test]
    fn extract_sni_reads_hostname_from_client_hello() {
        let record = client_hello_with_sni("quokka.example.com");
        assert_eq!(extract_sni(&record).as_deref(), Some("quokka.example.com"));
    }

    #[test]
    fn extract_sni_rejects_non_tls_payloads() {
        assert!(extract_sni(&[]).is_none());
        assert!(extract_sni(&[0x17, 0x03, 0x03, 0x00, 0x05]).is_none()); // not handshake
        assert!(extract_sni(&[0x16, 0x03]).is_none()); // truncated record header
    }

    // --- Property tests: tolerant parsers never panic on arbitrary input. ---
    proptest::proptest! {
        #[test]
        fn parse_summary_never_panics(data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2048)) {
            let _ = parse_summary(&pkt(data));
        }

        #[test]
        fn parse_dns_query_never_panics(data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2048)) {
            let _ = parse_dns_query(&data);
        }

        #[test]
        fn extract_sni_never_panics(data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2048)) {
            let _ = extract_sni(&data);
        }
    }
}
