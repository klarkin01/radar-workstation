use crate::error::Error;

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn reject_crlf(input: &str) -> Result<(), Error> {
    if input.contains(['\r', '\n']) {
        return Err(Error::InvalidInput("CR or LF in input"));
    }
    Ok(())
}

fn percent_encode(input: &str, extra_pass_through: impl Fn(u8) -> bool) -> Result<String, Error> {
    reject_crlf(input)?;
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if is_unreserved(b) || extra_pass_through(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(out)
}

/// Encodes strictly the RFC 3986 unreserved set. This is what makes the
/// base64 continuation token (`=`, `+`, `/`) correct.
pub fn encode_query_value(input: &str) -> Result<String, Error> {
    percent_encode(input, |_| false)
}

/// Same as [`encode_query_value`], but `/` passes through as a path
/// separator — NEXRAD keys are entirely unreserved plus `/`.
pub fn encode_path(input: &str) -> Result<String, Error> {
    percent_encode(input, |b| b == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreserved_bytes_pass_through_unchanged() {
        let s = "abcXYZ019-._~";
        assert_eq!(encode_query_value(s).unwrap(), s);
    }

    #[test]
    fn continuation_token_special_chars_are_encoded() {
        assert_eq!(encode_query_value("=").unwrap(), "%3D");
        assert_eq!(encode_query_value("+").unwrap(), "%2B");
        assert_eq!(encode_query_value("/").unwrap(), "%2F");
    }

    #[test]
    fn real_base64_continuation_token_round_trips() {
        let token = "11bmUQNS1mQxvNMMpletE+t9k5AoLT8vVt/lK5ijRK1lkB8mSCkCRYcXv3MCbMiiH/rLFZmV7ncCa07tpGEYJmcYzYF/tx3jh";
        let expected = "11bmUQNS1mQxvNMMpletE%2Bt9k5AoLT8vVt%2FlK5ijRK1lkB8mSCkCRYcXv3MCbMiiH%2FrLFZmV7ncCa07tpGEYJmcYzYF%2Ftx3jh";
        assert_eq!(encode_query_value(token).unwrap(), expected);
    }

    #[test]
    fn path_encoding_preserves_slash_encodes_special_and_non_ascii() {
        let key = "KDOX/2026/07/29/00/file name#1?%.bin";
        let encoded = encode_path(key).unwrap();
        assert_eq!(
            encoded,
            "KDOX/2026/07/29/00/file%20name%231%3F%25.bin"
        );

        // non-ASCII UTF-8 bytes get percent-encoded byte-by-byte
        let encoded = encode_path("café").unwrap();
        assert_eq!(encoded, "caf%C3%A9");
    }

    #[test]
    fn cr_or_lf_in_input_is_rejected() {
        assert!(matches!(encode_query_value("a\rb"), Err(Error::InvalidInput(_))));
        assert!(matches!(encode_query_value("a\nb"), Err(Error::InvalidInput(_))));
        assert!(matches!(encode_path("a\r\nb"), Err(Error::InvalidInput(_))));
    }

    #[test]
    fn hex_digits_are_uppercase() {
        assert_eq!(encode_query_value("\u{1}").unwrap(), "%01");
        assert_eq!(encode_query_value(":").unwrap(), "%3A");
    }
}
