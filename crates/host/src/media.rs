use std::{collections::VecDeque, sync::{Arc, Mutex}, time::{Duration, Instant}};

pub const AUDIO_RATE: u32 = 16_000;
const AUDIO_BYTES: usize = AUDIO_RATE as usize * 2 * 3;

#[derive(Default)]
pub struct EventMedia {
    pub photo: Option<Arc<[u8]>>,
    pub audio: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct Media(Mutex<Buffers>);

#[derive(Default)]
struct Buffers {
    photo: Option<(Instant, Arc<[u8]>)>,
    audio: VecDeque<u8>,
    audio_at: Option<Instant>,
}

impl Media {
    pub fn set_photo(&self, photo: Vec<u8>) {
        self.0.lock().unwrap().photo = Some((Instant::now(), photo.into()));
    }

    pub fn add_audio(&self, pcm: &[u8]) {
        let mut buffer = self.0.lock().unwrap();
        let excess = (buffer.audio.len() + pcm.len()).saturating_sub(AUDIO_BYTES);
        buffer.audio.drain(..excess);
        buffer.audio.extend(pcm);
        buffer.audio_at = Some(Instant::now());
    }

    pub fn clear_photo(&self) {
        self.0.lock().unwrap().photo = None;
    }

    pub fn clear_audio(&self) {
        let mut buffer = self.0.lock().unwrap();
        buffer.audio.clear();
        buffer.audio_at = None;
    }

    pub fn photo(&self) -> Option<Arc<[u8]>> {
        self.0.lock().unwrap().fresh_photo()
    }

    pub fn snapshot(&self) -> EventMedia {
        let buffer = self.0.lock().unwrap();
        let audio = buffer.audio_at.filter(|at| at.elapsed() < Duration::from_secs(1))
            .map(|_| wav(&buffer.audio));
        EventMedia { photo: buffer.fresh_photo(), audio }
    }
}

impl Buffers {
    fn fresh_photo(&self) -> Option<Arc<[u8]>> {
        self.photo.as_ref()
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(2))
            .map(|(_, photo)| photo.clone())
    }
}

fn wav(pcm: &VecDeque<u8>) -> Vec<u8> {
    let audio_len = pcm.len() as u32;
    let mut output = Vec::with_capacity(44 + pcm.len());
    output.extend(b"RIFF");
    output.extend((36 + audio_len).to_le_bytes());
    output.extend(b"WAVEfmt ");
    output.extend(16_u32.to_le_bytes()); // PCM format header length
    output.extend(1_u16.to_le_bytes()); // PCM
    output.extend(1_u16.to_le_bytes()); // mono
    output.extend(AUDIO_RATE.to_le_bytes());
    output.extend((AUDIO_RATE * 2).to_le_bytes()); // bytes per second
    output.extend(2_u16.to_le_bytes()); // bytes per sample
    output.extend(16_u16.to_le_bytes());
    output.extend(b"data");
    output.extend(audio_len.to_le_bytes());
    output.extend(pcm);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_audio_contains_only_recent_samples_and_a_wav_header() {
        let media = Media::default();
        for n in 0..61 {
            media.add_audio(&[n; 1600]);
        }
        let audio = media.snapshot().audio.unwrap();
        assert_eq!(&audio[..4], b"RIFF");
        assert_eq!(&audio[8..12], b"WAVE");
        assert_eq!(&audio[40..44], &96_000_u32.to_le_bytes());
        assert_eq!(audio[44], 1);
        assert_eq!(audio.last().copied(), Some(60));
        media.clear_audio();
        assert!(media.snapshot().audio.is_none());
    }
}
