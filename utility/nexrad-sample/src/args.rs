use std::env;

pub fn resolve_sample_url<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut values = args.into_iter().map(|value| value.as_ref().to_string());

    while let Some(argument) = values.next() {
        if argument == "--sample-url" {
            return values.next();
        }

        if let Some(value) = argument.strip_prefix("--sample-url=") {
            return Some(value.to_string());
        }
    }

    env::var("RADAR_SAMPLE_URL").ok()
}
