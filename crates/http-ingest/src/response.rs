//! Status-line and header parsing — the security-critical core of this crate.
//!
//! `parse_head` is a pure function over an accumulating byte buffer: `Ok(None)`
//! means "not enough bytes yet," `Ok(Some(head))` means a complete, validated
//! head was found, and `Err` means the connection is poisoned and must be
//! dropped. Every rejection rule here exists to defend against HTTP/1.1
//! response-smuggling shapes; see the hostile-input test matrix below, which
//! is the specification.
//!
//! ## Amendment to ADR-0014 (erratum, see docs/adr/0014 and the implementation
//! plan): the ADR as written hard-rejects any `Transfer-Encoding` header.
//! Verified live against both S3 buckets (2026-07-29), `ListObjectsV2`
//! responses — the primary chunk-discovery path, called every poll — always
//! arrive as `Transfer-Encoding: chunked` with no `Content-Length`. A blanket
//! rejection breaks the primary workload. This module instead implements a
//! bounded, spec-compliant chunked decoder: `Transfer-Encoding: chunked` is
//! the one accepted value; any other Transfer-Encoding value, a duplicate
//! Transfer-Encoding header, or Transfer-Encoding combined with
//! Content-Length is still rejected.

use crate::config::Limits;
use crate::error::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    pub framing: BodyFraming,
    pub connection_close: bool,
    pub head_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyFraming {
    /// No body regardless of headers (204, 304).
    None,
    ContentLength(u64),
    /// `Transfer-Encoding: chunked`. Total size is unknown until the body is
    /// fully decoded; `connection.rs` enforces `max_body_bytes` incrementally.
    Chunked,
}

/// Cap on a single chunk-size line, independent of `max_head_bytes`. Chunk
/// extensions are not supported (rejected outright), so a legitimate line is
/// a handful of hex digits plus CRLF; this bound exists only to stop an
/// unterminated line from growing the read buffer without limit.
const MAX_CHUNK_HEADER_LINE: usize = 256;

pub fn parse_head(buf: &[u8], limits: &Limits) -> Result<Option<Head>, Error> {
    let search_limit = buf.len().min(limits.max_head_bytes);

    let mut lines: Vec<&[u8]> = Vec::new();
    let mut line_start = 0usize;
    let mut head_len = None;

    let mut i = 0usize;
    while i < search_limit {
        if buf[i] != b'\n' {
            i += 1;
            continue;
        }
        if i == line_start || buf[i - 1] != b'\r' {
            return Err(Error::Protocol("bare LF"));
        }
        let line = &buf[line_start..i - 1];
        if line.contains(&b'\r') {
            return Err(Error::Protocol("bare CR in header line"));
        }
        if line.is_empty() {
            head_len = Some(i + 1);
            break;
        }
        lines.push(line);
        if lines.len() - 1 > limits.max_header_count {
            return Err(Error::Protocol("too many headers"));
        }
        line_start = i + 1;
        i += 1;
    }

    let head_len = match head_len {
        Some(len) => len,
        None => {
            if buf.len() >= limits.max_head_bytes {
                return Err(Error::Protocol("head too large"));
            }
            return Ok(None);
        }
    };

    let mut lines_iter = lines.into_iter();
    let status_line = lines_iter
        .next()
        .ok_or(Error::Protocol("missing status line"))?;
    let status = parse_status_line(status_line)?;

    let mut content_length: Option<u64> = None;
    let mut transfer_encoding_seen = false;
    let mut chunked = false;
    let mut connection_close = false;

    for line in lines_iter {
        let (name, value) = parse_header_line(line)?;

        if name.eq_ignore_ascii_case(b"content-length") {
            if content_length.is_some() {
                return Err(Error::Protocol("duplicate Content-Length"));
            }
            content_length = Some(parse_content_length(value)?);
        } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
            if transfer_encoding_seen {
                return Err(Error::Protocol("duplicate Transfer-Encoding"));
            }
            transfer_encoding_seen = true;
            if !trim_ows(value).eq_ignore_ascii_case(b"chunked") {
                return Err(Error::Protocol("unsupported Transfer-Encoding"));
            }
            chunked = true;
        } else if name.eq_ignore_ascii_case(b"connection") {
            if value
                .split(|&b| b == b',')
                .any(|tok| trim_ows(tok).eq_ignore_ascii_case(b"close"))
            {
                connection_close = true;
            }
        } else if name.eq_ignore_ascii_case(b"content-encoding")
            && !trim_ows(value).eq_ignore_ascii_case(b"identity")
        {
            return Err(Error::Protocol("unsupported Content-Encoding"));
        }
    }

    if content_length.is_some() && chunked {
        return Err(Error::Protocol("Content-Length and Transfer-Encoding together"));
    }

    if (100..200).contains(&status) {
        return Err(Error::Protocol("1xx status"));
    }

    let framing = if status == 204 || status == 304 {
        BodyFraming::None
    } else if chunked {
        BodyFraming::Chunked
    } else if let Some(len) = content_length {
        if len > limits.max_body_bytes {
            return Err(Error::BodyTooLarge { len, limit: limits.max_body_bytes });
        }
        BodyFraming::ContentLength(len)
    } else {
        return Err(Error::Protocol("missing Content-Length"));
    };

    Ok(Some(Head { status, framing, connection_close, head_len }))
}

