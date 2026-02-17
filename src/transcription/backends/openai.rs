use super::TranscriptionBackend;
use crate::transcription::TranscriptionSegment;
use anyhow::{Context, Result};
use async_trait::async_trait;
use log::{debug, error, info};
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use std::time::{Duration, Instant};

const SAMPLE_RATE: u32 = 16000;

pub struct OpenAiBackend {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
    site_url: Option<String>,
    app_name: Option<String>,
    audio_buffer: Arc<Mutex<Vec<i16>>>,
    result_rx: mpsc::Receiver<TranscriptionSegment>,
    // Internal channel to send results from tasks to result_rx
    internal_tx: mpsc::Sender<TranscriptionSegment>,
    last_send_time: Arc<Mutex<Instant>>,
    stream_time_ms: Arc<Mutex<u64>>,
}

impl OpenAiBackend {
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        let (internal_tx, result_rx) = mpsc::channel(100);
        Self {
            client: Client::new(),
            api_key,
            base_url: {
               let mut url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
               if url.ends_with("/models") {
                   url = url.trim_end_matches("/models").to_string();
               }
               url
            },
            model: model.unwrap_or_else(|| "whisper-1".to_string()),
            site_url: None, // Could be configured
            app_name: Some("SFU Server".to_string()),
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            result_rx,
            internal_tx,
            last_send_time: Arc::new(Mutex::new(Instant::now())),
            stream_time_ms: Arc::new(Mutex::new(0)),
        }
    }

    async fn flush(&self) -> Result<()> {
        let mut buffer = self.audio_buffer.lock().await;
        if buffer.is_empty() {
            return Ok(());
        }

        let audio_data = std::mem::take(&mut *buffer);
        let duration_ms = (audio_data.len() as u64 * 1000) / SAMPLE_RATE as u64;

        let start_ms = {
            let mut time = self.stream_time_ms.lock().await;
            let current = *time;
            *time += duration_ms;
            current
        };

        // Spawn a task to send the request so we don't block
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let model = self.model.clone();
        let site_url = self.site_url.clone();
        let app_name = self.app_name.clone();
        let tx = self.internal_tx.clone();

        let _start_time = *self.last_send_time.lock().await;
        // Update last send time broadly (approximation)
        // In a real implementation, we'd track timestamps more accurately

        tokio::spawn(async move {
            match Self::transcribe_chunk(client, api_key, base_url, model, site_url, app_name, audio_data).await {
                Ok(text) => {
                    if !text.trim().is_empty() {
                        let segment = TranscriptionSegment {
                            start_ms,
                            end_ms: start_ms + duration_ms,
                            text,
                        };
                        let _ = tx.send(segment).await;
                    }
                }
                Err(e) => error!("OpenAI transcription failed: {}", e),
            }
        });

        Ok(())
    }

    async fn transcribe_chunk(
        client: Client,
        api_key: String,
        base_url: String,
        model: String,
        site_url: Option<String>,
        app_name: Option<String>,
        pcm_data: Vec<i16>,
    ) -> Result<String> {
        let wav_data = create_wav(pcm_data, SAMPLE_RATE, 1);

        let part = reqwest::multipart::Part::bytes(wav_data)
            .file_name("audio.wav")
            .mime_str("audio/wav")?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", model);

        let url = format!("{}/audio/transcriptions", base_url.trim_end_matches('/'));

        let mut req = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key));

        if let Some(url) = site_url {
            req = req.header("HTTP-Referer", url);
        }
        if let Some(name) = app_name {
            req = req.header("X-Title", name);
        }

        let res = req
            .multipart(form)
            .send()
            .await
            .context("Failed to send request")?;

        if !res.status().is_success() {
            let error_text = res.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("API Error: {}", error_text));
        }

        let json: serde_json::Value = res.json().await?;
        let text = json["text"].as_str().unwrap_or("").to_string();

        Ok(text)
    }
}

#[async_trait]
impl TranscriptionBackend for OpenAiBackend {
    async fn send_audio(&mut self, data: &[i16]) -> Result<()> {
        let mut buffer = self.audio_buffer.lock().await;
        buffer.extend_from_slice(data);

        // Check size/time
        // Simplistic check: if buffer > 5 seconds
        if buffer.len() > (SAMPLE_RATE as usize * 5) { // 5 seconds
             drop(buffer); // release lock
             self.flush().await?;
             let mut last = self.last_send_time.lock().await;
             *last = Instant::now();
        }

        Ok(())
    }

    async fn next_segment(&mut self) -> Result<Option<TranscriptionSegment>> {
        Ok(self.result_rx.recv().await)
    }
}

pub struct OpenAiSyncBackend {
    audio_tx: std::sync::mpsc::Sender<Vec<i16>>,
    result_rx: std::sync::mpsc::Receiver<TranscriptionSegment>,
    _worker_thread: std::thread::JoinHandle<()>,
}

impl OpenAiSyncBackend {
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        let (audio_tx, audio_rx) = std::sync::mpsc::channel::<Vec<i16>>();
        let (result_tx, result_rx) = std::sync::mpsc::channel();

        let mut base_url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        if base_url.ends_with("/models") {
            base_url = base_url.trim_end_matches("/models").to_string();
        }
        let base_url = base_url;
        let model = model.unwrap_or_else(|| "whisper-1".to_string());

        let client = reqwest::blocking::Client::new();

