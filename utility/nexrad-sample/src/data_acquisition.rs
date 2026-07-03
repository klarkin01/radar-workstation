use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum AcquisitionError {
    HttpFailure(String),
    BadStatusCode(u16),
    EmptyResponse,
    Io(String),
    InvalidUrl(String),
}

impl fmt::Display for AcquisitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HttpFailure(message) => write!(f, "HTTP failure: {message}"),
            Self::BadStatusCode(status) => write!(f, "unexpected HTTP status code: {status}"),
            Self::EmptyResponse => write!(f, "downloaded response body was empty"),
            Self::Io(message) => write!(f, "I/O error: {message}"),
            Self::InvalidUrl(url) => write!(f, "invalid URL: {url}"),
        }
    }
}

impl Error for AcquisitionError {}

pub async fn download_sample(url: &str, output_path: &Path) -> Result<PathBuf, AcquisitionError> {
    let parsed_url = reqwest::Url::parse(url).map_err(|error| AcquisitionError::InvalidUrl(error.to_string()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| AcquisitionError::HttpFailure(error.to_string()))?;

    let response = client
        .get(parsed_url)
        .send()
        .await
        .map_err(|error| AcquisitionError::HttpFailure(error.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(AcquisitionError::BadStatusCode(status.as_u16()));
    }

    let body = response
        .bytes()
        .await
        .map_err(|error| AcquisitionError::HttpFailure(error.to_string()))?;

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
