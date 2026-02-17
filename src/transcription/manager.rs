//! Transcription session management.
//!
//! Provides high-level management of transcription sessions, including
//! connection pooling for multiple concurrent streams.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use log::{debug, error, info, warn};

use super::{
    AudioDecoder, SimulStreamingClient, TranscriptionSegment,
};
use super::backends::{SyncTranscriptionBackend, OpenAiSyncBackend};

#[derive(Debug, Clone)]
pub enum TranscriptionBackendConfig {
    None,
    SimulStreaming {
        addresses: Vec<String>,
    },
    OpenAi {
        api_key: String,
        base_url: Option<String>,
        model: Option<String>,
    },
}

impl Default for TranscriptionBackendConfig {
    fn default() -> Self {
        Self::None
    }
}

/// Configuration for the transcription manager.
#[derive(Debug, Clone)]
pub struct TranscriptionConfig {
    pub backend: TranscriptionBackendConfig,
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
            backend: TranscriptionBackendConfig::default(),
            connect_timeout: Duration::from_secs(10),
            auto_reconnect: true,
            max_reconnect_attempts: 3,
        }
    }
}

impl TranscriptionConfig {
    /// Create a new config with a single server address (Shim for compatibility).
    pub fn new(server_addr: &str) -> Self {
        Self {
            backend: TranscriptionBackendConfig::SimulStreaming {
                addresses: vec![server_addr.to_string()],
            },
            ..Default::default()
        }
    }

    /// Create a new config with multiple server addresses for load balancing.
    pub fn with_servers(server_addresses: Vec<String>) -> Self {
        Self {
            backend: TranscriptionBackendConfig::SimulStreaming {
                addresses: server_addresses,
            },
            ..Default::default()
        }
    }

    pub fn with_openai(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        Self {
            backend: TranscriptionBackendConfig::OpenAi {
                api_key,
                base_url,
                model,
            },
            ..Default::default()
        }
    }

    /// Create a new config with no backend.
    pub fn none() -> Self {
        Self {
            backend: TranscriptionBackendConfig::None,
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
    /// Transcription backend
    backend: Box<dyn SyncTranscriptionBackend>,
}

impl TranscriptionSession {
    /// Create a new transcription session.
    pub fn new(
        session_id: u64,
        endpoint_id: u64,
        config: &TranscriptionConfig,
        server_index: usize, // for round-robin
    ) -> anyhow::Result<Self> {
        let decoder = AudioDecoder::new()?;

        let backend: Box<dyn SyncTranscriptionBackend> = match &config.backend {
            TranscriptionBackendConfig::None => {
                return Err(anyhow::anyhow!("No transcription backend configured for this session"));
            }
            TranscriptionBackendConfig::SimulStreaming { addresses } => {
                if addresses.is_empty() {
                    return Err(anyhow::anyhow!("No SimulStreaming addresses configured"));
                }
                let addr = &addresses[server_index % addresses.len()];
                info!(
                    "Creating SimulStreaming session {}/{} connected to {}",
                    session_id, endpoint_id, addr
                );
                Box::new(SimulStreamingClient::connect_with_timeout(addr, config.connect_timeout)?)
            }
            TranscriptionBackendConfig::OpenAi { api_key, base_url, model } => {
                info!("Creating OpenAI session {}/{}", session_id, endpoint_id);
                Box::new(OpenAiSyncBackend::new(
                    api_key.clone(),
                    base_url.clone(),
                    model.clone(),
                ))
            }
        };

        Ok(Self {
            session_id,
            endpoint_id,
            decoder,
            backend,
        })
    }

    /// Process an OPUS audio packet.
    ///
    /// Decodes the OPUS data, resamples to 16kHz, and sends to SimulStreaming.
    pub fn process_audio(&mut self, opus_data: &[u8]) -> anyhow::Result<()> {
        // Decode OPUS and resample to 16kHz
        let pcm = self.decoder.decode(opus_data)?;

        // Send to backend
        self.backend.send_audio(&pcm)?;

        Ok(())
    }

    /// Process raw PCM audio (already at correct format).
    ///
    /// Use this if audio is already 16kHz mono PCM.
    #[allow(dead_code)]
    pub fn process_raw_pcm(&mut self, pcm_data: &[i16]) -> anyhow::Result<()> {
        self.backend.send_audio(pcm_data)
    }

    /// Try to get transcription results (non-blocking).
    pub fn try_recv(&mut self) -> Option<TranscriptionSegment> {
        self.backend.try_recv()
    }

    /// Get transcription results (blocking).
    #[allow(dead_code)]
    pub fn recv(&mut self) -> Option<TranscriptionSegment> {
        // Fallback to try_recv if backend implies blocking or use a loop/sleep
        self.backend.try_recv() // Placeholder
    }

    /// Get transcription results with timeout.
    #[allow(dead_code)]
    pub fn recv_timeout(&mut self, _timeout: Duration) -> Option<TranscriptionSegment> {
         // TODO: Add timeout support to trait
         self.backend.try_recv()
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
        match &config.backend {
            TranscriptionBackendConfig::None => {
                info!("Creating TranscriptionManager with NO default backend (client-provided config required)");
            }
            TranscriptionBackendConfig::SimulStreaming { addresses } => {
                info!(
                    "Creating TranscriptionManager with {} SimulStreaming servers",
                    addresses.len()
                );
            }
            TranscriptionBackendConfig::OpenAi { .. } => {
                info!("Creating TranscriptionManager with OpenAI backend");
            }
        }

        Self {
            config,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            next_server_index: Arc::new(Mutex::new(0)),
        }
    }

    /// Start a transcription session for a participant.
    pub fn start_session(
        &self,
        session_id: u64,
        endpoint_id: u64,
        config_override: Option<TranscriptionConfig>,
    ) -> anyhow::Result<()> {
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

        let config = config_override.as_ref().unwrap_or(&self.config);

        // Get server index
        let server_index = {
            let mut index = self.next_server_index.lock().unwrap();
            let current = *index;
            match &self.config.backend {
                TranscriptionBackendConfig::SimulStreaming { addresses } => {
                     if !addresses.is_empty() {
                        *index = (*index + 1) % addresses.len();
                     }
                }
                _ => {}
            }
            current
        };

        // Create new session
        let session = TranscriptionSession::new(session_id, endpoint_id, config, server_index)?;

        // Store session
        {
            let mut sessions = self.sessions.write().unwrap();
            sessions.insert(key, Arc::new(Mutex::new(session)));
        }

        info!(
            "Started transcription session {}/{}",
            session_id, endpoint_id
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
            let mut session = session.lock().unwrap();
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

