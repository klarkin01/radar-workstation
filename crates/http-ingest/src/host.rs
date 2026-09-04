/// The two ADR-0011 S3 buckets the radar ingest path ever touches — the
/// chunk stream and the archive. A closed set makes "this client can only
/// reach a NEXRAD S3 bucket" a compile-time property of `S3Client::new`
/// (ADR-0026 §2) rather than a runtime allowlist check: there is no
/// constructor left that accepts an arbitrary hostname.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Chunks,
    Archive,
}

impl Bucket {
    pub fn host(self) -> &'static str {
        match self {
            Self::Chunks => "unidata-nexrad-level2-chunks.s3.amazonaws.com",
            Self::Archive => "unidata-nexrad-level2.s3.amazonaws.com",
        }
    }

    /// Map a hostname onto the closed set. Used only by
    /// `utility/nexrad-sample`, which takes a URL from the developer; the
    /// production path never calls it — every production `S3Client` is
    /// constructed from a `Bucket` variant, never a string.
    pub fn from_host(host: &str) -> Option<Bucket> {
        [Bucket::Chunks, Bucket::Archive].into_iter().find(|b| b.host() == host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_returns_the_two_adr_0011_hosts() {
        assert_eq!(Bucket::Chunks.host(), "unidata-nexrad-level2-chunks.s3.amazonaws.com");
        assert_eq!(Bucket::Archive.host(), "unidata-nexrad-level2.s3.amazonaws.com");
    }

    #[test]
    fn from_host_round_trips_each_bucket() {
        assert_eq!(Bucket::from_host(Bucket::Chunks.host()), Some(Bucket::Chunks));
        assert_eq!(Bucket::from_host(Bucket::Archive.host()), Some(Bucket::Archive));
    }

    #[test]
    fn from_host_rejects_an_unrelated_host() {
        assert_eq!(Bucket::from_host("evil.com"), None);
        assert_eq!(Bucket::from_host(""), None);
    }
}
