//! Transcription module for SimulStreaming integration.
//!
//! This module provides real-time speech-to-text transcription by connecting
//! to a SimulStreaming server over TCP. It handles:
//! - OPUS audio decoding
//! - Audio resampling (48kHz → 16kHz)
//! - TCP communication with SimulStreaming
//! - Transcription result parsing
//! - SFU pipeline integration via TranscriptionHandler

mod client;
mod decoder;
mod handler;
mod manager;

pub use client::{AsyncSimulStreamingClient, SimulStreamingClient};
pub use decoder::{AudioDecoder, AudioResampler};
pub use handler::{
    create_transcription_channel, AudioPacket, RtpAudioExtractor, TranscriptionCommand,
    TranscriptionWorker,
};
pub use manager::{TranscriptionConfig, TranscriptionManager};

/// Transcription result from SimulStreaming
#[derive(Debug, Clone)]
pub struct TranscriptionSegment {
    /// Start timestamp in milliseconds (relative to audio stream start)
    pub start_ms: u64,
    /// End timestamp in milliseconds
    pub end_ms: u64,
    /// Transcribed text
    pub text: String,
}

/// SimulStreaming expects: 16kHz, mono, S16_LE PCM
pub const SIMULSTREAMING_SAMPLE_RATE: u32 = 16000;

/// WebRTC typically uses 48kHz audio
pub const WEBRTC_SAMPLE_RATE: u32 = 48000;

