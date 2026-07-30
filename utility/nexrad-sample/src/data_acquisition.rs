use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::url::split_s3_url;

#[derive(Debug, PartialEq, Eq)]
pub enum AcquisitionError {
    HttpFailure(String),
    BadStatusCode(u16),
    NotAllowed(String),
    EmptyResponse,
    Io(String),
    InvalidUrl(String),
}

impl fmt::Display for AcquisitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HttpFailure(message) => write!(f, "HTTP failure: {message}"),
            Self::BadStatusCode(status) => write!(f, "unexpected HTTP status code: {status}"),
            Self::NotAllowed(host) => write!(f, "host not allowed: {host}"),
            Self::EmptyResponse => write!(f, "downloaded response body was empty"),
            Self::Io(message) => write!(f, "I/O error: {message}"),
            Self::InvalidUrl(url) => write!(f, "invalid URL: {url}"),
        }
    }
}

impl Error for AcquisitionError {}

fn map_client_error(error: http_ingest::Error) -> AcquisitionError {
    match error {
        http_ingest::Error::HostNotAllowed(host) => AcquisitionError::NotAllowed(host),
        http_ingest::Error::Http { status } => AcquisitionError::BadStatusCode(status),
        other => AcquisitionError::HttpFailure(other.to_string()),
    }
}

pub async fn download_sample(url: &str, output_path: &Path) -> Result<PathBuf, AcquisitionError> {
    let (host, key) = split_s3_url(url)?;

    let mut client = http_ingest::Client::new(host).map_err(map_client_error)?;
    let body = client.get_object(key).await.map_err(map_client_error)?;

    if body.is_empty() {
        return Err(AcquisitionError::EmptyResponse);
    }

    if let Some(parent) = output_path.parent().filter(|path| !path.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| AcquisitionError::Io(error.to_string()))?;
    }

    let temp_path = output_path.with_extension(format!(
        "{}.tmp",
        output_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin")
    ));

    tokio::fs::write(&temp_path, &body)
        .await
        .map_err(|error| AcquisitionError::Io(error.to_string()))?;

    match tokio::fs::rename(&temp_path, output_path).await {
        Ok(()) => Ok(output_path.to_path_buf()),
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            Err(AcquisitionError::Io(error.to_string()))
        }
    }
}
