use radar_workstation::{download_sample, resolve_sample_url};
use reqwest::Url;
use std::{env, process};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let sample_url = resolve_sample_url(env::args().skip(1)).ok_or_else(|| {
        "No sample URL provided. Use --sample-url <URL> or set RADAR_SAMPLE_URL.".to_string()
    })?;

    let parsed_url = Url::parse(&sample_url)
        .map_err(|error| format!("invalid sample URL: {error}"))?;

    let filename = parsed_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| "sample URL must contain a filename".to_string())?;

    let output_dir = env::current_dir()
        .map_err(|error| format!("could not resolve current directory: {error}"))?
        .join("downloads");

    tokio::fs::create_dir_all(&output_dir)
        .await
        .map_err(|error| format!("could not create downloads directory: {error}"))?;

    let output_path = output_dir.join(filename);

    let resolved_path = download_sample(&sample_url, &output_path)
        .await
        .map_err(|error| format!("download failed: {error}"))?;

    println!("Downloaded sample to {}", resolved_path.display());
    Ok(())
}
