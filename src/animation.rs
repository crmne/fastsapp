//! Moving pictures: WhatsApp's GIFs (short MP4 videos), animated stickers
//! (animated WebP), and GIF files.
//!
//! Frames are decoded once, off the interface thread, into textures the
//! view cycles through. WebP and GIF decode in-process; MP4 goes through
//! the `ffmpeg` command when the desktop has one, and shows its poster
//! otherwise. Decoded animations are dropped again once nothing has drawn
//! them for a while, since a chat can hold a great many.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{ColorImage, TextureHandle, TextureOptions};

/// Frames are scaled down to at most this wide before upload.
const MAX_WIDTH: u32 = 320;
/// And no more than this many are kept; a long GIF loops early.
const MAX_FRAMES: usize = 150;
/// How long an animation nobody has drawn stays decoded.
const IDLE: Duration = Duration::from_secs(20);
/// Decoders running at once; a sticker-heavy chat queues the rest.
const MAX_DECODERS: usize = 2;
/// Frames kept as textures across every animation; the least recently
/// drawn go first when this is exceeded.
const MAX_RESIDENT_FRAMES: usize = 450;

static DECODING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Holds one of the decoder slots for as long as it lives.
struct DecodeSlot;

impl Drop for DecodeSlot {
    fn drop(&mut self) {
        DECODING.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

struct Decoded {
    frames: Vec<(ColorImage, Duration)>,
}

struct Playing {
    frames: Vec<(TextureHandle, Duration)>,
    total: Duration,
    started: Instant,
    last_drawn: Instant,
}

enum Entry {
    Decoding,
    Failed,
    Ready(Playing),
}

#[derive(Clone, Default)]
struct Cache(Arc<Mutex<HashMap<PathBuf, Entry>>>);

/// What a decoder thread delivers: the file, and its frames if any.
type Delivery = (PathBuf, Option<Decoded>);

/// Decoded frames waiting to become textures; filled by decoder threads.
#[derive(Clone, Default)]
struct Inbox(Arc<Mutex<Vec<Delivery>>>);

fn cache(ctx: &egui::Context) -> Cache {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Cache>(egui::Id::new("animations"))
            .clone()
    })
}

fn inbox(ctx: &egui::Context) -> Inbox {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Inbox>(egui::Id::new("animation-inbox"))
            .clone()
    })
}

/// What to show for an animated file right now.
pub enum Frame {
    /// The picture for this instant.
    Ready(TextureHandle),
    /// Still decoding; show the poster.
    Pending,
    /// Cannot be played here; show the poster and offer to open it.
    Unavailable,
}

/// The frame to draw for `path` at this moment, starting the decode the
/// first time. Asks for a repaint when the next frame is due.
pub fn frame(ctx: &egui::Context, path: &Path) -> Frame {
    let cache = cache(ctx);
    let inbox = inbox(ctx);
    // Move what the decoders delivered into textures, on this thread.
    let arrived: Vec<Delivery> =
        std::mem::take(&mut *inbox.0.lock().unwrap_or_else(|p| p.into_inner()));
    let mut entries = cache.0.lock().unwrap_or_else(|p| p.into_inner());
    for (arrived_path, decoded) in arrived {
        let entry = match decoded {
            Some(decoded) if !decoded.frames.is_empty() => {
                let mut total = Duration::ZERO;
                let frames = decoded
                    .frames
                    .into_iter()
                    .enumerate()
                    .map(|(index, (image, delay))| {
                        total += delay;
                        let name = format!("{}#{index}", arrived_path.display());
                        (ctx.load_texture(name, image, TextureOptions::LINEAR), delay)
                    })
                    .collect();
                Entry::Ready(Playing {
                    frames,
                    total: total.max(Duration::from_millis(50)),
                    started: Instant::now(),
                    last_drawn: Instant::now(),
                })
            }
            _ => Entry::Failed,
        };
        entries.insert(arrived_path, entry);
    }
    // Forget what nobody looks at any more, and beyond the budget, what
    // was looked at longest ago.
    let now = Instant::now();
    entries.retain(|_, entry| match entry {
        Entry::Ready(playing) => now.duration_since(playing.last_drawn) < IDLE,
        _ => true,
    });
    let mut resident: usize = entries
        .values()
        .map(|entry| match entry {
            Entry::Ready(playing) => playing.frames.len(),
            _ => 0,
        })
        .sum();
    while resident > MAX_RESIDENT_FRAMES {
        let victim = entries
            .iter()
            .filter_map(|(entry_path, entry)| match entry {
                Entry::Ready(playing) if entry_path.as_path() != path => {
                    Some((entry_path.clone(), playing.last_drawn, playing.frames.len()))
                }
                _ => None,
            })
            .min_by_key(|(_, last_drawn, _)| *last_drawn);
        let Some((victim, _, count)) = victim else {
            break;
        };
        entries.remove(&victim);
        resident -= count;
    }
    match entries.get_mut(path) {
        Some(Entry::Ready(playing)) => {
            playing.last_drawn = now;
            let elapsed = now.duration_since(playing.started);
            let mut position =
                Duration::from_nanos((elapsed.as_nanos() % playing.total.as_nanos()) as u64);
            let mut chosen = 0;
            let mut until_next = Duration::from_millis(40);
            for (index, (_, delay)) in playing.frames.iter().enumerate() {
                if position < *delay {
                    chosen = index;
                    until_next = *delay - position;
                    break;
                }
                position -= *delay;
            }
            ctx.request_repaint_after(until_next.max(Duration::from_millis(10)));
            Frame::Ready(playing.frames[chosen].0.clone())
        }
        Some(Entry::Decoding) => Frame::Pending,
        Some(Entry::Failed) => Frame::Unavailable,
        None => {
            if DECODING.load(std::sync::atomic::Ordering::Acquire) >= MAX_DECODERS {
                // Every decoder is busy; ask again shortly.
                ctx.request_repaint_after(Duration::from_millis(150));
                return Frame::Pending;
            }
            DECODING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let slot = DecodeSlot;
            entries.insert(path.to_path_buf(), Entry::Decoding);
            let file = path.to_path_buf();
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("animation-decode".into())
                .spawn(move || {
                    let _slot = slot;
                    // A decoder that panics still answers, with nothing.
                    let decoded =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode(&file)))
                            .unwrap_or(None);
                    inbox
                        .0
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push((file, decoded));
                    ctx.request_repaint();
                });
            if spawned.is_err() {
                entries.insert(path.to_path_buf(), Entry::Failed);
                return Frame::Unavailable;
            }
            Frame::Pending
        }
    }
}

