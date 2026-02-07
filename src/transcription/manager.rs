//! Transcription session management.
//!
//! Provides high-level management of transcription sessions, including
//! connection pooling for multiple concurrent streams.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use log::{debug, error, info, warn};
use tokio::sync::mpsc as tokio_mpsc;

use super::{
    AsyncSimulStreamingClient, AudioDecoder, SimulStreamingClient, TranscriptionSegment,
};

/// Configuration for the transcription manager.
#[derive(Debug, Clone)]
pub struct TranscriptionConfig {
    /// SimulStreaming server addresses (for connection pooling)
    pub server_addresses: Vec<String>,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Whether to automatically reconnect on failure
    pub auto_reconnect: bool,
    /// Maximum reconnect attempts
    pub max_reconnect_attempts: u32,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            server_addresses: vec!["localhost:43007".to_string()],
            connect_timeout: Duration::from_secs(10),
            auto_reconnect: true,
            max_reconnect_attempts: 3,
        }
    }
}

impl TranscriptionConfig {
    /// Create a new config with a single server address.
    pub fn new(server_addr: &str) -> Self {
        Self {
            server_addresses: vec![server_addr.to_string()],
            ..Default::default()
        }
    }

    /// Create a new config with multiple server addresses for load balancing.
    pub fn with_servers(server_addresses: Vec<String>) -> Self {
        Self {
            server_addresses,
            ..Default::default()
        }
    }
}

/// A transcription session for a single audio stream.
pub struct TranscriptionSession {
    /// Unique session identifier
    pub session_id: u64,
    /// Endpoint identifier
    pub endpoint_id: u64,
    /// Audio decoder (OPUS -> PCM 16kHz)
    decoder: AudioDecoder,
    /// SimulStreaming client
    client: SimulStreamingClient,
}

impl TranscriptionSession {
    /// Create a new transcription session.
    pub fn new(session_id: u64, endpoint_id: u64, server_addr: &str) -> anyhow::Result<Self> {
        let decoder = AudioDecoder::new()?;
        let client = SimulStreamingClient::connect(server_addr)?;

        info!(
            "Created transcription session {}/{} connected to {}",
            session_id, endpoint_id, server_addr
        );

        Ok(Self {
            session_id,
            endpoint_id,
            decoder,
            client,
        })
    }

    /// Process an OPUS audio packet.
    ///
    /// Decodes the OPUS data, resamples to 16kHz, and sends to SimulStreaming.
    pub fn process_audio(&mut self, opus_data: &[u8]) -> anyhow::Result<()> {
        // Decode OPUS and resample to 16kHz
        let pcm = self.decoder.decode(opus_data)?;

        // Send to SimulStreaming
        self.client.send_audio(&pcm)?;

        Ok(())
    }

    /// Process raw PCM audio (already at correct format).
    ///
    /// Use this if audio is already 16kHz mono PCM.
    pub fn process_raw_pcm(&mut self, pcm_data: &[i16]) -> anyhow::Result<()> {
        self.client.send_audio(pcm_data)
    }

    /// Try to get transcription results (non-blocking).
    pub fn try_recv(&self) -> Option<TranscriptionSegment> {
        self.client.try_recv()
    }

    /// Get transcription results (blocking).
    pub fn recv(&self) -> Option<TranscriptionSegment> {
        self.client.recv()
    }

    /// Get transcription results with timeout.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<TranscriptionSegment> {
        self.client.recv_timeout(timeout)
    }
}

/// Manager for multiple transcription sessions.
///
/// Handles session lifecycle and provides a callback interface for
/// receiving transcription results.
pub struct TranscriptionManager {
    config: TranscriptionConfig,
    sessions: Arc<RwLock<HashMap<(u64, u64), Arc<Mutex<TranscriptionSession>>>>>,
    next_server_index: Arc<Mutex<usize>>,
}

impl TranscriptionManager {
    /// Create a new transcription manager.
    pub fn new(config: TranscriptionConfig) -> Self {
        info!(
            "Creating TranscriptionManager with {} servers",
            config.server_addresses.len()
        );

        Self {
            config,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            next_server_index: Arc::new(Mutex::new(0)),
        }
    }

    /// Start a transcription session for a participant.
    pub fn start_session(&self, session_id: u64, endpoint_id: u64) -> anyhow::Result<()> {
        let key = (session_id, endpoint_id);

        // Check if session already exists
        {
            let sessions = self.sessions.read().unwrap();
            if sessions.contains_key(&key) {
                warn!(
                    "Transcription session {}/{} already exists",
                    session_id, endpoint_id
                );
                return Ok(());
            }
        }

        // Get next server address (round-robin)
        let server_addr = {
            let mut index = self.next_server_index.lock().unwrap();
            let addr = &self.config.server_addresses[*index % self.config.server_addresses.len()];
            *index = (*index + 1) % self.config.server_addresses.len();
            addr.clone()
        };

        // Create new session
        let session = TranscriptionSession::new(session_id, endpoint_id, &server_addr)?;

        // Store session
        {
            let mut sessions = self.sessions.write().unwrap();
            sessions.insert(key, Arc::new(Mutex::new(session)));
        }

        info!(
            "Started transcription session {}/{} on {}",
            session_id, endpoint_id, server_addr
        );

        Ok(())
    }

