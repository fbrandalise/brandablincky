use std::sync::Once;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::codecs::jpeg::JpegEncoder;

use super::backend::ScreenshotBackend;

/// Logs the monitor geometry diagnostic at most once per process.
static GEOMETRY_DIAG: Once = Once::new();

/// Cross-platform backend (Windows, macOS, Linux). On Linux, tries xcap first
/// (Wayland via wayshot, or X11 via xcb), falling back to a dedicated X11
/// path when xcap's Wayland detection triggers but the compositor doesn't
/// support the wayshot protocol.
/// Zero-sized; never instantiated.
pub struct Backend;

impl ScreenshotBackend for Backend {
    /// Geometry of the primary monitor.
    fn active_workspace_geometry()
    -> Result<(i32, i32, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
        if let Ok(geo) = xcap_geometry() {
            return Ok(geo);
        }
        #[cfg(target_os = "linux")]
        if let Ok(geo) = super::x11::Backend::active_workspace_geometry() {
            return Ok(geo);
        }
        #[cfg(target_os = "linux")]
        return super::portal::Backend::active_workspace_geometry();
        #[cfg(not(target_os = "linux"))]
        Err("no monitors found".into())
    }

    fn capture_resized_for_claude(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        target_w: u32,
        target_h: u32,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if let Ok(result) = xcap_capture(x, y, width, height, target_w, target_h) {
            return Ok(result);
        }
        #[cfg(target_os = "linux")]
        if let Ok(result) = super::x11::Backend::capture_resized_for_claude(
            x, y, width, height, target_w, target_h,
        ) {
            return Ok(result);
        }
        #[cfg(target_os = "linux")]
        return super::portal::Backend::capture_resized_for_claude(
            x, y, width, height, target_w, target_h,
        );
        #[cfg(not(target_os = "linux"))]
        Err("screenshot capture failed".into())
    }
}

// ── xcap path (Wayland or X11 via xcb) ─────────────────────────────────────

fn xcap_geometry() -> Result<(i32, i32, u32, u32), String> {
    let monitors = ::xcap::Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or("no primary monitor".to_string())?;
    let geo = (
        monitor.x().map_err(|e| e.to_string())?,
        monitor.y().map_err(|e| e.to_string())?,
        monitor.width().map_err(|e| e.to_string())?,
        monitor.height().map_err(|e| e.to_string())?,
    );

    // DIAGNOSTIC (once per process): find_action maps Claude's coordinates
    // into these w/h units.
    GEOMETRY_DIAG.call_once(|| {
        let sf = monitor.scale_factor().unwrap_or(1.0);
        let (_, _, w, h) = geo;
        eprintln!(
            "[diag:geometry] xcap primary monitor: pos=({}, {}) size={}x{} \
             scale_factor={:.2} → physical would be {}x{}. \
             Clicks use logical points; size above must be logical.",
            geo.0,
            geo.1,
            w,
            h,
            sf,
            (w as f32 * sf) as u32,
            (h as f32 * sf) as u32,
        );
    });

    Ok(geo)
}

fn xcap_capture(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    target_w: u32,
    target_h: u32,
) -> Result<String, String> {
    use fast_image_resize::images::Image as FirImage;
    use fast_image_resize::{
        FilterType as FirFilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    };

    let monitor = ::xcap::Monitor::from_point(x, y).map_err(|e| e.to_string())?;
    let local_x = (x - monitor.x().map_err(|e| e.to_string())?).max(0) as u32;
    let local_y = (y - monitor.y().map_err(|e| e.to_string())?).max(0) as u32;
    let image = monitor
        .capture_region(local_x, local_y, width as u32, height as u32)
        .map_err(|e| e.to_string())?;

    let src_w = image.width();
    let src_h = image.height();

    let rgba = image.into_raw();
    let mut rgb: Vec<u8> = Vec::with_capacity((src_w * src_h * 3) as usize);
    for chunk in rgba.chunks_exact(4) {
        rgb.push(chunk[0]);
        rgb.push(chunk[1]);
        rgb.push(chunk[2]);
    }

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

impl Backend {
    /// Capture a region, encode as JPEG q85, return base64 plus captured
    /// dimensions. Not part of the `ScreenshotBackend` contract: only the
    /// `demo_win` binary calls it, and grim has no equivalent. Kept here so
    /// the demo and the main pipeline share one screenshot module.
    #[allow(dead_code)]
    pub fn capture_for_claude(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<(String, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
        let monitor = ::xcap::Monitor::from_point(x, y)?;
        // capture_region takes monitor-local coords, so subtract monitor origin.
        let local_x = (x - monitor.x()?).max(0) as u32;
        let local_y = (y - monitor.y()?).max(0) as u32;
        let image = monitor.capture_region(local_x, local_y, width as u32, height as u32)?;

        let mut jpeg: Vec<u8> = Vec::new();
        let encoder = JpegEncoder::new_with_quality(&mut jpeg, 85);
        image.write_with_encoder(encoder)?;
        Ok((BASE64.encode(&jpeg), width as u32, height as u32))
    }
}
