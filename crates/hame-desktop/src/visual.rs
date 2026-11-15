//! Pixels: mapping a point a model picked in a screenshot back to the
//! screen, and recognising a capture that came back blank.

use hame_session::artifact::Capture;

/// Maps a point in a sent image of `sent` pixels to screen points through
/// the capture's crop. Refuses points outside the image instead of
/// clamping them, since a clamped click lands somewhere nobody chose.
pub fn image_to_screen(
    capture: &Capture,
    sent: (u32, u32),
    point: (f64, f64),
) -> Result<(f64, f64), String> {
    if capture.space != "screen-points" {
        return Err(format!(
            "The capture is in {} space; click_at needs a desktop screenshot in screen points.",
            capture.space
        ));
    }
    let (x, y) = point;
    if !x.is_finite() || !y.is_finite() {
        return Err("x and y must be numbers.".into());
    }
    if x < 0.0 || y < 0.0 || x >= f64::from(sent.0) || y >= f64::from(sent.1) {
        return Err(format!(
            "({x}, {y}) is outside the {}x{} screenshot; use coordinates of that image.",
            sent.0, sent.1
        ));
    }
    Ok(capture.to_space(sent, point))
}

/// The inverse of [`image_to_screen`]: where a screen point appears in the
/// sent image.
pub fn screen_to_image(capture: &Capture, sent: (u32, u32), point: (f64, f64)) -> (f64, f64) {
    let [left, top, width, height] = capture.crop;
    (
        (point.0 - left) * f64::from(sent.0) / width.max(f64::MIN_POSITIVE),
        (point.1 - top) * f64::from(sent.1) / height.max(f64::MIN_POSITIVE),
    )
}

/// How much the colours in a capture vary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uniformity {
    /// The largest per-channel difference between any two samples.
    pub spread: u8,
    /// Whether the capture is a single flat colour (or fully transparent),
    /// which is what a capture without Screen Recording permission, or of
    /// a window that has not drawn, looks like.
    pub blank: bool,
}

/// The largest channel spread still treated as one flat colour; it
/// absorbs dithering and compression noise.
pub const BLANK_SPREAD: u8 = 6;