    /// Stop a transcription session.
    pub fn stop_session(&self, session_id: u64, endpoint_id: u64) {
        let key = (session_id, endpoint_id);

        let mut sessions = self.sessions.write().unwrap();
        if sessions.remove(&key).is_some() {
            info!(
                "Stopped transcription session {}/{}",
                session_id, endpoint_id
            );
        }
    }

    /// Process audio for a session.
    pub fn process_audio(
        &self,
        session_id: u64,
        endpoint_id: u64,
        opus_data: &[u8],
    ) -> anyhow::Result<()> {
        let key = (session_id, endpoint_id);

        let session = {
            let sessions = self.sessions.read().unwrap();
            sessions.get(&key).cloned()
        };

        match session {
            Some(session) => {
                let mut session = session.lock().unwrap();
                session.process_audio(opus_data)
            }
            None => {
                debug!(
                    "No transcription session for {}/{}, ignoring audio",
                    session_id, endpoint_id
                );
                Ok(())
            }
        }
    }

    /// Poll for transcription results from all sessions.
    ///
    /// Returns a vector of (session_id, endpoint_id, segment) tuples.
    pub fn poll_results(&self) -> Vec<(u64, u64, TranscriptionSegment)> {
        let sessions = self.sessions.read().unwrap();
        let mut results = Vec::new();

        for (&(session_id, endpoint_id), session) in sessions.iter() {
            let session = session.lock().unwrap();
            while let Some(segment) = session.try_recv() {
                results.push((session_id, endpoint_id, segment));
            }
        }

        results
    }

    /// Get the number of active sessions.
    pub fn active_session_count(&self) -> usize {
        self.sessions.read().unwrap().len()
    }

    /// Check if a session exists.
    pub fn has_session(&self, session_id: u64, endpoint_id: u64) -> bool {
        let sessions = self.sessions.read().unwrap();
        sessions.contains_key(&(session_id, endpoint_id))
    }
}

// ============================================================================
// Async Manager
// ============================================================================

/// Async version of the transcription manager.
///
/// Uses Tokio for async I/O and provides channels for receiving results.
pub struct AsyncTranscriptionManager {
    config: TranscriptionConfig,
    result_tx: tokio_mpsc::Sender<(u64, u64, TranscriptionSegment)>,
    result_rx: tokio::sync::Mutex<tokio_mpsc::Receiver<(u64, u64, TranscriptionSegment)>>,
}

impl AsyncTranscriptionManager {
    /// Create a new async transcription manager.
    pub fn new(config: TranscriptionConfig) -> Self {
        let (tx, rx) = tokio_mpsc::channel(1000);

        Self {
            config,
            result_tx: tx,
            result_rx: tokio::sync::Mutex::new(rx),
        }
    }

    /// Start a transcription session and spawn a task to handle it.
    pub async fn start_session(
        &self,
        session_id: u64,
        endpoint_id: u64,
    ) -> anyhow::Result<tokio_mpsc::Sender<Vec<i16>>> {
        let server_addr = &self.config.server_addresses[0]; // Simple: use first server

        let mut client = AsyncSimulStreamingClient::connect(server_addr).await?;
        let result_tx = self.result_tx.clone();

        // Create channel for sending audio to this session
        let (audio_tx, mut audio_rx) = tokio_mpsc::channel::<Vec<i16>>(100);

        // Spawn task to handle this session
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Receive audio to send
                    Some(pcm_data) = audio_rx.recv() => {
                        if let Err(e) = client.send_audio(&pcm_data).await {
                            error!("Failed to send audio: {}", e);
                            break;
                        }
                    }
                    // Receive transcription results
                    Some(segment) = client.recv() => {
                        if result_tx.send((session_id, endpoint_id, segment)).await.is_err() {
                            break;
                        }
                    }
                    else => break,
                }
            }
            info!(
                "Transcription session {}/{} ended",
                session_id, endpoint_id
            );
        });

        info!(
            "Started async transcription session {}/{} on {}",
            session_id, endpoint_id, server_addr
        );

        Ok(audio_tx)
    }

    /// Receive transcription results.
    pub async fn recv(&self) -> Option<(u64, u64, TranscriptionSegment)> {
        self.result_rx.lock().await.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = TranscriptionConfig::default();
        assert_eq!(config.server_addresses.len(), 1);
        assert_eq!(config.server_addresses[0], "localhost:43007");
    }

    #[test]
    fn test_config_with_servers() {
        let config = TranscriptionConfig::with_servers(vec![
            "server1:43007".to_string(),
            "server2:43008".to_string(),
        ]);
        assert_eq!(config.server_addresses.len(), 2);
    }
}

