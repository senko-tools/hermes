#![no_main]

use hermes_proto::markup;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = markup::parse(text);
        let _ = markup::extract_mentions(text);
    }
});
