pub mod simul_streaming;
pub mod openai;

pub use simul_streaming::{AsyncSimulStreamingClient, SimulStreamingClient};
pub use openai::{OpenAiBackend, OpenAiSyncBackend};

use async_trait::async_trait;
use anyhow::Result;
use crate::transcription::TranscriptionSegment;

#[async_trait]
pub trait TranscriptionBackend: Send {
    /// Send audio data to the backend.
    /// Implementation may buffer or stream immediately.
    async fn send_audio(&mut self, data: &[i16]) -> Result<()>;

    /// Receive transcription results.
    /// Returns None if stream ended or error occurred.
    async fn next_segment(&mut self) -> Result<Option<TranscriptionSegment>>;
}

pub trait SyncTranscriptionBackend: Send {
    /// Send audio data (blocking).
    fn send_audio(&mut self, data: &[i16]) -> Result<()>;
    /// Receive result (non-blocking or with short timeout).
    fn try_recv(&mut self) -> Option<TranscriptionSegment>;
}





