//! Client Duplex Audio Frame Jitter Buffer & Microphone Passthrough Engine.
//!
//! Provides bounded low-latency speaker playback buffering (`ClientAudioBuffer`) for incoming
//! 20 ms 48 kHz stereo `AudioDirection::HostToClientSpeaker` frames (Opus or PCM16LE) with
//! overflow eviction, underrun telemetry (`AudioBufferStats`), and client microphone frame
//! packaging (`AudioDirection::ClientToHostMicrophone`).

use std::collections::VecDeque;

use anyhow::{Result, bail, ensure};
use rferram_protocol::{AudioCodec, AudioDirection, AudioFrameMessage};

/// Default client audio jitter buffer capacity in frames (32 frames = 640 ms at 20 ms/frame).
pub const DEFAULT_AUDIO_JITTER_CAPACITY: usize = 32;

/// Standard 48 kHz audio sample rate used across rFerram Opus and PCM streams.
pub const DEFAULT_AUDIO_SAMPLE_RATE_HZ: u32 = 48_000;

/// Standard stereo channel count.
pub const DEFAULT_AUDIO_CHANNELS: u8 = 2;

/// Standard 20 ms stereo Opus encoded payload size in bytes.
pub const DEFAULT_OPUS_FRAME_BYTES: usize = 192;

/// Telemetry counters tracked by [`ClientAudioBuffer`] during an active remote session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioBufferStats {
    /// Total valid speaker audio frames enqueued.
    pub frames_enqueued: u64,
    /// Total speaker audio frames popped for playback.
    pub frames_dequeued: u64,
    /// Total oldest frames evicted when the jitter buffer reached maximum capacity.
    pub frames_dropped_overflow: u64,
    /// Total frames rejected due to non-speaker direction (`ClientToHostMicrophone`).
    pub frames_dropped_direction: u64,
    /// Total playback dequeue attempts made while the buffer was empty.
    pub underruns: u64,
    /// Current total payload bytes residing in the jitter buffer.
    pub bytes_buffered: usize,
}

/// Bounded low-latency audio frame jitter buffer for client speaker playback.
#[derive(Debug, Clone)]
pub struct ClientAudioBuffer {
    queue: VecDeque<AudioFrameMessage>,
    max_capacity: usize,
    stats: AudioBufferStats,
}

impl Default for ClientAudioBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_AUDIO_JITTER_CAPACITY)
    }
}

impl ClientAudioBuffer {
    /// Creates a new [`ClientAudioBuffer`] with the specified maximum frame capacity (clamped to `>= 1`).
    pub fn new(max_capacity: usize) -> Self {
        let cap = max_capacity.max(1);
        Self {
            queue: VecDeque::with_capacity(cap),
            max_capacity: cap,
            stats: AudioBufferStats::default(),
        }
    }

    /// Returns the maximum number of audio frames the buffer can hold before evicting oldest frames.
    pub fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    /// Returns the number of audio frames currently buffered.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Returns `true` if no audio frames are currently buffered.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Returns a snapshot reference to the buffer's [`AudioBufferStats`].
    pub fn stats(&self) -> &AudioBufferStats {
        &self.stats
    }

    /// Peeks at the next [`AudioFrameMessage`] scheduled for playback without removing it.
    pub fn peek_frame(&self) -> Option<&AudioFrameMessage> {
        self.queue.front()
    }

