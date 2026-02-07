//! Audio decoding and resampling for transcription.
//!
//! WebRTC typically uses OPUS codec at 48kHz, but SimulStreaming
//! requires raw PCM at 16kHz. This module handles the conversion.

use audiopus::{coder::Decoder as OpusDecoder, packet::Packet, MutSignals, Channels, SampleRate};
use log::{debug, trace};
use rubato::{FftFixedIn, Resampler};

use super::{SIMULSTREAMING_SAMPLE_RATE, WEBRTC_SAMPLE_RATE};

/// Audio resampler that converts 48kHz audio to 16kHz.
///
/// SimulStreaming expects 16kHz audio, but WebRTC typically uses 48kHz.
/// This resampler handles the conversion efficiently using FFT-based resampling.
pub struct AudioResampler {
    resampler: FftFixedIn<f32>,
    input_buffer: Vec<f32>,
    chunk_size: usize,
}

impl AudioResampler {
    /// Create a new resampler from 48kHz to 16kHz.
    ///
    /// # Arguments
    /// * `chunk_size` - Number of input samples to process at once (default: 960)
    pub fn new() -> anyhow::Result<Self> {
        Self::with_chunk_size(960) // 20ms at 48kHz
    }

    /// Create a new resampler with a custom chunk size.
    pub fn with_chunk_size(chunk_size: usize) -> anyhow::Result<Self> {
        let resampler = FftFixedIn::<f32>::new(
            WEBRTC_SAMPLE_RATE as usize,         // input sample rate: 48kHz
            SIMULSTREAMING_SAMPLE_RATE as usize, // output sample rate: 16kHz
            chunk_size,                          // chunk size
            2,                                   // sub chunks
            1,                                   // channels (mono)
        )?;

        Ok(Self {
            resampler,
            input_buffer: Vec::with_capacity(chunk_size * 2),
            chunk_size,
        })
    }

    /// Resample audio from 48kHz to 16kHz.
    ///
    /// # Arguments
    /// * `input` - PCM samples at 48kHz (i16)
    ///
    /// # Returns
    /// PCM samples at 16kHz (i16)
    pub fn resample(&mut self, input: &[i16]) -> anyhow::Result<Vec<i16>> {
        // Convert to f32 and add to buffer
        for &sample in input {
            self.input_buffer.push(sample as f32 / 32768.0);
        }

        let mut output = Vec::new();

        // Process complete chunks
        while self.input_buffer.len() >= self.chunk_size {
            let chunk: Vec<f32> = self.input_buffer.drain(..self.chunk_size).collect();

            let resampled = self.resampler.process(&[chunk], None)?;

            // Convert back to i16
            for &sample in &resampled[0] {
                let clamped = sample.clamp(-1.0, 1.0);
                output.push((clamped * 32767.0) as i16);
            }
        }

        trace!("Resampled {} -> {} samples", input.len(), output.len());
        Ok(output)
    }

    /// Flush any remaining samples in the buffer.
    pub fn flush(&mut self) -> anyhow::Result<Vec<i16>> {
        if self.input_buffer.is_empty() {
            return Ok(Vec::new());
        }

        // Pad with zeros to make a complete chunk
        while self.input_buffer.len() < self.chunk_size {
            self.input_buffer.push(0.0);
        }

        let chunk: Vec<f32> = self.input_buffer.drain(..).collect();
        let resampled = self.resampler.process(&[chunk], None)?;

        let mut output = Vec::new();
        for &sample in &resampled[0] {
            let clamped = sample.clamp(-1.0, 1.0);
            output.push((clamped * 32767.0) as i16);
        }

        Ok(output)
    }

    /// Reset the resampler state.
    pub fn reset(&mut self) {
        self.input_buffer.clear();
        self.resampler.reset();
    }
}

impl Default for AudioResampler {
    fn default() -> Self {
        Self::new().expect("Failed to create default AudioResampler")
    }
}

