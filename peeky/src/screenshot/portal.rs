//! GNOME/Mutter Wayland screenshot fallback via xdg-desktop-portal's
//! ScreenCast interface over PipeWire. Reached when both xcap's Wayland path
//! (`wayshot`, unsupported by Mutter) and raw X11 `XGetImage` (the root
//! window isn't backed by real desktop content under a native Wayland
//! session) have already failed. Same mechanism Firefox/OBS use.
//!
//! The portal session and PipeWire stream are negotiated once per process
//! (`CAPTURE`, below) and kept alive on a dedicated thread for the process
//! lifetime — re-negotiating (and re-prompting the user) on every voice turn
//! would be unusable. A restore token persisted to
//! `~/.config/peeky/portal_restore_token` (see
//! `providers::portal_restore_token`) lets every run after the first skip the
//! "Share your screen" picker. A failed negotiation is cached as an error for
//! the rest of the process lifetime; restart peeky to retry.

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::video::VideoFormat;

use crate::providers::portal_restore_token;
use crate::tuning::{PORTAL_FRAME_MAX_AGE_MS, PORTAL_STREAM_READY_TIMEOUT_MS};

use super::backend::ScreenshotBackend;

/// Portal backend. Zero-sized; never instantiated. See module docs.
pub struct Backend;

/// One decoded RGB frame from the PipeWire stream.
struct Frame {
    rgb: Vec<u8>,
    width: u32,
    height: u32,
    captured_at: Instant,
}

/// Live portal session: the PipeWire thread writes into `latest`; callers
/// poll it. `geometry` is fixed once at negotiation time.
struct PortalCapture {
    latest: Arc<Mutex<Option<Frame>>>,
    geometry: (i32, i32, u32, u32),
}

/// `Some(negotiated_size)` once the PipeWire stream reports its first video
/// format; used both as the "stream is live" signal and, when the portal
/// itself didn't report a monitor size, as the geometry fallback.
type ReadyState = Arc<(Mutex<Option<(u32, u32)>>, Condvar)>;

static CAPTURE: OnceLock<Result<PortalCapture, String>> = OnceLock::new();

impl ScreenshotBackend for Backend {
    fn active_workspace_geometry()
    -> Result<(i32, i32, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
        CAPTURE
            .get_or_init(negotiate)
            .as_ref()
            .map(|c| c.geometry)
            .map_err(|e| e.clone().into())
    }

    /// `x`/`y`/`width`/`height` are ignored: the portal's monitor-scoped
    /// stream is already cropped to a single monitor by the compositor, so
    /// the virtual-desktop-space region other backends crop to doesn't apply
    /// here. The full negotiated frame is always what gets resized.
    fn capture_resized_for_claude(
        _x: i32,
        _y: i32,
        _width: i32,
        _height: i32,
        target_w: u32,
        target_h: u32,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let capture = CAPTURE
            .get_or_init(negotiate)
            .as_ref()
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.clone().into() })?;

        let deadline = Instant::now() + Duration::from_millis(PORTAL_STREAM_READY_TIMEOUT_MS);
        let (rgb, fw, fh) = loop {
            {
                let guard = capture
                    .latest
                    .lock()
                    .map_err(|_| "portal: frame lock poisoned".to_string())?;
                if let Some(frame) = guard.as_ref() {
                    if frame.captured_at.elapsed() <= Duration::from_millis(PORTAL_FRAME_MAX_AGE_MS)
                    {
                        break (frame.rgb.clone(), frame.width, frame.height);
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err("portal: timed out waiting for a fresh PipeWire frame".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        };

        resize_and_encode(rgb, fw, fh, target_w, target_h).map_err(|e| e.into())
    }
}

/// Negotiates a portal ScreenCast session, opens the PipeWire remote, and
/// spawns the background thread that feeds `latest`. Runs once per process
/// (guarded by `CAPTURE`).
fn negotiate() -> Result<PortalCapture, String> {
    // `capture_resized_for_claude` can run on an existing tokio worker thread
    // (the agent loop's `take_screenshot` calls it inline, not via
    // spawn_blocking), where building a second runtime and calling
    // `.block_on` on it panics ("Cannot start a runtime from within a
    // runtime"). `block_in_place` + the current `Handle` sidesteps that; the
    // dedicated runtime is only needed when negotiate() runs with no tokio
    // runtime already active.
    let (node_id, portal_geometry, fd) = match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(open_portal()))?,
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("portal: failed to start async runtime: {e}"))?;
            rt.block_on(open_portal())?
        }
    };

    let latest: Arc<Mutex<Option<Frame>>> = Arc::new(Mutex::new(None));
    let ready_state: ReadyState = Arc::new((Mutex::new(None), Condvar::new()));

    spawn_pipewire_thread(node_id, fd, latest.clone(), ready_state.clone())
        .map_err(|e| format!("portal: failed to start PipeWire thread: {e}"))?;

    let (lock, cvar) = &*ready_state;
    let guard = lock
        .lock()
        .map_err(|_| "portal: ready-lock poisoned".to_string())?;
    let (guard, timeout) = cvar
        .wait_timeout_while(
            guard,
            Duration::from_millis(PORTAL_STREAM_READY_TIMEOUT_MS),
            |size| size.is_none(),
        )
        .map_err(|_| "portal: ready-lock poisoned".to_string())?;
    if timeout.timed_out() {
        return Err("portal: PipeWire stream did not report a format in time".to_string());
    }
    // reason: wait_timeout_while only returns non-timed-out once the
    // predicate (`size.is_none()`) is false, i.e. `*guard` is `Some(..)`.
    let negotiated_size = guard.expect("ready_state is Some once the wait predicate is false");

    let (gx, gy, mut gw, mut gh) = portal_geometry;
    if gw == 0 || gh == 0 {
        (gw, gh) = negotiated_size;
    }

    Ok(PortalCapture {
        latest,
        geometry: (gx, gy, gw, gh),
    })
}