/// Whether MP4 playback is possible on this desktop: always, since H.264
/// decodes in-process; kept for the interface.
pub fn can_play_video() -> bool {
    true
}

/// Whether ffmpeg is around for anything H.264 cannot cover.
fn ffmpeg_present() -> bool {
    static KNOWN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *KNOWN.get_or_init(|| {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

fn decode(path: &Path) -> Option<Decoded> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "webp" | "gif" => decode_image(path, &extension),
        _ => decode_video(path),
    }
}

/// Animated GIF through the `image` crate; WebP goes to libwebp.
fn decode_image(path: &Path, extension: &str) -> Option<Decoded> {
    use image::AnimationDecoder;
    if extension != "gif" {
        return decode_webp(path);
    }
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let frames = image::codecs::gif::GifDecoder::new(reader)
        .ok()?
        .into_frames();
    let mut decoded = Vec::new();
    for frame in frames.take(MAX_FRAMES) {
        let frame = frame.ok()?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay = Duration::from_millis(u64::from(numerator / denominator.max(1)).max(20));
        let image = frame.into_buffer();
        decoded.push((to_color_image(&image), delay));
    }
    Some(Decoded { frames: decoded })
}

/// Animated WebP through libwebp itself. The `image` crate's WebP
/// decoder composites frames without disposing the ones before, so a
/// moving subject left its earlier selves behind; libwebp hands back
/// each full canvas. Its timestamps mark where a frame ends.
fn decode_webp(path: &Path) -> Option<Decoded> {
    let bytes = std::fs::read(path).ok()?;
    let decoder = webp_animation::Decoder::new(&bytes).ok()?;
    let (width, height) = decoder.dimensions();
    let mut decoded = Vec::new();
    let mut previous = 0i64;
    for frame in decoder.into_iter().take(MAX_FRAMES) {
        let image = image::RgbaImage::from_raw(width, height, frame.data().to_vec())?;
        let delay = (i64::from(frame.timestamp()) - previous).max(20) as u64;
        previous = i64::from(frame.timestamp());
        decoded.push((to_color_image(&image), Duration::from_millis(delay)));
    }
    // A single frame is a still; the plain picture path draws it.
    (decoded.len() > 1).then_some(Decoded { frames: decoded })
}

fn to_color_image(image: &image::RgbaImage) -> ColorImage {
    let image = if image.width() > MAX_WIDTH {
        let height = (image.height() * MAX_WIDTH / image.width()).max(1);
        image::imageops::resize(
            image,
            MAX_WIDTH,
            height,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image.clone()
    };
    ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    )
}

/// MP4 through the `ffmpeg` command: raw RGBA frames at a modest rate and
/// width, read off its standard output.
fn decode_video(path: &Path) -> Option<Decoded> {
    // WhatsApp's GIFs are H.264 in MP4, which decodes here without any
    // program installed; anything else goes to ffmpeg when there is one.
    decode_mp4(path).or_else(|| decode_with_ffmpeg(path))
}

/// The video track of an MP4, decoded in-process.
fn decode_mp4(path: &Path) -> Option<Decoded> {
    let file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let mut mp4 = mp4::Mp4Reader::read_header(std::io::BufReader::new(file), size).ok()?;
    let (track_id, timescale, sps, pps, count) = {
        let track = mp4
            .tracks()
            .values()
            .find(|track| track.track_type().ok() == Some(mp4::TrackType::Video))?;
        (
            track.track_id(),
            u64::from(track.timescale().max(1)),
            track.sequence_parameter_set().ok()?.to_vec(),
            track.picture_parameter_set().ok()?.to_vec(),
            track.sample_count(),
        )
    };
    let mut decoder = openh264::decoder::Decoder::new().ok()?;
    let mut frames: Vec<(ColorImage, Duration)> = Vec::new();
    let mut delays: std::collections::VecDeque<Duration> = std::collections::VecDeque::new();
    // The parameter sets first, then each sample, all as Annex B.
    let mut parameters = Vec::new();
    push_annex_b(&mut parameters, &sps);
    push_annex_b(&mut parameters, &pps);
    let _ = decoder.decode(&parameters);
    for sample_id in 1..=count {
        if frames.len() >= MAX_FRAMES {
            break;
        }
        let Ok(Some(sample)) = mp4.read_sample(track_id, sample_id) else {
            break;
        };
        let delay =
            Duration::from_millis((u64::from(sample.duration) * 1000 / timescale).clamp(20, 1000));
        delays.push_back(delay);
        let mut annex_b = Vec::with_capacity(sample.bytes.len() + 16);
        avcc_to_annex_b(&mut annex_b, &sample.bytes);
        if let Ok(Some(yuv)) = decoder.decode(&annex_b) {
            let delay = delays.pop_front().unwrap_or(delay);
            if let Some(frame) = frame_of(&yuv, delay) {
                frames.push(frame);
            }
        }
    }
    if let Ok(rest) = decoder.flush_remaining() {
        for yuv in &rest {
            if frames.len() >= MAX_FRAMES {
                break;
            }
            let delay = delays.pop_front().unwrap_or(Duration::from_millis(66));
            if let Some(frame) = frame_of(yuv, delay) {
                frames.push(frame);
            }
        }
    }
    (!frames.is_empty()).then_some(Decoded { frames })
}

/// One decoded picture as a frame, scaled down to the playback width.
fn frame_of(
    yuv: &openh264::decoder::DecodedYUV<'_>,
    delay: Duration,
) -> Option<(ColorImage, Duration)> {
    use openh264::formats::YUVSource;

    let (width, height) = yuv.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let mut rgba = vec![0u8; width * height * 4];
    yuv.write_rgba8(&mut rgba);
    let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba)?;
    let out_width = (width as u32).min(MAX_WIDTH);
    let out_height = ((height as u64 * out_width as u64 / width as u64) as u32).max(1);
    let scaled = if out_width == width as u32 {
        image
    } else {
        image::imageops::resize(
            &image,
            out_width,
            out_height,
            image::imageops::FilterType::Triangle,
        )
    };
    Some((to_color_image(&scaled), delay))
}

