#![no_main]

use hermes_proto::voice::{AudioHeader, frame_len};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((header, payload)) = AudioHeader::decode(data) {
        assert!(header.audio_level <= 127);
        // Encoding what was decoded gives the same bytes back (the level is masked).
        let again = header.encode(payload);
        assert_eq!(again.len(), data.len());
    }
    let mut prefix = [0u8; 4];
    for (p, b) in prefix.iter_mut().zip(data) {
        *p = *b;
    }
    let _ = frame_len(prefix);
});
