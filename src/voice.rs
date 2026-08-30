//! Voice messages the way WhatsApp sends them: Opus in an OGG container,
//! one channel, 48 kHz. Both directions are handled here with libopus built
//! into the app, so playing and recording need nothing installed.

use std::io::Cursor;

/// Opus runs at 48 kHz; every clip is handled at that rate, in mono.
pub const RATE: u32 = 48_000;
/// One Opus frame: 20 ms.
const FRAME: usize = 960;
/// The most one packet can decode to: 120 ms of stereo.
const LONGEST_PACKET: usize = 5760 * 2;
/// Bars in the waveform WhatsApp draws for a voice message.
pub const BARS: usize = 64;
/// Enough for speech; the phone sends about this much.
const BITRATE: i32 = 32_000;

/// Mono samples at `RATE` from an OGG/Opus file.
pub fn decode(bytes: &[u8]) -> Result<Vec<f32>, String> {
    let mut reader = ogg::PacketReader::new(Cursor::new(bytes));
    let mut stream: Option<Stream> = None;
    let mut out = Vec::new();
    let mut scratch = vec![0f32; LONGEST_PACKET];
    loop {
        let packet = match reader.read_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(error) => return Err(format!("bad OGG stream: {error}")),
        };
        let Some(current) = stream.as_mut() else {
            stream = Some(Stream::open(&packet.data)?);
            continue;
        };
        if !current.tagged {
            // The comment header; nothing a player needs.
            current.tagged = true;
            continue;
        }
        let frames = current
            .decoder
            .decode_float(&packet.data, &mut scratch, false)
            .map_err(|error| format!("bad Opus packet: {error}"))?;
        let decoded = &scratch[..frames * current.channels];
        let mono: Vec<f32> = if current.channels == 2 {
            decoded
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[left, right]| (left + right) * 0.5)
                .collect()
        } else {
            decoded.to_vec()
        };
        // The encoder's lookahead comes out first and is not audio.
        let skip = current.skip.min(mono.len());
        current.skip -= skip;
        out.extend_from_slice(&mono[skip..]);
    }
    if stream.is_none() {
        return Err("not an OGG stream".to_owned());
    }
    Ok(out)
}

struct Stream {
    decoder: opus::Decoder,
    channels: usize,
    skip: usize,
    tagged: bool,
}

impl Stream {
    /// Reads the identification header that opens every Opus stream.
    fn open(head: &[u8]) -> Result<Self, String> {
        let head = head
            .strip_prefix(b"OpusHead")
            .ok_or_else(|| "not an Opus stream".to_owned())?;
        if head.len() < 11 {
            return Err("truncated Opus header".to_owned());
        }
        let channels = usize::from(head[1]);
        let skip = usize::from(u16::from_le_bytes([head[2], head[3]]));
        let layout = match channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            other => return Err(format!("{other} channels")),
        };
        let decoder = opus::Decoder::new(RATE, layout).map_err(|error| error.to_string())?;
        Ok(Self {
            decoder,
            channels,
            skip,
            tagged: false,
        })
    }
}

/// An OGG/Opus file from mono samples at `RATE`.
pub fn encode(samples: &[f32]) -> Result<Vec<u8>, String> {
    use ogg::PacketWriteEndInfo::{EndPage, EndStream, NormalPacket};
    let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip)
        .map_err(|error| error.to_string())?;
    encoder
        .set_bitrate(opus::Bitrate::Bits(BITRATE))
        .map_err(|error| error.to_string())?;
    let pre_skip = encoder
        .get_lookahead()
        .map_err(|error| error.to_string())?
        .max(0) as usize;
    let serial = 0x5641_5341;
    let io = |error: std::io::Error| error.to_string();
    let mut writer = ogg::PacketWriter::new(Vec::new());
    writer
        .write_packet(opus_head(pre_skip as u16), serial, EndPage, 0)
        .map_err(io)?;
    writer
        .write_packet(opus_tags(), serial, EndPage, 0)
        .map_err(io)?;
    // The lookahead comes out after the input, so the clip is followed by
    // enough silence to flush it; the final granule position trims that.
    let frames = (samples.len() + pre_skip).div_ceil(FRAME).max(1);
    let mut frame = vec![0f32; FRAME];
    let mut packet = vec![0u8; 4000];
    for index in 0..frames {
        frame.fill(0.0);
        let start = index * FRAME;
        if start < samples.len() {
            let end = (start + FRAME).min(samples.len());
            frame[..end - start].copy_from_slice(&samples[start..end]);
        }
        let written = encoder
            .encode_float(&frame, &mut packet)
            .map_err(|error| error.to_string())?;
        let last = index + 1 == frames;
        let granule = if last {
            (pre_skip + samples.len()) as u64
        } else {
            ((index + 1) * FRAME) as u64
        };
        let end = if last {
            EndStream
        } else if (index + 1) % 50 == 0 {
            EndPage
        } else {
            NormalPacket
        };
        writer
            .write_packet(packet[..written].to_vec(), serial, end, granule)
            .map_err(io)?;
    }
    Ok(writer.into_inner())
}

fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(1);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&RATE.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    head
}

fn opus_tags() -> Vec<u8> {
    let vendor = b"Fastsapp";
    let mut tags = Vec::with_capacity(20 + vendor.len());
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    tags
}

