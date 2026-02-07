//! RTP handler for audio transcription.
//!
//! This module provides utilities for intercepting audio from RTP packets
//! and sending them to the transcription system.

use log::{debug, error, trace};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use super::{AudioDecoder, TranscriptionManager};

/// Audio data to be sent to the transcription worker thread.
#[derive(Debug)]
pub struct AudioPacket {
    pub session_id: u64,
    pub endpoint_id: u64,
    pub opus_data: Vec<u8>,
    pub timestamp: u32,
    pub ssrc: u32,
}

/// Command to control the transcription worker.
#[derive(Debug)]
pub enum TranscriptionCommand {
    /// Start transcription for a session/endpoint
    StartSession { session_id: u64, endpoint_id: u64 },
    /// Stop transcription for a session/endpoint
    StopSession { session_id: u64, endpoint_id: u64 },
    /// Process audio packet
    Audio(AudioPacket),
    /// Shutdown the worker
    Shutdown,
}

/// RTP packet parser for extracting audio.
///
/// This is a lightweight parser that extracts the necessary info from RTP packets
/// without needing the full sfu crate types.
pub struct RtpAudioExtractor {
    /// SSRC to (session_id, endpoint_id) mapping
    pub ssrc_map: HashMap<u32, (u64, u64)>,
    /// Audio payload types to intercept (typically 111 for OPUS)
    audio_payload_types: Vec<u8>,
    /// Default session/endpoint for auto-learning (if set)
    pub default_session: Option<(u64, u64)>,
    /// Auto-learn SSRCs from the first audio packets
    auto_learn: bool,
    /// Sessions that have transcription enabled
    transcription_sessions: HashMap<(u64, u64), bool>,
}

impl RtpAudioExtractor {
    /// Create a new RTP audio extractor.
    pub fn new() -> Self {
        Self {
            ssrc_map: HashMap::new(),
            // Common OPUS payload types in WebRTC
            audio_payload_types: vec![111, 109],
            default_session: None,
            auto_learn: false,
            transcription_sessions: HashMap::new(),
        }
    }

    /// Create an extractor that auto-learns SSRCs.
    ///
    /// When `auto_learn` is true, SSRCs seen in audio packets will be automatically
    /// assigned to the `default_session` if not already registered.
    pub fn with_auto_learn(default_session_id: u64, default_endpoint_id: u64) -> Self {
        Self {
            ssrc_map: HashMap::new(),
            audio_payload_types: vec![111, 109],
            default_session: Some((default_session_id, default_endpoint_id)),
            auto_learn: true,
            transcription_sessions: HashMap::new(),
        }
    }

    /// Enable auto-learning with a default session.
    pub fn set_auto_learn(&mut self, session_id: u64, endpoint_id: u64) {
        self.default_session = Some((session_id, endpoint_id));
        self.auto_learn = true;
    }

    /// Mark a session as having transcription enabled.
    pub fn enable_transcription_for_session(&mut self, session_id: u64, endpoint_id: u64) {
        debug!(
            "Enabled transcription for session {}/endpoint {}",
            session_id, endpoint_id
        );
        self.transcription_sessions.insert((session_id, endpoint_id), true);
        // Also set as default for auto-learning
        self.default_session = Some((session_id, endpoint_id));
        self.auto_learn = true;
    }

    /// Disable transcription for a session.
    pub fn disable_transcription_for_session(&mut self, session_id: u64, endpoint_id: u64) {
        self.transcription_sessions.remove(&(session_id, endpoint_id));
    }

    /// Check if a session has transcription enabled.
    pub fn is_transcription_enabled(&self, session_id: u64, endpoint_id: u64) -> bool {
        self.transcription_sessions.get(&(session_id, endpoint_id)).copied().unwrap_or(false)
    }

    /// Register an SSRC for a session/endpoint.
    pub fn register_ssrc(&mut self, ssrc: u32, session_id: u64, endpoint_id: u64) {
        debug!(
            "Registered SSRC {} for session {}/endpoint {}",
            ssrc, session_id, endpoint_id
        );
        self.ssrc_map.insert(ssrc, (session_id, endpoint_id));
    }

    /// Unregister an SSRC.
    pub fn unregister_ssrc(&mut self, ssrc: u32) {
        self.ssrc_map.remove(&ssrc);
    }