fn parse_status_line(line: &[u8]) -> Result<u16, Error> {
    if line.len() < 12 {
        return Err(Error::Protocol("status line too short"));
    }
    let protocol_ok = &line[0..8] == b"HTTP/1.1" || &line[0..8] == b"HTTP/1.0";
    if !protocol_ok || line[8] != b' ' {
        return Err(Error::Protocol("unsupported protocol token"));
    }
    let digits = &line[9..12];
    if !digits.iter().all(u8::is_ascii_digit) {
        return Err(Error::Protocol("non-digit status code"));
    }
    let status: u16 = std::str::from_utf8(digits)
        .expect("ASCII digits are valid UTF-8")
        .parse()
        .expect("three ASCII digits always parse as u16");

    match line.len().cmp(&12) {
        std::cmp::Ordering::Equal => {}
        std::cmp::Ordering::Greater => {
            if line[12] != b' ' {
                return Err(Error::Protocol("malformed reason phrase separator"));
            }
            if !line[13..].iter().all(|&b| is_vchar_sp_htab(b)) {
                return Err(Error::Protocol("invalid byte in reason phrase"));
            }
        }
        std::cmp::Ordering::Less => unreachable!("checked line.len() >= 12 above"),
    }

    Ok(status)
}

fn parse_header_line(line: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    if line.first() == Some(&b' ') || line.first() == Some(&b'\t') {
        return Err(Error::Protocol("obs-fold"));
    }
    let colon = line.iter().position(|&b| b == b':').ok_or(Error::Protocol("missing header colon"))?;
    let name = &line[..colon];
    if name.is_empty() || !name.iter().all(|&b| is_tchar(b)) {
        return Err(Error::Protocol("invalid header name"));
    }
    let raw_value = &line[colon + 1..];
    let value = trim_ows(raw_value);
    if !value.iter().all(|&b| is_vchar_sp_htab(b)) {
        return Err(Error::Protocol("invalid byte in header value"));
    }
    Ok((name, value))
}

fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~')
}

fn is_vchar_sp_htab(b: u8) -> bool {
    (0x21..=0x7E).contains(&b) || b == b' ' || b == b'\t'
}

fn trim_ows(value: &[u8]) -> &[u8] {
    let is_ows = |b: &u8| *b == b' ' || *b == b'\t';
    let start = value.iter().position(|b| !is_ows(b)).unwrap_or(value.len());
    let end = value.iter().rposition(|b| !is_ows(b)).map_or(start, |p| p + 1);
    &value[start..end]
}

fn parse_content_length(value: &[u8]) -> Result<u64, Error> {
    if value.is_empty() || value.len() > 20 || !value.iter().all(u8::is_ascii_digit) {
        return Err(Error::Protocol("malformed Content-Length"));
    }
    std::str::from_utf8(value)
        .expect("checked all-ASCII-digit above")
        .parse::<u64>()
        .map_err(|_| Error::Protocol("Content-Length overflow"))
}

