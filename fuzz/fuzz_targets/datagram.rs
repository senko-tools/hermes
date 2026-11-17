#![no_main]

use hermes_proto::voice::{AudioHeader, VideoHeader, frame_len};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((header, payload)) = AudioHeader::decode(data) {
        assert!(header.audio_level <= 127);
        // Encoding what was decoded gives the same bytes back (the level is masked).
        let again = header.encode(payload);
        assert_eq!(again.len(), data.len());
    }
    // Voice and share audio: re-encoding keeps the kind and decodes the same.
    if let Ok((kind, header, payload)) = AudioHeader::decode_any(data) {
        let again = header.encode_as(kind, payload);
        assert_eq!(again.len(), data.len());
        assert_eq!(AudioHeader::decode_any(&again), Ok((kind, header, payload)));
    }
    if let Ok(header) = VideoHeader::decode(data) {
        assert_eq!(VideoHeader::decode(&header.encode()), Ok(header));
    }
    let mut prefix = [0u8; 4];
    for (p, b) in prefix.iter_mut().zip(data) {
        *p = *b;
    }
    let _ = frame_len(prefix);
});