/// Portal geometry as `(x, y, width, height)`; width/height are `0` when the
/// portal didn't report a monitor size (caller falls back to the negotiated
/// PipeWire format size in that case).
type PortalGeometry = (i32, i32, u32, u32);

async fn open_portal() -> Result<(u32, PortalGeometry, std::os::fd::OwnedFd), String> {
    let proxy = Screencast::new().await.map_err(|e| e.to_string())?;
    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(|e| e.to_string())?;

    let restore_token = portal_restore_token::load();
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Hidden)
                .set_sources(ashpd::enumflags2::BitFlags::from(SourceType::Monitor))
                .set_multiple(false)
                .set_restore_token(restore_token.as_deref())
                .set_persist_mode(PersistMode::ExplicitlyRevoked),
        )
        .await
        .map_err(|e| e.to_string())?;

    let response = proxy
        .start(&session, None, Default::default())
        .await
        .map_err(|e| e.to_string())?
        .response()
        .map_err(|e| e.to_string())?;

    if let Some(token) = response.restore_token() {
        let _ = portal_restore_token::store(token);
    }

    let stream = response
        .streams()
        .first()
        .ok_or("portal returned no streams")?
        .to_owned();

    let (pos_x, pos_y) = stream.position().unwrap_or((0, 0));
    let (w, h) = stream.size().unwrap_or((0, 0));
    let node_id = stream.pipe_wire_node_id();

    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(|e| e.to_string())?;

    Ok((node_id, (pos_x, pos_y, w.max(0) as u32, h.max(0) as u32), fd))
}

/// Per-stream state handed to the PipeWire listener callbacks.
struct StreamUserData {
    format: spa::param::video::VideoInfoRaw,
    latest: Arc<Mutex<Option<Frame>>>,
    ready_state: ReadyState,
}

/// Spawns the dedicated PipeWire mainloop thread. Mirrors the `!Send`
/// cpal-stream-owning-thread pattern in `peeky/src/audio/input.rs`: PipeWire's
/// mainloop/stream types are equally `!Send`, so everything from
/// `pw::init()` to `mainloop.run()` (which blocks forever, for the process
/// lifetime) must happen on the one thread.
fn spawn_pipewire_thread(
    node_id: u32,
    fd: std::os::fd::OwnedFd,
    latest: Arc<Mutex<Option<Frame>>>,
    ready_state: ReadyState,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("peeky-portal-pw".to_string())
        .spawn(move || {
            if let Err(e) = run_pipewire_loop(node_id, fd, latest, ready_state) {
                eprintln!("[screenshot:portal] pipewire setup failed: {e}");
            }
        })
        .map(|_| ())
}

