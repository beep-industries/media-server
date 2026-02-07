//! SimulStreaming TCP client implementation.
//!
//! Provides both synchronous and asynchronous clients for communicating
//! with a SimulStreaming transcription server.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use log::{debug, error, info, warn};

use super::TranscriptionSegment;

/// Synchronous client for SimulStreaming TCP server.
///
/// This client connects to a SimulStreaming server and provides methods
/// to send audio data and receive transcription results.
///
/// # Example
/// ```ignore
/// let mut client = SimulStreamingClient::connect("localhost:43007")?;
/// client.send_audio(&pcm_samples)?;
/// if let Some(segment) = client.try_recv() {
///     println!("Transcription: {}", segment.text);
/// }
/// ```
pub struct SimulStreamingClient {
    stream: TcpStream,
    result_rx: Receiver<TranscriptionSegment>,
    _reader_handle: Option<thread::JoinHandle<()>>,
}

impl SimulStreamingClient {
    /// Connect to a SimulStreaming server at the given address.
    ///
    /// # Arguments
    /// * `addr` - Server address in format "host:port" (e.g., "localhost:43007")
    ///
    /// # Returns
    /// A connected client ready to send audio and receive transcriptions.
    pub fn connect(addr: &str) -> anyhow::Result<Self> {
        Self::connect_with_timeout(addr, Duration::from_secs(10))
    }

    /// Connect to a SimulStreaming server with a custom timeout.
    pub fn connect_with_timeout(addr: &str, timeout: Duration) -> anyhow::Result<Self> {
        info!("Connecting to SimulStreaming server at {}", addr);

        let stream = TcpStream::connect_timeout(
            &addr.parse()?,
            timeout,
        )?;

        // Set socket options for real-time streaming
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;

        let read_stream = stream.try_clone()?;
        let (tx, rx) = mpsc::channel();

        // Spawn reader thread for transcription results
        let handle = thread::spawn(move || {
            Self::read_results(read_stream, tx);
        });

        info!("Connected to SimulStreaming server at {}", addr);

        Ok(Self {
            stream,
            result_rx: rx,
            _reader_handle: Some(handle),
        })
    }

    /// Send audio data to the server.
    ///
    /// # Arguments
    /// * `pcm_data` - PCM audio samples (must be 16kHz, mono, S16_LE)
    ///
    /// # Note
    /// The audio must already be in the correct format (16kHz, mono).
    /// Use `AudioDecoder` to convert from OPUS/48kHz.
    pub fn send_audio(&mut self, pcm_data: &[i16]) -> anyhow::Result<()> {
        // Convert i16 slice to bytes (little-endian)
        let bytes: Vec<u8> = pcm_data
            .iter()
            .flat_map(|&sample| sample.to_le_bytes())
            .collect();

        self.stream.write_all(&bytes)?;
        self.stream.flush()?;

        debug!("Sent {} audio samples ({} bytes)", pcm_data.len(), bytes.len());
        Ok(())
    }

    /// Try to receive transcription results (non-blocking).
    ///
    /// # Returns
    /// `Some(TranscriptionSegment)` if a result is available, `None` otherwise.
    pub fn try_recv(&self) -> Option<TranscriptionSegment> {
        self.result_rx.try_recv().ok()
    }

    /// Receive transcription results (blocking).
    ///
    /// # Returns
    /// `Some(TranscriptionSegment)` when available, `None` if the channel is closed.
    pub fn recv(&self) -> Option<TranscriptionSegment> {
        self.result_rx.recv().ok()
    }

    /// Receive transcription results with timeout.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<TranscriptionSegment> {
        self.result_rx.recv_timeout(timeout).ok()
    }

    /// Check if there are pending transcription results.
    pub fn has_results(&self) -> bool {
        !self.result_rx.try_recv().is_err()
    }

    /// Internal: Read and parse results from the TCP stream.
    fn read_results(stream: TcpStream, tx: Sender<TranscriptionSegment>) {
        let reader = BufReader::new(stream);

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    error!("Error reading from SimulStreaming: {}", e);
                    break;
                }
            };

            if line.is_empty() {
                continue;
            }

            // Parse: "start_ms end_ms text"
            match Self::parse_line(&line) {
                Some(segment) => {
                    debug!("Received transcription: [{}-{}] {}",
                           segment.start_ms, segment.end_ms, segment.text);

                    if tx.send(segment).is_err() {
                        warn!("Transcription receiver dropped, stopping reader");
                        break;
                    }
                }
                None => {
                    warn!("Failed to parse transcription line: {}", line);
                }
            }
        }

        info!("SimulStreaming reader thread exiting");
    }

    /// Parse a line from SimulStreaming output.
    /// Format: "start_ms end_ms text"
    fn parse_line(line: &str) -> Option<TranscriptionSegment> {
        let parts: Vec<&str> = line.splitn(3, ' ').collect();
        if parts.len() >= 3 {
            let start = parts[0].parse::<f64>().ok()? as u64;
            let end = parts[1].parse::<f64>().ok()? as u64;
            Some(TranscriptionSegment {
                start_ms: start,
                end_ms: end,
                text: parts[2].to_string(),
            })
        } else {
            None
        }
    }
}

