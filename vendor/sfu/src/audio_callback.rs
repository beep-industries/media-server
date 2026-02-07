//! Audio callback for intercepting decrypted audio RTP packets.
//!
//! This module provides a mechanism for external code to receive audio packets
//! as they pass through the SFU, enabling transcription and other audio processing.

use std::sync::mpsc::Sender;

/// Audio packet data extracted from RTP.
#[derive(Debug, Clone)]
pub struct AudioPacketInfo {
    /// RTP SSRC
    pub ssrc: u32,
    /// RTP timestamp
    pub timestamp: u32,
    /// RTP sequence number
    pub sequence_number: u16,
    /// Payload type (e.g., 111 for OPUS)
    pub payload_type: u8,
    /// The audio payload (e.g., OPUS encoded data)
    pub payload: Vec<u8>,
}

/// Callback type for audio packets.
pub type AudioCallback = Box<dyn Fn(AudioPacketInfo) + Send + Sync>;

/// Thread-safe audio sender for cross-thread communication.
pub type AudioSender = Sender<AudioPacketInfo>;

/// Common OPUS payload types used in WebRTC.
pub const OPUS_PAYLOAD_TYPES: &[u8] = &[111, 109, 96];

/// Check if a payload type is likely audio (OPUS).
pub fn is_audio_payload_type(pt: u8) -> bool {
    OPUS_PAYLOAD_TYPES.contains(&pt)
}

