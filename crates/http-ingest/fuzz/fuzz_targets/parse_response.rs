#![no_main]

use http_ingest::response::{parse_head, BodyFraming};
use http_ingest::Limits;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    if let Ok(Some(head)) = parse_head(data, &limits) {
        assert!(head.head_len <= data.len());
        if let BodyFraming::ContentLength(len) = head.framing {
            assert!(len <= limits.max_body_bytes);
        }
    }
});