    /// Check if a payload type is audio.
    fn is_audio_payload_type(&self, pt: u8) -> bool {
        self.audio_payload_types.contains(&pt)
    }

    /// Try to extract audio from an RTP packet.
    ///
    /// Returns Some(AudioPacket) if this is an audio packet from a known SSRC,
    /// None otherwise.
    pub fn extract_audio(&mut self, rtp_data: &[u8]) -> Option<AudioPacket> {
        // RTP header is at least 12 bytes
        if rtp_data.len() < 12 {
            return None;
        }

        // Parse RTP header
        let version = (rtp_data[0] >> 6) & 0x03;
        if version != 2 {
            return None;
        }

        let padding = (rtp_data[0] >> 5) & 0x01;
        let extension = (rtp_data[0] >> 4) & 0x01;
        let csrc_count = rtp_data[0] & 0x0F;
        let payload_type = rtp_data[1] & 0x7F;
        let _sequence = u16::from_be_bytes([rtp_data[2], rtp_data[3]]);
        let timestamp = u32::from_be_bytes([rtp_data[4], rtp_data[5], rtp_data[6], rtp_data[7]]);
        let ssrc = u32::from_be_bytes([rtp_data[8], rtp_data[9], rtp_data[10], rtp_data[11]]);

        // Check if this is audio
        if !self.is_audio_payload_type(payload_type) {
            return None;
        }

        // Look up session/endpoint for this SSRC, or auto-learn
        let (session_id, endpoint_id) = if let Some(&ids) = self.ssrc_map.get(&ssrc) {
            ids
        } else if self.auto_learn {
            if let Some(default) = self.default_session {
                debug!(
                    "Auto-learned SSRC {} for session {}/endpoint {}",
                    ssrc, default.0, default.1
                );
                self.ssrc_map.insert(ssrc, default);
                default
            } else {
                return None;
            }
        } else {
            return None;
        };

        // Calculate payload offset
        let mut offset = 12 + (csrc_count as usize * 4);

        // Handle extension
        if extension == 1 && rtp_data.len() > offset + 4 {
            let ext_length = u16::from_be_bytes([rtp_data[offset + 2], rtp_data[offset + 3]]);
            offset += 4 + (ext_length as usize * 4);
        }

        if offset >= rtp_data.len() {
            return None;
        }

        // Handle padding
        let payload_end = if padding == 1 && !rtp_data.is_empty() {
            let padding_len = rtp_data[rtp_data.len() - 1] as usize;
            rtp_data.len().saturating_sub(padding_len)
        } else {
            rtp_data.len()
        };

        if offset >= payload_end {
            return None;
        }

        let opus_data = rtp_data[offset..payload_end].to_vec();

        Some(AudioPacket {
            session_id,
            endpoint_id,
            opus_data,
            timestamp,
            ssrc,
        })
    }
}

impl Default for RtpAudioExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Transcription worker that runs on a separate thread.
///
/// Receives audio packets and processes them through the transcription system.
pub struct TranscriptionWorker {
    manager: Arc<TranscriptionManager>,
    rx: Receiver<TranscriptionCommand>,
    /// Per-session decoders (session_id, endpoint_id) -> decoder
    decoders: HashMap<(u64, u64), AudioDecoder>,
}

impl TranscriptionWorker {
    /// Create a new transcription worker.
    pub fn new(manager: Arc<TranscriptionManager>, rx: Receiver<TranscriptionCommand>) -> Self {
        Self {
            manager,
            rx,
            decoders: HashMap::new(),
        }
    }

    /// Run the worker (blocking).
    ///
    /// This should be called from a dedicated thread.
    pub fn run(&mut self) {
        debug!("Transcription worker started");

        loop {
            match self.rx.recv() {
                Ok(cmd) => match cmd {
                    TranscriptionCommand::StartSession {
                        session_id,
                        endpoint_id,
                    } => {
                        debug!(
                            "Starting transcription session {}/{}",
                            session_id, endpoint_id
                        );
                        if let Err(e) = self.manager.start_session(session_id, endpoint_id) {
                            error!("Failed to start transcription session: {}", e);
                        }
                        // Create decoder for this session
                        match AudioDecoder::new() {
                            Ok(decoder) => {
                                self.decoders.insert((session_id, endpoint_id), decoder);
                            }
                            Err(e) => {
                                error!("Failed to create audio decoder: {}", e);
                            }
                        }
                    }
                    TranscriptionCommand::StopSession {
                        session_id,
                        endpoint_id,
                    } => {
                        debug!(
                            "Stopping transcription session {}/{}",
                            session_id, endpoint_id
                        );
                        self.manager.stop_session(session_id, endpoint_id);
                        self.decoders.remove(&(session_id, endpoint_id));
                    }
                    TranscriptionCommand::Audio(packet) => {
                        self.process_audio_packet(packet);
                    }
                    TranscriptionCommand::Shutdown => {
                        debug!("Transcription worker shutting down");
                        break;
                    }
                },
                Err(_) => {
                    debug!("Transcription channel closed, worker exiting");
                    break;
                }
            }
        }
    }

