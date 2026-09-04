use crate::data_acquisition::AcquisitionError;

/// Splits a `https://<host>/<key>` sample URL into its host and key.
/// Only syntax is validated here — host allowlist validation is delegated to
/// `http_ingest::Bucket::from_host`, not duplicated.
pub fn split_s3_url(url: &str) -> Result<(&str, &str), AcquisitionError> {
    let rest = match url.strip_prefix("https://") {
        Some(rest) => rest,
        None if url.starts_with("http://") => {
            return Err(AcquisitionError::InvalidUrl(
                "http:// is not supported; use https://".to_string(),
            ));
        }
        None => return Err(AcquisitionError::InvalidUrl(format!("missing https:// scheme: {url}"))),
    };

    let slash = rest
        .find('/')
        .ok_or_else(|| AcquisitionError::InvalidUrl(format!("URL must contain a path separating host from key: {url}")))?;

    let host = &rest[..slash];
    let key = &rest[slash + 1..];

    if host.is_empty() {
        return Err(AcquisitionError::InvalidUrl(format!("empty host: {url}")));
    }
    if key.is_empty() {
        return Err(AcquisitionError::InvalidUrl(format!("empty key: {url}")));
    }

    Ok((host, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_well_formed_url() {
        let (host, key) = split_s3_url("https://unidata-nexrad-level2-chunks.s3.amazonaws.com/KDOX/1/20260727-164425-001-S").unwrap();
        assert_eq!(host, "unidata-nexrad-level2-chunks.s3.amazonaws.com");
        assert_eq!(key, "KDOX/1/20260727-164425-001-S");
    }

    #[test]
    fn rejects_http_scheme_with_a_distinct_message() {
        let err = split_s3_url("http://example.com/key").unwrap_err();
        match err {
            AcquisitionError::InvalidUrl(msg) => assert!(msg.contains("https://"), "message was: {msg}"),
            other => panic!("expected InvalidUrl, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_scheme() {
        assert!(matches!(split_s3_url("example.com/key"), Err(AcquisitionError::InvalidUrl(_))));
    }

    #[test]
    fn rejects_missing_path_separator() {
        assert!(matches!(split_s3_url("https://example.com"), Err(AcquisitionError::InvalidUrl(_))));
    }

    #[test]
    fn rejects_empty_host() {
        assert!(matches!(split_s3_url("https:///key"), Err(AcquisitionError::InvalidUrl(_))));
    }

    #[test]
    fn rejects_empty_key() {
        assert!(matches!(split_s3_url("https://example.com/"), Err(AcquisitionError::InvalidUrl(_))));
    }

    #[test]
    fn does_not_itself_reject_a_non_allowlisted_host() {
        // Syntax only — allowlist enforcement happens in Bucket::from_host.
        let (host, key) = split_s3_url("https://evil.com/key").unwrap();
        assert_eq!(host, "evil.com");
        assert_eq!(key, "key");
    }
}