    /// Clears all buffered audio frames and resets `bytes_buffered` to `0`.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.stats.bytes_buffered = 0;
    }

    /// Validates and enqueues an incoming [`AudioFrameMessage`] destined for client speaker output.
    ///
    /// If the buffer is at `max_capacity`, the oldest frame is evicted (`frames_dropped_overflow`
    /// is incremented) so playback latency remains strictly bounded.
    pub fn push_frame(&mut self, frame: AudioFrameMessage) -> Result<()> {
        if frame.direction != AudioDirection::HostToClientSpeaker {
            self.stats.frames_dropped_direction += 1;
            bail!(
                "ClientAudioBuffer rejected frame {} with non-speaker direction {:?}",
                frame.stream_id,
                frame.direction
            );
        }
        ensure!(
            frame.channels > 0,
            "AudioFrameMessage stream {} must specify at least 1 channel",
            frame.stream_id
        );
        ensure!(
            frame.sample_rate > 0,
            "AudioFrameMessage stream {} must specify a non-zero sample rate",
            frame.stream_id
        );
        ensure!(
            !frame.payload.is_empty(),
            "AudioFrameMessage stream {} payload must not be empty",
            frame.stream_id
        );

        if self.queue.len() >= self.max_capacity
            && let Some(evicted) = self.queue.pop_front()
        {
            self.stats.bytes_buffered = self
                .stats
                .bytes_buffered
                .saturating_sub(evicted.payload.len());
            self.stats.frames_dropped_overflow += 1;
        }

        self.stats.bytes_buffered = self
            .stats
            .bytes_buffered
            .saturating_add(frame.payload.len());
        self.stats.frames_enqueued += 1;
        self.queue.push_back(frame);
        Ok(())
    }

    /// Pops the oldest buffered [`AudioFrameMessage`] for audio device playback.
    ///
    /// Increments `stats.underruns` when called on an empty buffer.
    pub fn pop_frame(&mut self) -> Option<AudioFrameMessage> {
        if let Some(frame) = self.queue.pop_front() {
            self.stats.bytes_buffered = self
                .stats
                .bytes_buffered
                .saturating_sub(frame.payload.len());
            self.stats.frames_dequeued += 1;
            Some(frame)
        } else {
            self.stats.underruns += 1;
            None
        }
    }

    /// Builds a validated [`AudioFrameMessage`] for client-to-host microphone passthrough.
    pub fn build_microphone_frame(
        stream_id: u32,
        timestamp_us: u64,
        codec: AudioCodec,
        payload: Vec<u8>,
    ) -> Result<AudioFrameMessage> {
        ensure!(
            !payload.is_empty(),
            "Microphone audio frame payload must not be empty"
        );
        Ok(AudioFrameMessage {
            stream_id,
            timestamp_us,
            direction: AudioDirection::ClientToHostMicrophone,
            codec,
            channels: DEFAULT_AUDIO_CHANNELS,
            sample_rate: DEFAULT_AUDIO_SAMPLE_RATE_HZ,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_speaker_frame(stream_id: u32, payload_len: usize) -> AudioFrameMessage {
        AudioFrameMessage {
            stream_id,
            timestamp_us: u64::from(stream_id) * 20_000,
            direction: AudioDirection::HostToClientSpeaker,
            codec: AudioCodec::Opus,
            channels: DEFAULT_AUDIO_CHANNELS,
            sample_rate: DEFAULT_AUDIO_SAMPLE_RATE_HZ,
            payload: vec![0x5A; payload_len],
        }
    }

    #[test]
    fn test_audio_buffer_enqueue_dequeue_and_underrun_tracking() {
        let mut buf = ClientAudioBuffer::new(8);
        assert!(buf.is_empty());
        assert_eq!(buf.max_capacity(), 8);

        // Popping from empty buffer increments underrun count
        assert!(buf.pop_frame().is_none());
        assert_eq!(buf.stats().underruns, 1);

        buf.push_frame(sample_speaker_frame(1, 192)).unwrap();
        buf.push_frame(sample_speaker_frame(2, 192)).unwrap();
        assert_eq!(buf.len(), 2);
        assert_eq!(buf.stats().frames_enqueued, 2);
        assert_eq!(buf.stats().bytes_buffered, 384);
        assert_eq!(buf.peek_frame().unwrap().stream_id, 1);

        let f1 = buf.pop_frame().unwrap();
        assert_eq!(f1.stream_id, 1);
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.stats().frames_dequeued, 1);
        assert_eq!(buf.stats().bytes_buffered, 192);

        let f2 = buf.pop_frame().unwrap();
        assert_eq!(f2.stream_id, 2);
        assert!(buf.is_empty());
        assert_eq!(buf.stats().bytes_buffered, 0);
    }

    #[test]
    fn test_audio_buffer_overflow_evicts_oldest_frame() {
        let mut buf = ClientAudioBuffer::new(3);
        buf.push_frame(sample_speaker_frame(10, 100)).unwrap();
        buf.push_frame(sample_speaker_frame(11, 100)).unwrap();
        buf.push_frame(sample_speaker_frame(12, 100)).unwrap();
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.stats().frames_dropped_overflow, 0);

        // Pushing a 4th frame into a capacity-3 buffer evicts stream_id 10
        buf.push_frame(sample_speaker_frame(13, 150)).unwrap();
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.stats().frames_dropped_overflow, 1);
        assert_eq!(buf.stats().bytes_buffered, 350);

        let first_remaining = buf.pop_frame().unwrap();
        assert_eq!(first_remaining.stream_id, 11);
    }

    #[test]
    fn test_audio_buffer_rejects_wrong_direction_and_invalid_frames() {
        let mut buf = ClientAudioBuffer::default();

        let mic_frame = AudioFrameMessage {
            stream_id: 1,
            timestamp_us: 1000,
            direction: AudioDirection::ClientToHostMicrophone,
            codec: AudioCodec::Opus,
            channels: 2,
            sample_rate: 48_000,
            payload: vec![1, 2, 3, 4],
        };
        assert!(buf.push_frame(mic_frame).is_err());
        assert_eq!(buf.stats().frames_dropped_direction, 1);
        assert!(buf.is_empty());

        let empty_payload = AudioFrameMessage {
            stream_id: 2,
            timestamp_us: 2000,
            direction: AudioDirection::HostToClientSpeaker,
            codec: AudioCodec::Opus,
            channels: 2,
            sample_rate: 48_000,
            payload: vec![],
        };
        assert!(buf.push_frame(empty_payload).is_err());

        let zero_channels = AudioFrameMessage {
            stream_id: 3,
            timestamp_us: 3000,
            direction: AudioDirection::HostToClientSpeaker,
            codec: AudioCodec::Opus,
            channels: 0,
            sample_rate: 48_000,
            payload: vec![0xAA; 64],
        };
        assert!(buf.push_frame(zero_channels).is_err());
    }

    #[test]
    fn test_build_microphone_frame_validation() {
        assert!(
            ClientAudioBuffer::build_microphone_frame(1, 20_000, AudioCodec::Opus, vec![]).is_err()
        );

        let mic = ClientAudioBuffer::build_microphone_frame(
            7,
            140_000,
            AudioCodec::Opus,
            vec![0x11; DEFAULT_OPUS_FRAME_BYTES],
        )
        .unwrap();
        assert_eq!(mic.stream_id, 7);
        assert_eq!(mic.direction, AudioDirection::ClientToHostMicrophone);
        assert_eq!(mic.channels, DEFAULT_AUDIO_CHANNELS);
        assert_eq!(mic.sample_rate, DEFAULT_AUDIO_SAMPLE_RATE_HZ);
        assert_eq!(mic.payload.len(), DEFAULT_OPUS_FRAME_BYTES);
    }
}