fn push_annex_b(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
}

/// MP4 samples carry NAL units behind four-byte lengths; the decoder wants
/// them behind start codes.
fn avcc_to_annex_b(out: &mut Vec<u8>, sample: &[u8]) {
    let mut rest = sample;
    while rest.len() >= 4 {
        let length = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        rest = &rest[4..];
        if length == 0 || length > rest.len() {
            break;
        }
        push_annex_b(out, &rest[..length]);
        rest = &rest[length..];
    }
}

fn decode_with_ffmpeg(path: &Path) -> Option<Decoded> {
    if !ffmpeg_present() {
        return None;
    }
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .ok()?;
    let dimensions = String::from_utf8_lossy(&probe.stdout);
    let mut parts = dimensions.trim().split(',');
    let width: u32 = parts.next()?.trim().parse().ok()?;
    let height: u32 = parts.next()?.trim().parse().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let out_width = width.min(MAX_WIDTH);
    // Even dimensions keep every encoder happy; the height follows the
    // aspect ratio.
    let out_height = ((height as u64 * out_width as u64 / width as u64) as u32).max(2) & !1;
    let fps = 15u32;
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-an",
            "-vf",
            &format!("fps={fps},scale={out_width}:{out_height}"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let frame_bytes = (out_width * out_height * 4) as usize;
    let mut frames = Vec::new();
    let delay = Duration::from_millis(1000 / u64::from(fps));
    let mut buffer = vec![0u8; frame_bytes];
    while frames.len() < MAX_FRAMES {
        if stdout.read_exact(&mut buffer).is_err() {
            break;
        }
        frames.push((
            ColorImage::from_rgba_unmultiplied([out_width as usize, out_height as usize], &buffer),
            delay,
        ));
    }
    let _ = child.kill();
    let _ = child.wait();
    (!frames.is_empty()).then_some(Decoded { frames })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug the picker showed: a moving subject left its earlier
    /// selves behind, because the image crate's WebP decoder blends
    /// frames without disposing the ones before.
    #[test]
    fn a_moving_subject_leaves_no_trace_behind() {
        use webp_animation::prelude::*;
        let side = 64u32;
        let square = |x0: u32, y0: u32, color: [u8; 4]| {
            let mut frame = vec![0u8; (side * side * 4) as usize];
            for y in y0..y0 + 16 {
                for x in x0..x0 + 16 {
                    let at = ((y * side + x) * 4) as usize;
                    frame[at..at + 4].copy_from_slice(&color);
                }
            }
            frame
        };
        let mut encoder = Encoder::new((side, side)).expect("encoder");
        encoder
            .add_frame(&square(0, 0, [255, 0, 0, 255]), 0)
            .expect("frame");
        encoder
            .add_frame(&square(40, 40, [0, 255, 0, 255]), 100)
            .expect("frame");
        let webp = encoder.finalize(200).expect("finalizes");
        let dir = std::env::temp_dir().join(format!("fastsapp-ghost-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("moving.webp");
        std::fs::write(&path, &webp).expect("writes");
        let decoded = decode(&path).expect("decodes");
        assert_eq!(decoded.frames.len(), 2);
        let second = &decoded.frames[1].0;
        let old = second.pixels[8 * second.width() + 8];
        assert_eq!(old.a(), 0, "the first frame's square is gone: {old:?}");
        let new = second.pixels[48 * second.width() + 48];
        assert!(new.a() > 200, "the second frame's square shows: {new:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn animated_webp_decodes_into_frames() {
        // Two frames, 100 ms apart, built with the image crate itself.
        let dir = std::env::temp_dir().join(format!("fastsapp-anim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("two.gif");
        {
            let file = std::fs::File::create(&path).expect("file");
            let mut encoder = image::codecs::gif::GifEncoder::new(file);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .expect("repeat");
            for shade in [40u8, 200u8] {
                let frame = image::Frame::from_parts(
                    image::RgbaImage::from_pixel(8, 8, image::Rgba([shade, shade, shade, 255])),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(100, 1),
                );
                encoder.encode_frame(frame).expect("frame");
            }
        }
        let decoded = decode(&path).expect("decodes");
        assert_eq!(decoded.frames.len(), 2);
        assert_eq!(decoded.frames[0].1, Duration::from_millis(100));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_mp4_made_by_ffmpeg_decodes_in_process() {
        if !can_play_video() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("fastsapp-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("clip.mp4");
        let made = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=0.5:size=64x48:rate=10",
            ])
            .args(["-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .is_ok_and(|status| status.success());
        if !made {
            // No encoder on this ffmpeg; nothing to check.
            return;
        }
        let decoded = decode(&path).expect("decodes");
        // Half a second, resampled to 15 frames a second.
        // Five samples at ten a second: the in-process path keeps each one
        // and its own timing (ffmpeg would resample to fifteen a second).
        assert_eq!(decoded.frames.len(), 5);
        assert_eq!(decoded.frames[0].1, Duration::from_millis(100));
        assert_eq!(decoded.frames[0].0.size, [64, 48]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_still_webp_is_not_an_animation() {
        let dir = std::env::temp_dir().join(format!("fastsapp-still-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("still.webp");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .expect("saves");
        assert!(decode(&path).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod probe {
    use super::*;

    /// Decodes the file named by `FASTSAPP_MP4_PROBE` and reports on it:
    /// `FASTSAPP_MP4_PROBE=some.mp4 cargo test --all-features probe -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a file to look at"]
    fn decodes_the_file_named_by_the_environment() {
        let Some(path) = std::env::var_os("FASTSAPP_MP4_PROBE") else {
            return;
        };
        let started = Instant::now();
        let decoded = decode_mp4(Path::new(&path)).expect("decodes in-process");
        eprintln!(
            "{} frames of {:?}, first delay {:?}, in {:?}",
            decoded.frames.len(),
            decoded.frames[0].0.size,
            decoded.frames[0].1,
            started.elapsed()
        );
        assert!(!decoded.frames.is_empty());
    }
}