impl Drop for SimulStreamingClient {
    fn drop(&mut self) {
        // Shutdown the write side to signal EOF to the server
        let _ = self.stream.shutdown(std::net::Shutdown::Write);
    }
}

// ============================================================================
// Async Client
// ============================================================================

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::TcpStream as TokioTcpStream;
use tokio::sync::mpsc as tokio_mpsc;

/// Asynchronous client for SimulStreaming TCP server.
///
/// This client uses Tokio for async I/O and is suitable for integration
/// with async runtimes.
///
/// # Example
/// ```ignore
/// let mut client = AsyncSimulStreamingClient::connect("localhost:43007").await?;
/// client.send_audio(&pcm_samples).await?;
/// if let Some(segment) = client.recv().await {
///     println!("Transcription: {}", segment.text);
/// }
/// ```
pub struct AsyncSimulStreamingClient {
    write_half: tokio::net::tcp::OwnedWriteHalf,
    result_rx: tokio_mpsc::Receiver<TranscriptionSegment>,
}

impl AsyncSimulStreamingClient {
    /// Connect to a SimulStreaming server asynchronously.
    ///
    /// # Arguments
    /// * `addr` - Server address in format "host:port"
    pub async fn connect(addr: &str) -> anyhow::Result<Self> {
        info!("Connecting to SimulStreaming server at {} (async)", addr);

        let stream = TokioTcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;

        let (read_half, write_half) = stream.into_split();
        let (tx, rx) = tokio_mpsc::channel(100);

        // Spawn reader task
        tokio::spawn(async move {
            Self::read_results(read_half, tx).await;
        });

        info!("Connected to SimulStreaming server at {} (async)", addr);

        Ok(Self {
            write_half,
            result_rx: rx,
        })
    }

    /// Send audio data to the server asynchronously.
    ///
    /// # Arguments
    /// * `pcm_data` - PCM audio samples (must be 16kHz, mono, S16_LE)
    pub async fn send_audio(&mut self, pcm_data: &[i16]) -> anyhow::Result<()> {
        let bytes: Vec<u8> = pcm_data
            .iter()
            .flat_map(|&sample| sample.to_le_bytes())
            .collect();

        self.write_half.write_all(&bytes).await?;
        self.write_half.flush().await?;

        debug!("Sent {} audio samples ({} bytes)", pcm_data.len(), bytes.len());
        Ok(())
    }

    /// Receive transcription result asynchronously.
    ///
    /// # Returns
    /// `Some(TranscriptionSegment)` when available, `None` if the channel is closed.
    pub async fn recv(&mut self) -> Option<TranscriptionSegment> {
        self.result_rx.recv().await
    }

    /// Try to receive transcription result without waiting.
    pub fn try_recv(&mut self) -> Option<TranscriptionSegment> {
        self.result_rx.try_recv().ok()
    }

    /// Internal: Read and parse results from the TCP stream.
    async fn read_results(
        read_half: tokio::net::tcp::OwnedReadHalf,
        tx: tokio_mpsc::Sender<TranscriptionSegment>,
    ) {
        let mut reader = TokioBufReader::new(read_half);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    info!("SimulStreaming connection closed (EOF)");
                    break;
                }
                Ok(_) => {
                    if let Some(segment) = Self::parse_line(&line) {
                        debug!("Received transcription: [{}-{}] {}",
                               segment.start_ms, segment.end_ms, segment.text);

                        if tx.send(segment).await.is_err() {
                            warn!("Transcription receiver dropped, stopping reader");
                            break;
                        }
                    }
                }
                Err(e) => {
                    error!("Error reading from SimulStreaming: {}", e);
                    break;
                }
            }
        }

        info!("SimulStreaming async reader task exiting");
    }

    /// Parse a line from SimulStreaming output.
    fn parse_line(line: &str) -> Option<TranscriptionSegment> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
        if parts.len() >= 3 {
            let start = parts[0].parse::<f64>().ok()? as u64;
            let end = parts[1].parse::<f64>().ok()? as u64;
            Some(TranscriptionSegment {
                start_ms: start,
                end_ms: end,
                text: parts[2].to_string(),
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_line() {
        let line = "0 1200  And so";
        let segment = SimulStreamingClient::parse_line(line).unwrap();
        assert_eq!(segment.start_ms, 0);
        assert_eq!(segment.end_ms, 1200);
        assert_eq!(segment.text, " And so");
    }

    #[test]
    fn test_parse_line_with_punctuation() {
        let line = "2400 3600 ,";
        let segment = SimulStreamingClient::parse_line(line).unwrap();
        assert_eq!(segment.start_ms, 2400);
        assert_eq!(segment.end_ms, 3600);
        assert_eq!(segment.text, ",");
    }

    #[test]
    fn test_parse_line_invalid() {
        assert!(SimulStreamingClient::parse_line("invalid").is_none());
        assert!(SimulStreamingClient::parse_line("").is_none());
        assert!(SimulStreamingClient::parse_line("100 200").is_none());
    }
}

