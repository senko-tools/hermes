//! Voice protocol (PLAN.md section 12): the QUIC control stream messages and their framing, the
//! audio datagram format and the screen share frame format. No I/O.

use serde::{Deserialize, Serialize};

use crate::id::Id;

/// ALPN of the voice QUIC endpoint.
pub const ALPN: &[u8] = b"hermes-voice/1";

/// Join tokens are valid this long (and single-use).
pub const TOKEN_LIFETIME_SECS: u64 = 30;

/// QUIC idle timeout and keep-alive interval (PLAN.md 12.2).
pub const IDLE_TIMEOUT_MS: u64 = 15_000;
pub const KEEP_ALIVE_MS: u64 = 5_000;

// ---------------------------------------------------------------------------------------------
// Control stream
// ---------------------------------------------------------------------------------------------

/// Largest control frame body either side accepts.
pub const MAX_CONTROL_FRAME: usize = 64 * 1024;

/// Client → server messages on the control stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", content = "d", rename_all = "snake_case")]
pub enum ClientControl {
    /// First message on the stream.
    Auth {
        token: String,
        self_mute: bool,
        self_deaf: bool,
    },
    /// Move this connection to another room (after `/voice/join` or a `moved`).
    Switch {
        token: String,
    },
    SetState {
        self_mute: bool,
        self_deaf: bool,
    },
    /// `t` is echoed back in `pong`.
    Ping {
        t: u64,
    },
    Leave {},
    /// I start sharing my screen (`STREAM`); frames follow on unidirectional streams.
    ShareStart {},
    ShareStop {},
    /// Start (or stop) receiving someone's screen share.
    Watch {
        participant_id: u16,
        watching: bool,
    },
    /// Ask a sharer for a keyframe (a frame was lost or the picture is broken).
    RequestKeyframe {
        participant_id: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub participant_id: u16,
    pub user_id: Id,
}

/// Server → client messages on the control stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", content = "d", rename_all = "snake_case")]
pub enum ServerControl {
    /// Answer to `auth` and `switch`. `participants` are the others already in the room.
    Ready {
        participant_id: u16,
        room_id: Id,
        bitrate: u32,
        participants: Vec<Participant>,
        /// Those of `participants` sharing their screen.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sharing: Vec<u16>,
    },
    ParticipantJoined(Participant),
    ParticipantLeft {
        participant_id: u16,
    },
    ServerState {
        server_mute: bool,
        server_deaf: bool,
    },
    BitrateChanged {
        bitrate: u32,
    },
    /// A moderator moved you; `switch` with this token.
    Moved {
        token: String,
    },
    /// You are no longer in a room. The connection stays usable for a later `switch`.
    Kicked {
        reason: KickReason,
    },
    Pong {
        t: u64,
    },
    Error {
        code: String,
        message: String,
    },
    ShareStarted {
        participant_id: u16,
    },
    ShareStopped {
        participant_id: u16,
    },
    /// Someone started watching your share or lost a frame: send a keyframe next.
    KeyframeRequested {},
    /// How many members watch your share: sent after `share_start` and whenever it changes.
    /// While it is 0, frames go nowhere, so a sharer may stop capturing and encoding.
    ShareWatchers {
        count: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KickReason {
    /// The same user joined voice from another connection.
    JoinedElsewhere,
    /// A moderator disconnected you.
    Disconnected,
    /// The channel was deleted.
    ChannelDeleted,
    /// You can no longer see or connect to the channel (left or removed from the space).
    Removed,
}

/// `u32` big-endian length prefix + JSON body.
pub fn encode_frame<T: Serialize>(msg: &T) -> Vec<u8> {
    let body = serde_json::to_vec(msg).expect("control messages always serialize");
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// Reads a frame's length prefix; `None` if it exceeds [`MAX_CONTROL_FRAME`].
pub fn frame_len(prefix: [u8; 4]) -> Option<usize> {
    let len = u32::from_be_bytes(prefix) as usize;
    (len <= MAX_CONTROL_FRAME).then_some(len)
}

// ---------------------------------------------------------------------------------------------
// Audio datagrams (PLAN.md 12.4)
// ---------------------------------------------------------------------------------------------

pub const PACKET_VERSION: u8 = 1;
pub const PACKET_TYPE_AUDIO: u8 = 1;
pub const HEADER_LEN: usize = 11;
/// Datagrams larger than this are dropped by the server.
pub const MAX_DATAGRAM: usize = 1000;
/// 20 ms at 48 kHz.
pub const FRAME_SAMPLES: u32 = 960;
/// `audio_level` of silence.
pub const LEVEL_SILENT: u8 = 127;

const FLAG_VOICE_ACTIVE: u8 = 1;

/// The fixed header of an audio datagram; the Opus frame follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioHeader {
    pub voice_active: bool,
    /// The sender's participant ID. The server overwrites it.
    pub sender_id: u16,
    /// In 48 kHz samples; +960 per 20 ms frame.
    pub timestamp: u32,
    /// +1 per packet, wraps.
    pub sequence: u16,
    /// 0 = loud … 127 = silent (-dBov, as in RFC 6464).
    pub audio_level: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketError {
    TooShort,
    TooLarge,
    BadVersion(u8),
    UnknownType(u8),
}

impl AudioHeader {
    /// Writes the header and `opus` into one datagram.
    pub fn encode(&self, opus: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + opus.len());
        out.push((PACKET_VERSION << 4) | PACKET_TYPE_AUDIO);
        out.push(if self.voice_active {
            FLAG_VOICE_ACTIVE
        } else {
            0
        });
        out.extend_from_slice(&self.sender_id.to_be_bytes());
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.push(self.audio_level.min(LEVEL_SILENT));
        out.extend_from_slice(opus);
        out
    }

    /// Parses a datagram into its header and Opus payload.
    pub fn decode(data: &[u8]) -> Result<(AudioHeader, &[u8]), PacketError> {
        if data.len() > MAX_DATAGRAM {
            return Err(PacketError::TooLarge);
        }
        if data.len() < HEADER_LEN {
            return Err(PacketError::TooShort);
        }
        let version = data[0] >> 4;
        if version != PACKET_VERSION {
            return Err(PacketError::BadVersion(version));
        }
        let kind = data[0] & 0x0f;
        if kind != PACKET_TYPE_AUDIO {
            return Err(PacketError::UnknownType(kind));
        }
        let header = AudioHeader {
            voice_active: data[1] & FLAG_VOICE_ACTIVE != 0,
            sender_id: u16::from_be_bytes([data[2], data[3]]),
            timestamp: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            sequence: u16::from_be_bytes([data[8], data[9]]),
            audio_level: data[10] & 0x7f,
        };
        Ok((header, &data[HEADER_LEN..]))
    }
}

/// Overwrites the sender ID of an encoded datagram in place (what the server does).
pub fn set_sender_id(datagram: &mut [u8], sender_id: u16) {
    if datagram.len() >= 4 {
        datagram[2..4].copy_from_slice(&sender_id.to_be_bytes());
    }
}

/// The `audio_level` of a frame with this RMS (linear, full scale = 1.0).
pub fn audio_level(rms: f32) -> u8 {
    if rms.is_nan() || rms <= 0.0 {
        return LEVEL_SILENT;
    }
    let dbov = 20.0 * rms.log10();
    (-dbov).clamp(0.0, f32::from(LEVEL_SILENT)) as u8
}

/// Formats a SHA-256 digest as `"3F:A9:…"` (the form in `ready.voice` and `/voice/join`).
pub fn colon_hex(digest: &[u8]) -> String {
    let mut out = String::with_capacity(digest.len() * 3);
    for (i, byte) in digest.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Screen share frames
// ---------------------------------------------------------------------------------------------

pub const PACKET_TYPE_VIDEO: u8 = 2;
pub const VIDEO_HEADER_LEN: usize = 12;
/// Frames larger than this (header included) are dropped.
pub const MAX_VIDEO_FRAME: usize = 512 * 1024;
/// Largest width and height a share may have.
pub const MAX_VIDEO_DIMENSION: u16 = 3840;
/// A sharer sends at most this many frames per second; the server drops the rest.
pub const MAX_VIDEO_FPS: u32 = 30;

const FLAG_KEYFRAME: u8 = 1;

/// The header of a screen share frame. One frame (an H.264 access unit in Annex B form) is
/// sent per QUIC unidirectional stream: the header, then the bitstream, then the end of the
/// stream.
///
/// ```text
/// offset size field
/// 0      1    version_and_type   high nibble = version (1), low nibble = type (2 = video)
/// 1      1    flags              bit0 = keyframe
/// 2      2    sender_id          participant_id (the server overwrites it)
/// 4      2    width              pixels, even, 2..=3840
/// 6      2    height             pixels, even, 2..=3840
/// 8      4    timestamp          milliseconds since the share started
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoHeader {
    pub keyframe: bool,
    pub sender_id: u16,
    pub width: u16,
    pub height: u16,
    pub timestamp: u32,
}

impl VideoHeader {
    pub fn encode(&self) -> [u8; VIDEO_HEADER_LEN] {
        let mut out = [0u8; VIDEO_HEADER_LEN];
        out[0] = (PACKET_VERSION << 4) | PACKET_TYPE_VIDEO;
        out[1] = if self.keyframe { FLAG_KEYFRAME } else { 0 };
        out[2..4].copy_from_slice(&self.sender_id.to_be_bytes());
        out[4..6].copy_from_slice(&self.width.to_be_bytes());
        out[6..8].copy_from_slice(&self.height.to_be_bytes());
        out[8..12].copy_from_slice(&self.timestamp.to_be_bytes());
        out
    }

    /// Parses and checks a header (version, type, sizes).
    pub fn decode(data: &[u8]) -> Result<VideoHeader, PacketError> {
        if data.len() < VIDEO_HEADER_LEN {
            return Err(PacketError::TooShort);
        }
        let version = data[0] >> 4;
        if version != PACKET_VERSION {
            return Err(PacketError::BadVersion(version));
        }
        let kind = data[0] & 0x0f;
        if kind != PACKET_TYPE_VIDEO {
            return Err(PacketError::UnknownType(kind));
        }
        let header = VideoHeader {
            keyframe: data[1] & FLAG_KEYFRAME != 0,
            sender_id: u16::from_be_bytes([data[2], data[3]]),
            width: u16::from_be_bytes([data[4], data[5]]),
            height: u16::from_be_bytes([data[6], data[7]]),
            timestamp: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        };
        let size_ok = |n: u16| (2..=MAX_VIDEO_DIMENSION).contains(&n) && n.is_multiple_of(2);
        if !size_ok(header.width) || !size_ok(header.height) {
            return Err(PacketError::TooLarge);
        }
        Ok(header)
    }
}

/// Overwrites the sender ID of an encoded video header.
pub fn set_video_sender_id(header: &mut [u8], sender_id: u16) {
    if header.len() >= 4 {
        header[2..4].copy_from_slice(&sender_id.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn header_round_trips() {
        let header = AudioHeader {
            voice_active: true,
            sender_id: 0xBEEF,
            timestamp: 0xDEAD_BEEF,
            sequence: 65535,
            audio_level: 42,
        };
        let packet = header.encode(&[1, 2, 3]);
        assert_eq!(packet.len(), HEADER_LEN + 3);
        assert_eq!(
            &packet[..HEADER_LEN],
            &[0x11, 1, 0xBE, 0xEF, 0xDE, 0xAD, 0xBE, 0xEF, 0xFF, 0xFF, 42]
        );
        let (decoded, opus) = AudioHeader::decode(&packet).unwrap();
        assert_eq!(decoded, header);
        assert_eq!(opus, &[1, 2, 3]);
    }

    #[test]
    fn decode_rejects_bad_packets() {
        let ok = AudioHeader {
            voice_active: false,
            sender_id: 1,
            timestamp: 0,
            sequence: 0,
            audio_level: LEVEL_SILENT,
        }
        .encode(&[]);
        assert!(AudioHeader::decode(&ok).is_ok());
        assert_eq!(AudioHeader::decode(&ok[..10]), Err(PacketError::TooShort));
        let mut bad = ok.clone();
        bad[0] = 0x21;
        assert_eq!(AudioHeader::decode(&bad), Err(PacketError::BadVersion(2)));
        bad[0] = 0x12;
        assert_eq!(AudioHeader::decode(&bad), Err(PacketError::UnknownType(2)));
        let big = vec![0x11; MAX_DATAGRAM + 1];
        assert_eq!(AudioHeader::decode(&big), Err(PacketError::TooLarge));
        // Reserved flag bits are ignored.
        let mut flags = ok;
        flags[1] = 0xFE;
        assert!(!AudioHeader::decode(&flags).unwrap().0.voice_active);
    }

    #[test]
    fn sender_id_is_overwritten() {
        let mut packet = AudioHeader {
            voice_active: true,
            sender_id: 7,
            timestamp: 1,
            sequence: 2,
            audio_level: 3,
        }
        .encode(&[9]);
        set_sender_id(&mut packet, 300);
        assert_eq!(AudioHeader::decode(&packet).unwrap().0.sender_id, 300);
    }

    #[test]
    fn audio_levels() {
        assert_eq!(audio_level(0.0), LEVEL_SILENT);
        assert_eq!(audio_level(f32::NAN), LEVEL_SILENT);
        assert_eq!(audio_level(1.0), 0);
        assert_eq!(audio_level(0.1), 20);
        assert_eq!(audio_level(1e-9), LEVEL_SILENT);
    }

    #[test]
    fn control_messages_use_the_envelope() {
        let msg = ClientControl::Auth {
            token: "t".into(),
            self_mute: false,
            self_deaf: true,
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({ "op": "auth", "d": { "token": "t", "self_mute": false, "self_deaf": true } })
        );
        assert_eq!(
            serde_json::to_value(ClientControl::Leave {}).unwrap(),
            json!({ "op": "leave", "d": {} })
        );
        let ready = ServerControl::Ready {
            participant_id: 1,
            room_id: Id(5),
            bitrate: 64000,
            participants: vec![Participant {
                participant_id: 2,
                user_id: Id(9),
            }],
            sharing: vec![2],
        };
        let value = serde_json::to_value(&ready).unwrap();
        assert_eq!(value["d"]["participants"][0]["user_id"], "9");
        assert_eq!(
            serde_json::from_value::<ServerControl>(value).unwrap(),
            ready
        );
        assert_eq!(
            serde_json::to_value(ServerControl::Kicked {
                reason: KickReason::JoinedElsewhere
            })
            .unwrap(),
            json!({ "op": "kicked", "d": { "reason": "joined_elsewhere" } })
        );
    }

    #[test]
    fn frames_have_a_length_prefix() {
        let frame = encode_frame(&ClientControl::Ping { t: 1 });
        let len = frame_len(frame[..4].try_into().unwrap()).unwrap();
        assert_eq!(len, frame.len() - 4);
        let msg: ClientControl = serde_json::from_slice(&frame[4..]).unwrap();
        assert_eq!(msg, ClientControl::Ping { t: 1 });
        assert_eq!(frame_len(u32::MAX.to_be_bytes()), None);
    }

    #[test]
    fn colon_hex_format() {
        assert_eq!(colon_hex(&[0x3f, 0xa9, 0x01]), "3F:A9:01");
    }

    #[test]
    fn video_headers_round_trip_and_are_checked() {
        let header = VideoHeader {
            keyframe: true,
            sender_id: 7,
            width: 1280,
            height: 720,
            timestamp: 123_456,
        };
        let mut bytes = header.encode().to_vec();
        assert_eq!(VideoHeader::decode(&bytes), Ok(header));
        set_video_sender_id(&mut bytes, 9);
        assert_eq!(VideoHeader::decode(&bytes).unwrap().sender_id, 9);
        assert_eq!(
            VideoHeader::decode(&bytes[..11]),
            Err(PacketError::TooShort)
        );
        let mut audio = bytes.clone();
        audio[0] = (PACKET_VERSION << 4) | PACKET_TYPE_AUDIO;
        assert_eq!(
            VideoHeader::decode(&audio),
            Err(PacketError::UnknownType(1))
        );
        for (w, h) in [(0, 720), (1281, 720), (4000, 720), (1280, 0)] {
            let bad = VideoHeader {
                width: w,
                height: h,
                ..header
            };
            assert!(VideoHeader::decode(&bad.encode()).is_err(), "{w}x{h}");
        }
    }

    #[test]
    fn share_control_messages_round_trip() {
        for msg in [
            ClientControl::ShareStart {},
            ClientControl::ShareStop {},
            ClientControl::Watch {
                participant_id: 3,
                watching: true,
            },
            ClientControl::RequestKeyframe { participant_id: 3 },
        ] {
            let json = serde_json::to_string(&msg).unwrap();
            assert_eq!(serde_json::from_str::<ClientControl>(&json).unwrap(), msg);
        }
        let ready: ServerControl = serde_json::from_str(
            r#"{"op":"ready","d":{"participant_id":1,"room_id":"5","bitrate":64000,"participants":[]}}"#,
        )
        .unwrap();
        assert!(matches!(ready, ServerControl::Ready { sharing, .. } if sharing.is_empty()));
        let watchers = ServerControl::ShareWatchers { count: 2 };
        let json = serde_json::to_string(&watchers).unwrap();
        assert_eq!(json, r#"{"op":"share_watchers","d":{"count":2}}"#);
        assert_eq!(
            serde_json::from_str::<ServerControl>(&json).unwrap(),
            watchers
        );
    }
}