/// Parses one `[extension...]<size-hex>\r\n` chunk-size line from the start
/// of `buf`. Chunk extensions (`;name=value`) are rejected rather than
/// supported — the S3 workload never sends them, and rejecting shrinks the
/// parser's attack surface.
///
/// `Ok(None)` = incomplete, need more bytes. `Ok(Some((size, consumed)))` =
/// a full line was found; `consumed` bytes (including the trailing CRLF)
/// should be dropped from the read buffer.
pub(crate) fn parse_chunk_size_line(buf: &[u8]) -> Result<Option<(u64, usize)>, Error> {
    let search_limit = buf.len().min(MAX_CHUNK_HEADER_LINE);
    let nl = match buf[..search_limit].iter().position(|&b| b == b'\n') {
        Some(pos) => pos,
        None => {
            if buf.len() >= MAX_CHUNK_HEADER_LINE {
                return Err(Error::Protocol("chunk size line too large"));
            }
            return Ok(None);
        }
    };
    if nl == 0 || buf[nl - 1] != b'\r' {
        return Err(Error::Protocol("bare LF in chunk size line"));
    }
    let line = &buf[..nl - 1];
    if line.contains(&b'\r') {
        return Err(Error::Protocol("bare CR in chunk size line"));
    }
    if line.contains(&b';') {
        return Err(Error::Protocol("chunk extensions not supported"));
    }
    if line.is_empty() || line.len() > 16 || !line.iter().all(u8::is_ascii_hexdigit) {
        return Err(Error::Protocol("invalid chunk size"));
    }
    let size = u64::from_str_radix(
        std::str::from_utf8(line).expect("checked all-ASCII-hex above"),
        16,
    )
    .map_err(|_| Error::Protocol("chunk size overflow"))?;
    Ok(Some((size, nl + 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits::default()
    }

    fn head(bytes: &[u8]) -> Result<Option<Head>, Error> {
        parse_head(bytes, &limits())
    }

    fn assert_protocol(bytes: &[u8], needle: &str) {
        match head(bytes) {
            Err(Error::Protocol(msg)) => {
                assert!(msg.contains(needle), "expected rule mentioning {needle:?}, got {msg:?}");
            }
            other => panic!("expected Protocol error containing {needle:?}, got {other:?}"),
        }
    }

    #[test]
    fn bare_lf_line_endings_rejected() {
        assert_protocol(b"HTTP/1.1 200 OK\nContent-Length: 0\r\n\r\n", "bare LF");
    }

    #[test]
    fn duplicate_content_length_identical_values_rejected() {
        assert_protocol(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\n",
            "duplicate Content-Length",
        );
    }

    #[test]
    fn content_length_comma_list_rejected() {
        assert_protocol(b"HTTP/1.1 200 OK\r\nContent-Length: 5, 5\r\n\r\n", "malformed Content-Length");
    }

    #[test]
    fn content_length_malformed_variants_rejected() {
        for value in ["+5", "-5", "0x5", "5abc", "5 5", ""] {
            let req = format!("HTTP/1.1 200 OK\r\nContent-Length: {value}\r\n\r\n");
            assert_protocol(req.as_bytes(), "Content-Length");
        }
    }

    /// Rule 8 explicitly permits OWS around the digits ("1-20 ASCII digits
    /// after optional OWS") — this is standard RFC 7230 field-value framing,
    /// not a malformed value once trimmed.
    #[test]
    fn content_length_surrounding_ows_is_accepted() {
        let h = head(b"HTTP/1.1 200 OK\r\nContent-Length:  5 \r\n\r\n").unwrap().unwrap();
        assert_eq!(h.framing, BodyFraming::ContentLength(5));
    }

    #[test]
    fn content_length_overflow_rejected() {
        assert_protocol(
            b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999999\r\n\r\n",
            "Content-Length",
        );
    }

    #[test]
    fn transfer_encoding_chunked_is_accepted() {
        let h = head(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .unwrap()
            .unwrap();
        assert_eq!(h.framing, BodyFraming::Chunked);
    }

    #[test]
    fn transfer_encoding_identity_rejected() {
        assert_protocol(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: identity\r\n\r\n",
            "unsupported Transfer-Encoding",
        );
    }

    #[test]
    fn transfer_encoding_gzip_rejected() {
        assert_protocol(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
            "unsupported Transfer-Encoding",
        );
    }

    #[test]
    fn duplicate_transfer_encoding_rejected() {
        assert_protocol(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n",
            "duplicate Transfer-Encoding",
        );
    }

    #[test]
    fn content_length_and_transfer_encoding_together_rejected() {
        assert_protocol(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n",
            "Content-Length and Transfer-Encoding together",
        );
    }

    #[test]
    fn space_before_colon_rejected() {
        assert_protocol(b"HTTP/1.1 200 OK\r\nContent-Length : 5\r\n\r\n", "invalid header name");
    }

    #[test]
    fn obs_fold_continuation_rejected() {
        assert_protocol(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n  extra\r\n\r\n",
            "obs-fold",
        );
    }

    #[test]
    fn header_name_with_nul_space_paren_rejected() {
        assert_protocol(b"HTTP/1.1 200 OK\r\nBad Name: 1\r\n\r\n", "invalid header name");
        assert_protocol(b"HTTP/1.1 200 OK\r\nBad(Name: 1\r\n\r\n", "invalid header name");
        assert_protocol(b"HTTP/1.1 200 OK\r\nBad\x00Name: 1\r\n\r\n", "invalid header name");
    }

    #[test]
    fn header_value_with_embedded_cr_rejected() {
        assert_protocol(b"HTTP/1.1 200 OK\r\nX: a\rb\r\n\r\n", "bare CR in header line");
    }

    #[test]
    fn content_encoding_gzip_rejected() {
        assert_protocol(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Encoding: gzip\r\n\r\n",
            "unsupported Content-Encoding",
        );
    }

    #[test]
    fn bad_protocol_tokens_rejected() {
        assert_protocol(b"HTTP/2 200 OK\r\n\r\n", "protocol token");
        assert_protocol(b"HTTP/1.1 20 OK\r\n\r\n", "non-digit status code");
        assert_protocol(b"HTTP/1.1 2000 OK\r\n\r\n", "malformed reason phrase separator");
        assert_protocol(b"ICAP/1.0 200 OK\r\n\r\n", "protocol token");
    }

    #[test]
    fn continue_status_rejected() {
        assert_protocol(b"HTTP/1.1 100 Continue\r\n\r\n", "1xx status");
    }

    #[test]
    fn no_content_status_without_content_length_ok_empty_body() {
        for status in [204, 304] {
            let req = format!("HTTP/1.1 {status} OK\r\n\r\n");
            let h = head(req.as_bytes()).unwrap().unwrap();
            assert_eq!(h.status, status);
            assert_eq!(h.framing, BodyFraming::None);
        }
    }

    #[test]
    fn content_length_one_byte_over_limit_rejected_no_allocation() {
        let limit = Limits::default().max_body_bytes;
        let req = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", limit + 1);
        match parse_head(req.as_bytes(), &Limits::default()) {
            Err(Error::BodyTooLarge { len, limit: lim }) => {
                assert_eq!(len, limit + 1);
                assert_eq!(lim, limit);
            }
            other => panic!("expected BodyTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn head_one_byte_over_max_head_bytes_rejected() {
        let limits = Limits { max_head_bytes: 64, ..Limits::default() };
        let mut req = b"HTTP/1.1 200 OK\r\n".to_vec();
        while req.len() < 64 {
            req.extend_from_slice(b"X: 1\r\n");
        }
        req.extend_from_slice(b"\r\n");
        assert!(req.len() > 64);
        assert!(matches!(parse_head(&req, &limits), Err(Error::Protocol("head too large"))));
    }

    #[test]
    fn too_many_headers_rejected() {
        let limits = Limits { max_header_count: 128, ..Limits::default() };
        let mut req = b"HTTP/1.1 200 OK\r\n".to_vec();
        for _ in 0..129 {
            req.extend_from_slice(b"X: 1\r\n");
        }
        req.extend_from_slice(b"\r\n");
        assert!(matches!(parse_head(&req, &limits), Err(Error::Protocol("too many headers"))));
    }

    #[test]
    fn connection_close_variants_detected() {
        for value in ["close", "keep-alive, close", "CLOSE"] {
            let req = format!("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: {value}\r\n\r\n");
            let h = head(req.as_bytes()).unwrap().unwrap();
            assert!(h.connection_close, "expected connection_close for Connection: {value}");
        }
    }

    #[test]
    fn well_formed_200_parses_correctly() {
        let req = b"HTTP/1.1 200 OK\r\nContent-Length: 42\r\n\r\n";
        let h = head(req).unwrap().unwrap();
        assert_eq!(h.status, 200);
        assert_eq!(h.framing, BodyFraming::ContentLength(42));
        assert_eq!(h.head_len, req.len());
    }

    #[test]
    fn incremental_prefixes_return_none_until_complete() {
        let full = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n";
        for n in 1..full.len() {
            assert_eq!(head(&full[..n]).unwrap(), None, "prefix of length {n} should be incomplete");
        }
        let h = head(full).unwrap().unwrap();
        assert_eq!(h.head_len, full.len());
    }

    #[test]
    fn chunk_size_line_parses_hex_size() {
        assert_eq!(parse_chunk_size_line(b"1a\r\nrest").unwrap(), Some((0x1a, 4)));
        assert_eq!(parse_chunk_size_line(b"0\r\n\r\n").unwrap(), Some((0, 3)));
    }

    #[test]
    fn chunk_size_line_incomplete_returns_none() {
        assert_eq!(parse_chunk_size_line(b"1a").unwrap(), None);
    }

    #[test]
    fn chunk_size_line_rejects_extensions() {
        assert!(matches!(
            parse_chunk_size_line(b"1a;foo=bar\r\n"),
            Err(Error::Protocol("chunk extensions not supported"))
        ));
    }

    #[test]
    fn chunk_size_line_rejects_bare_lf_and_non_hex() {
        assert!(matches!(parse_chunk_size_line(b"1a\nrest"), Err(Error::Protocol(_))));
        assert!(matches!(parse_chunk_size_line(b"zz\r\n"), Err(Error::Protocol(_))));
        assert!(matches!(parse_chunk_size_line(b"\r\n"), Err(Error::Protocol(_))));
    }

    fn corpus_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/corpus/parse_response")
    }

    fn read_corpus() -> Vec<Vec<u8>> {
        let dir = corpus_dir();
        std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("could not read corpus dir {dir:?}: {e}"))
            .map(|entry| std::fs::read(entry.expect("readable dir entry").path()).expect("readable corpus file"))
            .collect()
    }

    /// Corpus regressions fail plain `cargo test`, not just the (nightly,
    /// manual) fuzzer — see D-e in the implementation plan.
    #[test]
    fn fuzz_corpus_never_panics() {
        let limits = Limits::default();
        let corpus = read_corpus();
        assert!(!corpus.is_empty(), "corpus directory should not be empty");
        for data in &corpus {
            let _ = parse_head(data, &limits); // must not panic
        }
    }

    /// Minimal xorshift64 PRNG — no dependency, and a fixed seed makes any
    /// failure reproducible rather than a flake.
    struct XorShift64(u64);

    impl XorShift64 {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    #[test]
    fn mutated_inputs_never_panic() {
        let limits = Limits::default();
        let seeds = read_corpus();
        assert!(!seeds.is_empty());

        let mut rng = XorShift64(0x9E37_79B9_7F4A_7C15);
        for _ in 0..5000 {
            let seed = &seeds[(rng.next() as usize) % seeds.len()];
            if seed.is_empty() {
                continue;
            }
            let mut mutated = seed.clone();
            match rng.next() % 3 {
                0 => {
                    // bit flip
                    let idx = (rng.next() as usize) % mutated.len();
                    let bit = (rng.next() % 8) as u8;
                    mutated[idx] ^= 1 << bit;
                }
                1 => {
                    // byte splice from another seed
                    let other = &seeds[(rng.next() as usize) % seeds.len()];
                    if !other.is_empty() {
                        let src_idx = (rng.next() as usize) % other.len();
                        let dst_idx = (rng.next() as usize) % mutated.len();
                        mutated[dst_idx] = other[src_idx];
                    }
                }
                _ => {
                    // truncation
                    let new_len = (rng.next() as usize) % (mutated.len() + 1);
                    mutated.truncate(new_len);
                }
            }
            let _ = parse_head(&mutated, &limits); // must not panic
        }
    }
}