fn run_pipewire_loop(
    node_id: u32,
    fd: std::os::fd::OwnedFd,
    latest: Arc<Mutex<Option<Frame>>>,
    ready_state: ReadyState,
) -> Result<(), String> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopBox::new(None).map_err(|e| e.to_string())?;
    let context =
        pw::context::ContextBox::new(mainloop.loop_(), None).map_err(|e| e.to_string())?;
    let core = context.connect_fd(fd, None).map_err(|e| e.to_string())?;

    let user_data = StreamUserData {
        format: Default::default(),
        latest,
        ready_state,
    };

    let stream = pw::stream::StreamBox::new(
        &core,
        "peeky-screenshot",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| e.to_string())?;

    let _listener = stream
        .add_local_listener_with_user_data(user_data)
        .param_changed(|_, user_data, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = pw::spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != pw::spa::param::format::MediaType::Video
                || media_subtype != pw::spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            if user_data.format.parse(param).is_err() {
                return;
            }

            let size = user_data.format.size();
            let (lock, cvar) = &*user_data.ready_state;
            if let Ok(mut guard) = lock.lock() {
                *guard = Some((size.width, size.height));
                cvar.notify_all();
            }
        })
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let stride = datas[0].chunk().stride();
            let chunk_offset = datas[0].chunk().offset() as usize;
            let Some(raw) = datas[0].data() else {
                return;
            };
            if chunk_offset >= raw.len() {
                return;
            }
            let px_format = user_data.format.format();
            let size = user_data.format.size();
            match convert_frame_to_rgb(&raw[chunk_offset..], stride, px_format, size.width, size.height)
            {
                Ok(rgb) => {
                    if let Ok(mut guard) = user_data.latest.lock() {
                        *guard = Some(Frame {
                            rgb,
                            width: size.width,
                            height: size.height,
                            captured_at: Instant::now(),
                        });
                    }
                }
                Err(_) => {}
            }
        })
        .register()
        .map_err(|e| e.to_string())?;

    // Advertise only the raw formats `convert_frame_to_rgb` can decode, so
    // negotiation never lands on a YUV format we can't handle.
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::RGB,
            VideoFormat::RGB,
            VideoFormat::RGBA,
            VideoFormat::RGBx,
            VideoFormat::BGRx,
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 10, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: 60,
                denom: 1
            }
        ),
    );
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map_err(|e| e.to_string())?
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&values).ok_or("portal: bad format pod")?];

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;

    mainloop.run();
    Ok(())
}

/// Converts one raw video row-major buffer to tightly-packed RGB, respecting
/// `stride` (bytes per row, may exceed `width * bpp` due to alignment).
/// Mirrors `x11.rs::convert_ximage_to_rgba`'s per-format branch shape.
fn convert_frame_to_rgb(
    data: &[u8],
    stride: i32,
    format: VideoFormat,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let bpp: usize = match format {
        VideoFormat::RGB => 3,
        VideoFormat::RGBA | VideoFormat::RGBx | VideoFormat::BGRx => 4,
        other => return Err(format!("portal: unsupported pixel format {other:?}")),
    };
    let stride = stride.max(0) as usize;
    let (width, height) = (width as usize, height as usize);

    let mut rgb = Vec::with_capacity(width * height * 3);
    for row in 0..height {
        let row_start = row * stride;
        if row_start + width * bpp > data.len() {
            return Err("portal: frame buffer shorter than expected".to_string());
        }
        for col in 0..width {
            let px = row_start + col * bpp;
            match format {
                VideoFormat::RGB | VideoFormat::RGBA | VideoFormat::RGBx => {
                    rgb.push(data[px]);
                    rgb.push(data[px + 1]);
                    rgb.push(data[px + 2]);
                }
                VideoFormat::BGRx => {
                    rgb.push(data[px + 2]);
                    rgb.push(data[px + 1]);
                    rgb.push(data[px]);
                }
                _ => unreachable!("filtered by the bpp match above"),
            }
        }
    }
    Ok(rgb)
}

/// Resize to `(target_w, target_h)`, encode JPEG q85, base64. Same tail as
/// every other screenshot backend (`x11.rs`, `crossplatform.rs`, `hyprland.rs`).
fn resize_and_encode(
    rgb: Vec<u8>,
    src_w: u32,
    src_h: u32,
    target_w: u32,
    target_h: u32,
) -> Result<String, String> {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use fast_image_resize::images::Image as FirImage;
    use fast_image_resize::{
        FilterType as FirFilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    };
    use image::codecs::jpeg::JpegEncoder;

    let fir_src =
        FirImage::from_vec_u8(src_w, src_h, rgb, PixelType::U8x3).map_err(|e| e.to_string())?;
    let mut fir_dst = FirImage::new(target_w, target_h, PixelType::U8x3);
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FirFilterType::Bilinear));
    let mut resizer = Resizer::new();
    resizer
        .resize(&fir_src, &mut fir_dst, &opts)
        .map_err(|e| e.to_string())?;

    let mut out: Vec<u8> = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 85)
        .encode(
            fir_dst.buffer(),
            target_w,
            target_h,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    Ok(BASE64.encode(&out))
}
