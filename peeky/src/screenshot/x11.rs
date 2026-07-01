//! X11 screenshot capture via the `x11` crate (Xlib + Xrandr). Used as a
//! fallback on Linux when `xcap`'s Wayland detection triggers (WAYLAND_DISPLAY
//! set) but the compositor doesn't support the wayshot protocol.
//!
//! Captures the root window via `XGetImage` and queries monitor layout via
//! `XRRGetMonitors`.

use super::backend::ScreenshotBackend;

/// X11 backend. Zero-sized; never instantiated.
pub struct Backend;

impl ScreenshotBackend for Backend {
    fn active_workspace_geometry()
    -> Result<(i32, i32, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
        let display = open_display()?;
        let screen = 0;
        let monitors = list_monitors(display, screen)?;
        close_display(display);

        if monitors.is_empty() {
            // Fallback: use display dimensions.
            let width = unsafe { x11::xlib::XDisplayWidth(display, screen) };
            let height = unsafe { x11::xlib::XDisplayHeight(display, screen) };
            return Ok((0, 0, width as u32, height as u32));
        }

        let m = &monitors[0];
        Ok((m.x, m.y, m.width, m.height))
    }

    fn capture_resized_for_claude(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        target_w: u32,
        target_h: u32,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
        use fast_image_resize::{
            FilterType as FirFilterType, Image as FirImage, PixelType, ResizeAlg, ResizeOptions,
            Resizer,
        };
        use image::codecs::jpeg::JpegEncoder;

        let display = open_display()?;
        let root = unsafe { x11::xlib::XDefaultRootWindow(display) };
        let screen = 0;

        let screen_ptr = unsafe { x11::xlib::XDefaultScreenOfDisplay(display) };
        let root_depth = unsafe { (*screen_ptr).root_depth };

        let full_width = unsafe { x11::xlib::XDisplayWidth(display, screen) };
        let full_height = unsafe { x11::xlib::XDisplayHeight(display, screen) };

        // Capture full root window.
        let ximage = xget_image(display, root, 0, 0, full_width as u32, full_height as u32)?;
        let img_w = ximage.width;
        let img_h = ximage.height;
        let data_ptr = ximage.data as *const u8;
        let bpp = ximage.bits_per_pixel as u32;

        let mut raw_data = vec![0u8; (img_w * img_h * 4) as usize];
        unsafe {
            let src = std::slice::from_raw_parts(data_ptr, (img_w * img_h * (bpp / 8)) as usize);
            convert_ximage_to_rgba(src, bpp, &mut raw_data, img_w, img_h);
        }

        // Crop to requested region (relative to monitor origin).
        let crop_x = (x - ximage.x).max(0).min(img_w);
        let crop_y = (y - ximage.y).max(0).min(img_h);
        let region_w = width.max(1).min(img_w - crop_x as i32) as u32;
        let region_h = height.max(1).min(img_h - crop_y as i32) as u32;

        // Extract region RGB.
        let mut region_rgb = Vec::with_capacity(region_w as usize * region_h as usize * 3);
        for py in crop_y..crop_y + region_h {
            let row_start = ((py * img_w + crop_x) * 4) as usize;
            for px in 0..region_w {
                let offset = (px * 4) as usize;
                region_rgb.push(raw_data[row_start + offset]);
                region_rgb.push(raw_data[row_start + offset + 1]);
                region_rgb.push(raw_data[row_start + offset + 2]);
            }
        }

        // Free ximage and close display.
        unsafe { x11::xlib::XDestroyImage(ximage) };
        close_display(display);

        // Resize and encode.
        let fir_src = FirImage::from_vec_u8(region_w, region_h, region_rgb, PixelType::U8x3)
            .map_err(|e| format!("fast_image_resize: {e}"))?;
        let mut fir_dst = FirImage::new(target_w, target_h, PixelType::U8x3);
        let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FirFilterType::Bilinear));
        let mut resizer = Resizer::new();
        resizer
            .resize(&fir_src, &mut fir_dst, &opts)
            .map_err(|e| e.to_string())?;

        let mut out = Vec::new();
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
}

// ── x11 crate helpers ──────────────────────────────────────────────────────

type XDisplay = x11::xlib::Display;

