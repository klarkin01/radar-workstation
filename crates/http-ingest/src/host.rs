use crate::error::Error;

pub const ALLOWED_HOSTS: &[&str] = &[
    "unidata-nexrad-level2-chunks.s3.amazonaws.com",
    "unidata-nexrad-level2.s3.amazonaws.com",
];

/// A validated hostname from the compile-time allowlist. Port is fixed at 443
/// and is never part of the input.
#[derive(Debug, Clone)]
pub struct Host(String);

impl Host {
    pub fn parse(input: &str) -> Result<Self, Error> {
        if input.contains(['@', ':', '/']) || input.ends_with('.') {
            return Err(Error::HostNotAllowed(input.to_string()));
        }
        match ALLOWED_HOSTS.iter().find(|&&h| h == input) {
            Some(&h) => Ok(Self(h.to_string())),
            None => Err(Error::HostNotAllowed(input.to_string())),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_each_allowlisted_host() {
        for host in ALLOWED_HOSTS {
            assert!(Host::parse(host).is_ok(), "expected {host} to be accepted");
        }
    }

    #[test]
    fn rejects_case_variant() {
        assert!(Host::parse("UNIDATA-NEXRAD-LEVEL2-CHUNKS.s3.amazonaws.com").is_err());
    }

    #[test]
    fn rejects_trailing_dot() {
        assert!(Host::parse("unidata-nexrad-level2-chunks.s3.amazonaws.com.").is_err());
    }

    #[test]
    fn rejects_unrelated_host() {
        assert!(Host::parse("evil.com").is_err());
    }

    #[test]
    fn rejects_suffix_match_attempt() {
        assert!(Host::parse("unidata-nexrad-level2-chunks.s3.amazonaws.com.evil.com").is_err());
    }

    #[test]
    fn rejects_prefix_match_attempt() {
        assert!(Host::parse("evil.unidata-nexrad-level2-chunks.s3.amazonaws.com").is_err());
    }

    #[test]
    fn rejects_embedded_port() {
        assert!(Host::parse("unidata-nexrad-level2-chunks.s3.amazonaws.com:443").is_err());
    }

    #[test]
    fn rejects_userinfo() {
        assert!(Host::parse("user@unidata-nexrad-level2-chunks.s3.amazonaws.com").is_err());
    }

    #[test]
    fn rejects_empty_string() {
        assert!(Host::parse("").is_err());
    }

    #[test]
    fn rejects_host_containing_slash() {
        assert!(Host::parse("unidata-nexrad-level2-chunks.s3.amazonaws.com/evil").is_err());
    }
}