/// Audio decoder that decodes OPUS packets and resamples to 16kHz.
///
/// This combines OPUS decoding with resampling in a single convenient interface.
pub struct AudioDecoder {
    decoder: OpusDecoder,
    resampler: AudioResampler,
    decode_buffer: Vec<i16>,
}

impl AudioDecoder {
    /// Create a new audio decoder.
    ///
    /// The decoder is configured for:
    /// - 48kHz sample rate (WebRTC standard)
    /// - Mono channel
    /// - Output resampled to 16kHz
    pub fn new() -> anyhow::Result<Self> {
        let decoder = OpusDecoder::new(SampleRate::Hz48000, Channels::Mono)?;

        let resampler = AudioResampler::new()?;

        // OPUS frame size: up to 120ms at 48kHz = 5760 samples
        let decode_buffer = vec![0i16; 5760];

        debug!("Created AudioDecoder (48kHz OPUS -> 16kHz PCM)");

        Ok(Self {
            decoder,
            resampler,
            decode_buffer,
        })
    }

    /// Decode an OPUS packet and resample to 16kHz.
    ///
    /// # Arguments
    /// * `opus_data` - Raw OPUS packet data
    ///
    /// # Returns
    /// PCM samples at 16kHz, mono, suitable for SimulStreaming
    pub fn decode(&mut self, opus_data: &[u8]) -> anyhow::Result<Vec<i16>> {
        // Create packet wrapper
        let packet = Packet::try_from(opus_data)?;

        // Create output signal wrapper
        let output = MutSignals::try_from(&mut self.decode_buffer[..])?;

        // Decode OPUS to 48kHz PCM
        let samples = self.decoder.decode(Some(packet), output, false)?;

        trace!(
            "Decoded {} bytes OPUS -> {} samples",
            opus_data.len(),
            samples
        );

        // Resample to 16kHz
        let resampled = self.resampler.resample(&self.decode_buffer[..samples])?;

        Ok(resampled)
    }

    /// Decode with Forward Error Correction (FEC) for packet loss.
    ///
    /// # Arguments
    /// * `opus_data` - OPUS packet (can be None for PLC)
    /// * `fec` - Whether to use FEC data
    pub fn decode_with_fec(
        &mut self,
        opus_data: Option<&[u8]>,
        fec: bool,
    ) -> anyhow::Result<Vec<i16>> {
        let packet = match opus_data {
            Some(data) => Some(Packet::try_from(data)?),
            None => None,
        };

        let output = MutSignals::try_from(&mut self.decode_buffer[..])?;
        let samples = self.decoder.decode(packet, output, fec)?;

        let resampled = self.resampler.resample(&self.decode_buffer[..samples])?;
        Ok(resampled)
    }

    /// Flush any remaining audio in the resampler buffer.
    pub fn flush(&mut self) -> anyhow::Result<Vec<i16>> {
        self.resampler.flush()
    }

    /// Reset the decoder and resampler state.
    pub fn reset(&mut self) -> anyhow::Result<()> {
        use audiopus::coder::GenericCtl;
        self.decoder.reset_state()?;
        self.resampler.reset();
        Ok(())
    }
}

impl Default for AudioDecoder {
    fn default() -> Self {
        Self::new().expect("Failed to create default AudioDecoder")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resampler_ratio() {
        let mut resampler = AudioResampler::new().unwrap();

        // 960 samples at 48kHz = 20ms
        // Should produce ~320 samples at 16kHz
        let input: Vec<i16> = vec![0; 960];
        let output = resampler.resample(&input).unwrap();

        // Allow some tolerance due to resampling algorithm
        assert!(
            output.len() >= 300 && output.len() <= 340,
            "Expected ~320 samples, got {}",
            output.len()
        );
    }

    #[test]
    fn test_resampler_preserves_silence() {
        let mut resampler = AudioResampler::new().unwrap();

        let input: Vec<i16> = vec![0; 960];
        let output = resampler.resample(&input).unwrap();

        // All output samples should be near zero
        for sample in output {
            assert!(sample.abs() < 10, "Non-silent sample: {}", sample);
        }
    }
}

