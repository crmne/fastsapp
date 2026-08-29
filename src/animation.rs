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

/// Whether MP4 playback is possible on this desktop.
pub fn can_play_video() -> bool {
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

/// Animated WebP and GIF, through the `image` crate.
fn decode_image(path: &Path, extension: &str) -> Option<Decoded> {
    use image::AnimationDecoder;
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let frames = match extension {
        "gif" => image::codecs::gif::GifDecoder::new(reader)
            .ok()?
            .into_frames(),
        _ => {
            let decoder = image::codecs::webp::WebPDecoder::new(reader).ok()?;
            if !decoder.has_animation() {
                return None;
            }
            decoder.into_frames()
        }
    };
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
    if !can_play_video() {
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
    fn an_mp4_decodes_through_ffmpeg_when_present() {
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
        assert!(
            (5..=9).contains(&decoded.frames.len()),
            "{} frames",
            decoded.frames.len()
        );
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