/// Brings a quiet recording up to speaking volume: the loudest moment
/// lands just under full scale, with the gain capped so a silent room's
/// noise is not blasted instead.
pub fn normalize(samples: &mut [f32]) {
    let peak = samples
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    if peak <= 0.0 {
        return;
    }
    let gain = (0.89 / peak).clamp(1.0, 10.0);
    if gain > 1.0 {
        for sample in samples {
            *sample *= gain;
        }
    }
}

/// The bars WhatsApp draws: loudness per slice of the clip, 0 to 100 with
/// the loudest slice at 100.
pub fn waveform(samples: &[f32]) -> Vec<u8> {
    if samples.is_empty() {
        return vec![0; BARS];
    }
    let slice = samples.len().div_ceil(BARS);
    let loudness: Vec<f32> = samples
        .chunks(slice)
        .map(|chunk| (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt())
        .collect();
    let loudest = loudness.iter().copied().fold(0.0f32, f32::max);
    let mut bars: Vec<u8> = loudness
        .iter()
        .map(|value| {
            if loudest > 0.0 {
                (value / loudest * 100.0).round() as u8
            } else {
                0
            }
        })
        .collect();
    bars.resize(BARS, 0);
    bars
}

/// Mixes interleaved channels down to one and brings any rate to `RATE`.
pub fn mono_at_rate(interleaved: &[f32], channels: u16, rate: u32) -> Vec<f32> {
    let channels = usize::from(channels.max(1));
    let mono: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();
    if rate == RATE || rate == 0 || mono.is_empty() {
        return mono;
    }
    let ratio = f64::from(rate) / f64::from(RATE);
    let count = (mono.len() as f64 / ratio).floor() as usize;
    (0..count)
        .map(|index| {
            let position = index as f64 * ratio;
            let left = position.floor() as usize;
            let t = (position - left as f64) as f32;
            let a = mono[left.min(mono.len() - 1)];
            let b = mono.get(left + 1).copied().unwrap_or(a);
            a + (b - a) * t
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32) -> Vec<f32> {
        (0..(RATE as f32 * seconds) as usize)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5)
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn a_clip_survives_the_trip_through_opus() {
        let original = tone(1.0);
        let bytes = encode(&original).expect("encodes");
        assert!(bytes.starts_with(b"OggS"));
        let decoded = decode(&bytes).expect("decodes");
        let drift = decoded.len() as i64 - original.len() as i64;
        assert!(
            drift.abs() <= FRAME as i64,
            "{} samples back",
            decoded.len()
        );
        let end = decoded.len().min(original.len());
        let (before, after) = (rms(&original[4800..end]), rms(&decoded[4800..end]));
        assert!((before - after).abs() < 0.08, "{before} in, {after} out");
    }

    #[test]
    fn the_waveform_follows_the_loudness() {
        let mut samples = tone(1.0);
        let count = samples.len() as f32;
        for (index, sample) in samples.iter_mut().enumerate() {
            *sample *= index as f32 / count;
        }
        let bars = waveform(&samples);
        assert_eq!(bars.len(), BARS);
        assert_eq!(bars[BARS - 1], 100);
        assert!(bars[0] < 10, "{}", bars[0]);
        assert!(bars.windows(2).all(|pair| pair[0] <= pair[1] + 2));
        assert_eq!(waveform(&[]), vec![0; BARS]);
    }

    #[test]
    fn any_input_becomes_mono_at_48k() {
        let stereo: Vec<f32> = (0..44_100 * 2)
            .map(|i| if i % 2 == 0 { 1.0 } else { 0.0 })
            .collect();
        let mono = mono_at_rate(&stereo, 2, 44_100);
        assert!((mono.len() as i64 - 48_000).abs() <= 2, "{}", mono.len());
        assert!(mono.iter().all(|v| (v - 0.5).abs() < 1e-6));
        assert_eq!(mono_at_rate(&[0.25; 10], 1, RATE), vec![0.25; 10]);
    }

    #[test]
    fn a_quiet_recording_is_brought_up_but_not_blasted() {
        let mut quiet: Vec<f32> = tone(0.1).iter().map(|sample| sample * 0.2).collect();
        normalize(&mut quiet);
        let peak = quiet.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!((peak - 0.89).abs() < 0.01, "{peak}");
        // A whisper of noise gains at most twenty decibels.
        let mut faint = vec![0.001f32, -0.002];
        normalize(&mut faint);
        assert!((faint[1] + 0.02).abs() < 1e-6, "{}", faint[1]);
        // What is already loud is left alone.
        let mut loud = vec![0.95f32];
        normalize(&mut loud);
        assert_eq!(loud, vec![0.95]);
        normalize(&mut []);
    }

    #[test]
    fn what_is_not_opus_is_refused() {
        assert!(decode(b"not an ogg file at all").is_err());
        assert!(decode(&[]).is_err());
    }

    /// Decodes a real voice message and reports on it:
    /// `FASTSAPP_OGG_PROBE=note.ogg cargo test voice::tests::probe -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a file to look at"]
    fn probe() {
        let Some(path) = std::env::var_os("FASTSAPP_OGG_PROBE") else {
            return;
        };
        let bytes = std::fs::read(path).expect("readable");
        let started = std::time::Instant::now();
        let samples = decode(&bytes).expect("decodes");
        eprintln!(
            "{} samples ({:.2} s), loudness {:.3}, in {:?}; bars {:?}",
            samples.len(),
            samples.len() as f32 / RATE as f32,
            rms(&samples),
            started.elapsed(),
            &waveform(&samples)[..8]
        );
        assert!(!samples.is_empty());
    }
}