/// Checks RGBA samples (a small downscaled grid of the image) for a flat
/// colour.
pub fn uniformity(rgba: &[u8]) -> Uniformity {
    let pixels: &[[u8; 4]] = rgba.as_chunks::<4>().0;
    if pixels.is_empty() || pixels.iter().all(|p| p[3] == 0) {
        return Uniformity {
            spread: 0,
            blank: true,
        };
    }
    let mut spread = 0u8;
    for channel in 0..3 {
        let values = pixels.iter().map(|p| p[channel]);
        let (lo, hi) = values.fold((u8::MAX, u8::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        spread = spread.max(hi - lo);
    }
    Uniformity {
        spread,
        blank: spread <= BLANK_SPREAD,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(crop: [f64; 4], scale: f64) -> Capture {
        Capture {
            target: "desktop:test".into(),
            ts: 0,
            space: "screen-points".into(),
            crop,
            scale,
            redacted: false,
            derived_from: None,
        }
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn retina_capture_at_full_resolution_maps_pixels_to_points() {
        // An 800x600-point window at (100, 50) on a scale-2.0 display is
        // captured at 1600x1200 pixels.
        let c = capture([100.0, 50.0, 800.0, 600.0], 2.0);
        let sent = (1600, 1200);
        assert!(close(
            image_to_screen(&c, sent, (800.0, 600.0)).unwrap(),
            (500.0, 350.0)
        ));
        assert!(close(
            image_to_screen(&c, sent, (0.0, 0.0)).unwrap(),
            (100.0, 50.0)
        ));
        // One physical pixel is half a point.
        assert!(close(
            image_to_screen(&c, sent, (1.0, 1.0)).unwrap(),
            (100.5, 50.5)
        ));
    }

    #[test]
    fn downscaled_capture_maps_through_the_crop_not_the_pixels() {
        // Same window, sent downscaled to max_edge 800: 800x600 pixels.
        let c = capture([100.0, 50.0, 800.0, 600.0], 2.0);
        let sent = (800, 600);
        assert!(close(
            image_to_screen(&c, sent, (400.0, 300.0)).unwrap(),
            (500.0, 350.0)
        ));
        // Downscaled to a non-integer ratio (max_edge 500 -> 500x375).
        let sent = (500, 375);
        let p = image_to_screen(&c, sent, (250.0, 187.5)).unwrap();
        assert!(close(p, (500.0, 350.0)), "{p:?}");
    }

    #[test]
    fn monitors_left_of_and_above_the_primary_have_negative_origins() {
        // A scale-1.0 monitor at (-1920, -200); a 1000x700 window on it at
        // (-1500, 100).
        let c = capture([-1500.0, 100.0, 1000.0, 700.0], 1.0);
        let p = image_to_screen(&c, (1000, 700), (10.0, 10.0)).unwrap();
        assert!(close(p, (-1490.0, 110.0)), "{p:?}");
        // A region crop that is itself above the primary display.
        let c = capture([-1800.0, -150.0, 400.0, 100.0], 1.0);
        let p = image_to_screen(&c, (400, 100), (200.0, 50.0)).unwrap();
        assert!(close(p, (-1600.0, -100.0)), "{p:?}");
        // A Retina monitor to the right, region crop downscaled by 4.
        let c = capture([3024.0 + 200.0, 300.0, 1200.0, 800.0], 2.0);
        let p = image_to_screen(&c, (600, 400), (300.0, 200.0)).unwrap();
        assert!(close(p, (3824.0, 700.0)), "{p:?}");
    }

    #[test]
    fn points_outside_the_image_are_refused_and_round_trips_hold() {
        let c = capture([100.0, 50.0, 800.0, 600.0], 2.0);
        assert!(image_to_screen(&c, (800, 600), (800.0, 10.0)).is_err());
        assert!(image_to_screen(&c, (800, 600), (-1.0, 10.0)).is_err());
        assert!(image_to_screen(&c, (800, 600), (f64::NAN, 10.0)).is_err());
        let css = Capture {
            space: "css-pixels".into(),
            ..c.clone()
        };
        assert!(image_to_screen(&css, (800, 600), (1.0, 1.0)).is_err());
        for point in [(0.0, 0.0), (123.25, 456.5), (799.0, 599.0)] {
            let screen = image_to_screen(&c, (800, 600), point).unwrap();
            assert!(close(screen_to_image(&c, (800, 600), screen), point));
        }
    }

    fn grid(f: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..24 {
            for x in 0..24 {
                out.extend_from_slice(&f(x, y));
            }
        }
        out
    }

    #[test]
    fn flat_and_transparent_captures_are_blank() {
        assert!(uniformity(&grid(|_, _| [0, 0, 0, 255])).blank);
        assert!(uniformity(&grid(|_, _| [0, 0, 0, 0])).blank);
        assert!(uniformity(&[]).blank);
        // Noise of a few levels is still one colour.
        let noisy = grid(|x, y| {
            let n = ((x * 7 + y * 3) % 5) as u8;
            [200 + n, 200 + n, 201, 255]
        });
        let u = uniformity(&noisy);
        assert!(u.blank && u.spread == 4, "{u:?}");
    }

    #[test]
    fn a_window_with_a_title_bar_or_text_is_not_blank() {
        // White document with a grey title bar across the top two rows.
        let window = grid(|_, y| {
            if y < 2 {
                [220, 220, 220, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let u = uniformity(&window);
        assert!(!u.blank && u.spread == 35, "{u:?}");
        // One dark sample (a line of text) is enough.
        let text = grid(|x, y| {
            if (x, y) == (5, 12) {
                [40, 40, 40, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        assert!(!uniformity(&text).blank);
    }
}
