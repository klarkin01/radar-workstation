use crate::encode::{encode_path, encode_query_value};
use crate::error::Error;

/// Builds the path+query for `GET /?list-type=2&prefix=…`.
///
/// `start_after` and `continuation_token` are mutually exclusive at the S3
/// API level (the caller is responsible for that invariant — see
/// `S3Poller::list_all_keys_after`'s `first_page` logic).
///
/// `delimiter` requests `<CommonPrefixes>` grouping instead of (or in
/// addition to) flat `<Contents>` — used by `S3Poller::list_volume_folders`
/// to enumerate the chunk bucket's volume-sequence subdirectories without
/// paging through every key in them.
pub fn list_query(
    prefix: &str,
    start_after: Option<&str>,
    continuation_token: Option<&str>,
    delimiter: Option<&str>,
) -> Result<String, Error> {
    let mut out = String::from("/?list-type=2&prefix=");
    out.push_str(&encode_query_value(prefix)?);
    if let Some(start_after) = start_after {
        out.push_str("&start-after=");
        out.push_str(&encode_query_value(start_after)?);
    }
    if let Some(token) = continuation_token {
        out.push_str("&continuation-token=");
        out.push_str(&encode_query_value(token)?);
    }
    if let Some(delimiter) = delimiter {
        out.push_str("&delimiter=");
        out.push_str(&encode_query_value(delimiter)?);
    }
    Ok(out)
}

/// Builds the path for `GET /<key>`.
pub fn object_path(key: &str) -> Result<String, Error> {
    Ok(format!("/{}", encode_path(key)?))
}

/// Serializes a full HTTP/1.1 GET request. `path_and_query` must already be
/// percent-encoded (see [`list_query`] / [`object_path`]).
pub fn format_request(host: &str, path_and_query: &str) -> Vec<u8> {
    format!(
        "GET {path_and_query} HTTP/1.1\r\n\
         Host: {host}\r\n\
         User-Agent: radar-workstation/{}\r\n\
         Accept: */*\r\n\
         Connection: keep-alive\r\n\
         \r\n",
        env!("CARGO_PKG_VERSION")
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "unidata-nexrad-level2-chunks.s3.amazonaws.com";

    fn golden(path_and_query: &str) -> String {
        format!(
            "GET {path_and_query} HTTP/1.1\r\n\
             Host: {HOST}\r\n\
             User-Agent: radar-workstation/{}\r\n\
             Accept: */*\r\n\
             Connection: keep-alive\r\n\
             \r\n",
            env!("CARGO_PKG_VERSION")
        )
    }

    #[test]
    fn list_prefix_no_pagination_args() {
        let pq = list_query("KDOX/", None, None, None).unwrap();
        assert_eq!(pq, "/?list-type=2&prefix=KDOX%2F");
        let bytes = format_request(HOST, &pq);
        assert_eq!(String::from_utf8(bytes).unwrap(), golden(&pq));
    }

    #[test]
    fn list_prefix_with_start_after() {
        let pq = list_query("KDOX/", Some("KDOX/166/20260728-095259-005-I"), None, None).unwrap();
        assert_eq!(
            pq,
            "/?list-type=2&prefix=KDOX%2F&start-after=KDOX%2F166%2F20260728-095259-005-I"
        );
        let bytes = format_request(HOST, &pq);
        assert_eq!(String::from_utf8(bytes).unwrap(), golden(&pq));
    }

    #[test]
    fn list_prefix_with_continuation_token() {
        let pq = list_query("KDOX/", None, Some("abc+/="), None).unwrap();
        assert_eq!(pq, "/?list-type=2&prefix=KDOX%2F&continuation-token=abc%2B%2F%3D");
        let bytes = format_request(HOST, &pq);
        assert_eq!(String::from_utf8(bytes).unwrap(), golden(&pq));
    }

    #[test]
    fn list_prefix_with_delimiter() {
        let pq = list_query("KDOX/", None, None, Some("/")).unwrap();
        assert_eq!(pq, "/?list-type=2&prefix=KDOX%2F&delimiter=%2F");
        let bytes = format_request(HOST, &pq);
        assert_eq!(String::from_utf8(bytes).unwrap(), golden(&pq));
    }

    #[test]
    fn list_prefix_empty_prefix() {
        let pq = list_query("", None, None, None).unwrap();
        assert_eq!(pq, "/?list-type=2&prefix=");
        let bytes = format_request(HOST, &pq);
        assert_eq!(String::from_utf8(bytes).unwrap(), golden(&pq));
    }

    #[test]
    fn get_object_golden_request() {
        let path = object_path("KDOX/2026/07/29/00/KDOX_20260729_000248_S").unwrap();
        assert_eq!(path, "/KDOX/2026/07/29/00/KDOX_20260729_000248_S");
        let bytes = format_request(HOST, &path);
        assert_eq!(String::from_utf8(bytes).unwrap(), golden(&path));
    }

    #[test]
    fn accept_encoding_is_absent() {
        let bytes = format_request(HOST, "/x");
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.to_ascii_lowercase().contains("accept-encoding"));
    }

    #[test]
    fn exactly_one_host_header_and_keepalive() {
        let bytes = format_request(HOST, "/x");
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.matches("Host:").count(), 1);
        assert!(text.contains("Connection: keep-alive\r\n"));
    }

    #[test]
    fn key_requiring_encoding_produces_correct_request_line() {
        let path = object_path("weird key#1?.bin").unwrap();
        assert_eq!(path, "/weird%20key%231%3F.bin");
        let bytes = format_request(HOST, &path);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("GET /weird%20key%231%3F.bin HTTP/1.1\r\n"));
    }
}