        let worker_thread = std::thread::spawn(move || {
            Self::worker_loop(client, api_key, base_url, model, audio_rx, result_tx);
        });

        Self {
            audio_tx,
            result_rx,
            _worker_thread: worker_thread,
        }
    }

    fn worker_loop(
        client: reqwest::blocking::Client,
        api_key: String,
        base_url: String,
        model: String,
        audio_rx: std::sync::mpsc::Receiver<Vec<i16>>,
        result_tx: std::sync::mpsc::Sender<TranscriptionSegment>,
    ) {
        let mut buffer = Vec::new();
        // Accumulate until we have enough data or timeout
        // Since receiver blocks provided timeout, we can use recv_timeout
        // But mpsc::Receiver doesn't have recv_timeout easily unless we use crossbeam or simple loop
        // We'll use simple accumulation approach:
        // Try to read all available packets, if buffer reaches size, send.
        // If no packets, sleep briefly? Or use recv_timeout if available (std mpsc recv_timeout is available)

        let mut last_send = std::time::Instant::now();
        let mut stream_time_ms = 0u64;

        loop {
            // receive with timeout to ensure we flush periodically
            match audio_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(data) => {
                    buffer.extend_from_slice(&data);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // check if we should flush
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }

            let min_samples = (SAMPLE_RATE / 10) as usize; // 0.1s
            let should_flush = buffer.len() > (SAMPLE_RATE as usize * 5) || // 5 seconds of buffer
                (buffer.len() >= min_samples && last_send.elapsed() >= Duration::from_secs(5)); // OR periodic flush if we have enough data

            if should_flush {
                 let chunk = std::mem::take(&mut buffer);
                 let duration_ms = (chunk.len() as u64 * 1000) / SAMPLE_RATE as u64;
                 let start_ms = stream_time_ms;
                 stream_time_ms += duration_ms;

                 if let Err(e) = Self::transcribe_chunk_sync(&client, &api_key, &base_url, &model, chunk, start_ms, &result_tx) {
                     error!("Sync OpenAI transcription failed: {}", e);
                 }
                 last_send = std::time::Instant::now();
            }
        }
    }

    fn transcribe_chunk_sync(
        client: &reqwest::blocking::Client,
        api_key: &str,
        base_url: &str,
        model: &str,
        pcm_data: Vec<i16>,
        start_ms: u64,
        result_tx: &std::sync::mpsc::Sender<TranscriptionSegment>,
    ) -> Result<()> {
        if pcm_data.is_empty() { return Ok(()); }

        let duration_ms = (pcm_data.len() as u64 * 1000) / SAMPLE_RATE as u64;
        info!("Sending audio chunk of {}ms to OpenAI...", duration_ms);
        let wav_data = create_wav(pcm_data, SAMPLE_RATE, 1);

        let part = reqwest::blocking::multipart::Part::bytes(wav_data)
            .file_name("audio.wav")
            .mime_str("audio/wav")?;

        let form = reqwest::blocking::multipart::Form::new()
            .part("file", part)
            .text("model", model.to_string());

        let url = format!("{}/audio/transcriptions", base_url.trim_end_matches('/'));

        let res = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("X-Title", "SFU Server") // Default app name for sync backend
            .multipart(form)
            .send()
            .context("Failed to send request")?;

        if !res.status().is_success() {
            let error_text = res.text().unwrap_or_default();
            return Err(anyhow::anyhow!("API Error: {}", error_text));
        }

        let json: serde_json::Value = res.json()?;
        let text = json["text"].as_str().unwrap_or("").to_string();

        if !text.trim().is_empty() {
             info!("OpenAI Transcription received: '{}'", text);
             let segment = TranscriptionSegment {
                start_ms,
                end_ms: start_ms + duration_ms,
                text,
            };
            let _ = result_tx.send(segment);
        } else {
             info!("OpenAI Transcription received empty text");
        }

        Ok(())
    }
}

use super::SyncTranscriptionBackend;

impl SyncTranscriptionBackend for OpenAiSyncBackend {
    fn send_audio(&mut self, data: &[i16]) -> Result<()> {
        // Just send to the worker thread
        self.audio_tx.send(data.to_vec()).map_err(|_| anyhow::anyhow!("Worker thread dead"))
    }

    fn try_recv(&mut self) -> Option<TranscriptionSegment> {
        self.result_rx.try_recv().ok()
    }
}

fn create_wav(pcm: Vec<i16>, sample_rate: u32, channels: u16) -> Vec<u8> {
    let mut header = Vec::with_capacity(44 + pcm.len() * 2);
    let data_len = (pcm.len() * 2) as u32;
    let total_len = 36 + data_len;

    // RIFF header
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&total_len.to_le_bytes());
    header.extend_from_slice(b"WAVE");

    // fmt chunk
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    header.extend_from_slice(&1u16.to_le_bytes()); // PCM
    header.extend_from_slice(&channels.to_le_bytes());
    header.extend_from_slice(&sample_rate.to_le_bytes());
    header.extend_from_slice(&(sample_rate * 2 * channels as u32).to_le_bytes()); // byte rate
    header.extend_from_slice(&(2 * channels).to_le_bytes()); // block align
    header.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data chunk
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_len.to_le_bytes());

    for sample in pcm {
        header.extend_from_slice(&sample.to_le_bytes());
    }

    header
}
