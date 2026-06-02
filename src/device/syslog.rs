//! Pure parser for the iOS `syslog_relay` line format. Lives next to the
//! device layer that consumes it (`RealDevice`'s syslog loop stitches
//! continuations and parses each frame) so the seam stays free of any command
//! module — a prerequisite for moving `device` into a presentation-free core.

use super::{LogEntry, LogLevel};

/// Parse a single BSD-syslog-ish iOS log line. Returns a structured
/// `LogEntry`; if parsing fails the entry has `level: Unknown`,
/// `process: "?"`, and the raw line as `message` so no data is lost.
pub fn parse_syslog_line(raw: &str) -> LogEntry {
    // Frames from `syslog_relay` come delimited by `\n\x00`. The trailing
    // delim is already stripped by `read_until_delim`, but real captures
    // sometimes show stray nulls / newlines on either end.
    let trimmed = raw
        .trim_start_matches('\0')
        .trim_start_matches('\n')
        .trim_end_matches('\0')
        .trim_end_matches('\n');

    // Continuation lines (leading whitespace) — caller stitches.
    // Format: "Mmm DD HH:MM:SS host process[pid] <Level>: message"
    // Skip the "Mmm DD HH:MM:SS" prefix (15 chars + 1 space at minimum).
    let mut rest = trimmed;

    let host_start = match find_after_timestamp(rest) {
        Some(idx) => idx,
        None => return unknown_entry(trimmed),
    };
    // BSD syslog: "Mmm DD HH:MM:SS" — bytes 7..15 are the HH:MM:SS slice.
    let time_text = rest.get(7..15).map(|s| s.to_string());
    rest = &rest[host_start..];

    // host process[pid] <Level>: message
    let (host, after_host) = match rest.split_once(' ') {
        Some(pair) => pair,
        None => return unknown_entry(trimmed),
    };

    let (process_token, after_proc) = match after_host.split_once(' ') {
        Some(pair) => pair,
        None => return unknown_entry(trimmed),
    };

    let (process, pid) = parse_process_pid(process_token);

    // Expect "<Level>:" then message.
    let (level, message) = if let Some(end) = after_proc.find('>') {
        if after_proc.starts_with('<') {
            let level_text = &after_proc[1..end];
            let after = &after_proc[end + 1..];
            let after = after.trim_start_matches(':').trim_start();
            (LogLevel::parse(level_text), after.to_string())
        } else {
            (LogLevel::Unknown, after_proc.to_string())
        }
    } else {
        (LogLevel::Unknown, after_proc.to_string())
    };

    LogEntry {
        timestamp_unix_ms: None,
        time_text,
        host: host.to_string(),
        process,
        pid,
        level,
        message,
    }
}

fn unknown_entry(raw: &str) -> LogEntry {
    LogEntry {
        timestamp_unix_ms: None,
        time_text: None,
        host: String::new(),
        process: "?".to_string(),
        pid: None,
        level: LogLevel::Unknown,
        message: raw.to_string(),
    }
}

/// The iOS syslog timestamp is "Mmm DD HH:MM:SS" (15 chars). Return the
/// index after that prefix + its trailing space, or None on malformed.
fn find_after_timestamp(s: &str) -> Option<usize> {
    // Fast path: the 16th byte should be a space.
    let bytes = s.as_bytes();
    if bytes.len() < 16 {
        return None;
    }
    if bytes[15] != b' ' {
        return None;
    }
    Some(16)
}

fn parse_process_pid(token: &str) -> (String, Option<u32>) {
    if let Some(open) = token.rfind('[') {
        if token.ends_with(']') {
            let pid_str = &token[open + 1..token.len() - 1];
            if let Ok(pid) = pid_str.parse::<u32>() {
                return (token[..open].to_string(), Some(pid));
            }
        }
    }
    (token.to_string(), None)
}

pub fn is_continuation(raw: &str) -> bool {
    raw.starts_with(' ') || raw.starts_with('\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_syslog_line_extracts_fields() {
        let raw =
            "Nov 14 22:13:20 Lucass-iPhone SpringBoard[63] <Warning>: Bluetooth: reconnecting";
        let e = parse_syslog_line(raw);
        assert_eq!(e.process, "SpringBoard");
        assert_eq!(e.pid, Some(63));
        assert_eq!(e.level, LogLevel::Warning);
        assert!(e.message.contains("Bluetooth"));
    }

    #[test]
    fn parse_syslog_line_handles_missing_pid() {
        let raw = "Nov 14 22:13:20 host process <Error>: boom";
        let e = parse_syslog_line(raw);
        assert_eq!(e.process, "process");
        assert_eq!(e.pid, None);
        assert_eq!(e.level, LogLevel::Error);
    }

    #[test]
    fn parse_syslog_line_garbage_becomes_unknown() {
        let raw = "totally not a syslog line";
        let e = parse_syslog_line(raw);
        assert_eq!(e.process, "?");
        assert_eq!(e.level, LogLevel::Unknown);
        assert!(e.message.contains("totally"));
    }

    #[test]
    fn is_continuation_detects_leading_whitespace() {
        assert!(is_continuation("    continuation"));
        assert!(is_continuation("\tcontinuation"));
        assert!(!is_continuation("Nov 14 ..."));
    }

    #[test]
    fn parse_syslog_line_captures_time_text_slice() {
        let raw = "Nov 14 22:13:20 host SpringBoard[63] <Notice>: hello";
        let e = parse_syslog_line(raw);
        assert_eq!(e.time_text.as_deref(), Some("22:13:20"));
        assert_eq!(e.host, "host");
        assert_eq!(e.message, "hello");
    }

    #[test]
    fn parse_syslog_line_strips_null_and_newline_framing() {
        // `syslog_relay` delivers frames as "\nLINE\0"; trim_start/end strip
        // both. A regression would push the leading byte into the date column
        // and the whole line would fall back to Unknown.
        let raw = "\nNov 14 22:13:20 host process[7] <Error>: boom\0";
        let e = parse_syslog_line(raw);
        assert_eq!(e.process, "process");
        assert_eq!(e.pid, Some(7));
        assert_eq!(e.level, LogLevel::Error);
        assert_eq!(e.message, "boom");
    }

    #[test]
    fn parse_syslog_line_non_numeric_pid_keeps_brackets_in_process() {
        // `process[abc]` shouldn't crash and shouldn't claim a PID it can't parse.
        let raw = "Nov 14 22:13:20 host weird[abc] <Info>: x";
        let e = parse_syslog_line(raw);
        assert_eq!(e.pid, None);
        assert_eq!(e.process, "weird[abc]");
    }

    #[test]
    fn parse_syslog_line_missing_level_brackets_keeps_message_intact() {
        // No `<Level>:` — the parser keeps everything after the process token
        // as the message and marks level Unknown.
        let raw = "Nov 14 22:13:20 host p[1] just a message";
        let e = parse_syslog_line(raw);
        assert_eq!(e.level, LogLevel::Unknown);
        assert!(e.message.contains("just a message"));
    }
}
