use radar_workstation::{download_sample, AcquisitionError};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;
use tempfile::tempdir;

fn start_test_server(status: u16, body: &[u8]) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let address = listener.local_addr().expect("listener should report an address");
    let body = body.to_vec();

    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("server should accept a connection");
        let mut buffer = [0_u8; 4096];
        let _ = stream.read(&mut buffer).expect("server should read request bytes");

        let response = format!(
            "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/octet-stream\r\n\r\n",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("server should write response headers");
        stream
            .write_all(&body)
            .expect("server should write response body");
    });

    (format!("http://{address}"), handle)
}

fn create_output_path(name: &str) -> (PathBuf, tempfile::TempDir) {
    let temp_dir = tempdir().expect("temp dir should be created");
    let output_path = temp_dir.path().join(name);
    (output_path, temp_dir)
}

fn assert_output_file_does_not_exist(output_path: &PathBuf) {
    assert!(!output_path.exists(), "output file should not be created on failure");
}

#[tokio::test]
async fn download_sample_writes_a_file_for_a_successful_response() {
    let (url, handle) = start_test_server(200, b"radar-data");
    let (output_path, _temp_dir) = create_output_path("sample.bin");

    let result = download_sample(&url, &output_path).await;

    assert!(result.is_ok(), "download should succeed for a 200 response");
    assert_eq!(result.unwrap(), output_path);
    assert!(output_path.exists());
    assert_eq!(fs::read(&output_path).expect("file should be readable"), b"radar-data");

    handle.join().expect("test server thread should finish");
}

#[tokio::test]
async fn download_sample_returns_an_error_for_a_non_success_status() {
    let (url, handle) = start_test_server(404, b"");
    let (output_path, _temp_dir) = create_output_path("missing.bin");

    let result = download_sample(&url, &output_path).await;

    assert_eq!(result.unwrap_err(), AcquisitionError::BadStatusCode(404));
    assert_output_file_does_not_exist(&output_path);

    handle.join().expect("test server thread should finish");
}

#[tokio::test]
async fn download_sample_returns_an_error_for_an_empty_body() {
    let (url, handle) = start_test_server(200, b"");
    let (output_path, _temp_dir) = create_output_path("empty.bin");

    let result = download_sample(&url, &output_path).await;

    assert_eq!(result.unwrap_err(), AcquisitionError::EmptyResponse);
    assert_output_file_does_not_exist(&output_path);

    handle.join().expect("test server thread should finish");
}

#[tokio::test]
async fn download_sample_returns_an_error_for_invalid_urls() {
    let (output_path, _temp_dir) = create_output_path("invalid.bin");

    let result = download_sample("not a valid url", &output_path).await;

    assert!(matches!(result.unwrap_err(), AcquisitionError::InvalidUrl(_)));
    assert_output_file_does_not_exist(&output_path);
}