/// Open the X display. Returns a raw pointer.
fn open_display() -> Result<*mut XDisplay, String> {
    let display = unsafe { x11::xlib::XOpenDisplay(std::ptr::null()) };
    if display.is_null() {
        return Err("XOpenDisplay failed: cannot connect to X server".to_string());
    }
    Ok(display)
}

/// Close the X display.
unsafe fn close_display(display: *mut XDisplay) {
    x11::xlib::XCloseDisplay(display);
}

/// XGetImage wrapper.
fn xget_image(
    display: *mut XDisplay,
    window: x11::xlib::Window,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<*mut x11::xlib::XImage, String> {
    let image = unsafe {
        x11::xlib::XGetImage(
            display,
            window,
            x,
            y,
            width,
            height,
            !0u32,
            x11::xlib::ZPixmap,
        )
    };
    if image.is_null() {
        return Err("XGetImage failed: cannot capture root window".to_string());
    }
    Ok(image)
}

/// Convert XImage pixel data to RGBA buffer.
fn convert_ximage_to_rgba(src: &[u8], bpp: u32, dst: &mut [u8], width: u32, height: u32) {
    let total_pixels = width as usize * height as usize;
    match bpp {
        32 => {
            for i in 0..total_pixels {
                let si = i * 4;
                let di = i * 4;
                dst[di] = src[si + 2];
                dst[di + 1] = src[si + 1];
                dst[di + 2] = src[si];
                dst[di + 3] = src[si + 3];
            }
        }
        24 => {
            for i in 0..total_pixels {
                let si = i * 3;
                let di = i * 4;
                dst[di] = src[si + 2];
                dst[di + 1] = src[si + 1];
                dst[di + 2] = src[si];
                dst[di + 3] = 255;
            }
        }
        16 => {
            for i in 0..total_pixels {
                let si = i * 2;
                let di = i * 4;
                let pixel = u16::from_le_bytes([src[si], src[si + 1]]);
                let r = ((pixel >> 10) & 0x1f) as u32;
                let g = ((pixel >> 5) & 0x3f) as u32;
                let b = (pixel & 0x1f) as u32;
                dst[di] = (r * 255 / 31) as u8;
                dst[di + 1] = (g * 255 / 63) as u8;
                dst[di + 2] = (b * 255 / 31) as u8;
                dst[di + 3] = 255;
            }
        }
        _ => {
            let bpp_bytes = (bpp + 7) / 8;
            for i in 0..total_pixels {
                let si = i * bpp_bytes as usize;
                let di = i * 4;
                if si + 2 < src.len() {
                    dst[di] = src[si + 2];
                    dst[di + 1] = src[si + 1];
                    dst[di + 2] = src[si];
                }
                dst[di + 3] = 255;
            }
        }
    }
}

// ── Xrandr monitor enumeration ─────────────────────────────────────────────

struct Monitor {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    primary: bool,
}

/// Query Xrandr for monitor layout.
fn list_monitors(display: *mut XDisplay, screen: i32) -> Result<Vec<Monitor>, String> {
    // SAFETY: display is a valid Xlib Display pointer.
    let monitors_ptr = unsafe { x11::xrandr::XRRGetMonitors(display, screen, 1) };
    if monitors_ptr.is_null() {
        return Ok(Vec::new());
    }

    // SAFETY: XRRGetMonitors returns a pointer we own; XRRFreeMonitors frees it.
    let monitors = unsafe { *monitors_ptr };

    let mut result = Vec::with_capacity(monitors.nmonitors as usize);
    for i in 0..monitors.nmonitors {
        let mon = unsafe { *monitors.monitors.add(i as usize) };
        result.push(Monitor {
            x: mon.x,
            y: mon.y,
            width: mon.width,
            height: mon.height,
            primary: mon.primary,
        });
    }

    // Free the monitors array.
    unsafe { x11::xrandr::XRRFreeMonitors(monitors_ptr) };

    // Primary first, then by position.
    result.sort_by(|a, b| {
        b.primary
            .cmp(&a.primary)
            .then_with(|| a.y.cmp(&b.y).then_with(|| a.x.cmp(&b.x)))
    });

    Ok(result)
}