    /// Process an audio packet.
    fn process_audio_packet(&mut self, packet: AudioPacket) {
        let key = (packet.session_id, packet.endpoint_id);

        // Get or create decoder
        let decoder = self.decoders.entry(key).or_insert_with(|| {
            AudioDecoder::new().expect("Failed to create audio decoder")
        });

        // Decode OPUS to PCM
        match decoder.decode(&packet.opus_data) {
            Ok(_pcm) => {
                // Send PCM to transcription manager
                // The manager already has the session, so just send the raw OPUS
                // since the manager's session will decode it again
                // TODO: Consider passing PCM directly to avoid double decode
                if let Err(e) = self.manager.process_audio(
                    packet.session_id,
                    packet.endpoint_id,
                    &packet.opus_data,
                ) {
                    trace!("Failed to process audio: {}", e);
                }
            }
            Err(e) => {
                trace!("Failed to decode OPUS: {}", e);
            }
        }
    }
}

/// Create a transcription channel and worker.
///
/// Returns (sender, worker) where:
/// - sender: Use to send audio commands
/// - worker: Run on a separate thread
pub fn create_transcription_channel(
    manager: Arc<TranscriptionManager>,
) -> (Sender<TranscriptionCommand>, TranscriptionWorker) {
    let (tx, rx) = mpsc::channel();
    let worker = TranscriptionWorker::new(manager, rx);
    (tx, worker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rtp_parser() {
        let mut extractor = RtpAudioExtractor::new();

        // Register an SSRC
        extractor.register_ssrc(0x12345678, 1, 1);

        // Create a minimal RTP packet with PT=111 (OPUS)
        let rtp_packet = vec![
            0x80, // V=2, P=0, X=0, CC=0
            111,  // M=0, PT=111
            0x00, 0x01, // Sequence number
            0x00, 0x00, 0x00, 0x00, // Timestamp
            0x12, 0x34, 0x56, 0x78, // SSRC
            // Payload (fake OPUS data)
            0xFC, 0x01, 0x02, 0x03,
        ];

        let result = extractor.extract_audio(&rtp_packet);
        assert!(result.is_some());
        let packet = result.unwrap();
        assert_eq!(packet.session_id, 1);
        assert_eq!(packet.endpoint_id, 1);
        assert_eq!(packet.ssrc, 0x12345678);
        assert_eq!(packet.opus_data, vec![0xFC, 0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_rtp_parser_unknown_ssrc() {
        let mut extractor = RtpAudioExtractor::new();

        // Create RTP packet with unknown SSRC
        let rtp_packet = vec![
            0x80, 111, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
            0xAB, 0xCD, 0xEF, 0x00,
            0xFC, 0x01, 0x02, 0x03,
        ];

        let result = extractor.extract_audio(&rtp_packet);
        assert!(result.is_none()); // Unknown SSRC
    }

    #[test]
    fn test_rtp_parser_video_packet() {
        let mut extractor = RtpAudioExtractor::new();
        extractor.register_ssrc(0x12345678, 1, 1);

        // Create RTP packet with PT=96 (VP8 video)
        let rtp_packet = vec![
            0x80, 96, 0x00, 0x01, // PT=96 (not in our audio list)
            0x00, 0x00, 0x00, 0x00,
            0x12, 0x34, 0x56, 0x78,
            0x00, 0x01, 0x02, 0x03,
        ];

        let result = extractor.extract_audio(&rtp_packet);
        // PT=96 is NOT in our audio list (111, 109), so this should be None
        assert!(result.is_none());
    }
}

