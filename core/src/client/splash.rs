//! What the streaming window shows until the first video frame: a dark
//! background, a spinner and what the session is doing, so the window
//! never sits there blank (Windows paints a new window white) looking
//! as if it had hung.
//!
//! Drawn on the CPU into the softbuffer surface; text uses a built-in
//! 5x7 pixel font (capitals, `.` and `+`).

const BACKGROUND: u32 = 0x0007_090D;
const TEXT: u32 = 0x00E6_E9EE;
const DIM: u32 = 0x007A_828E;
const ACCENT: (u32, u32, u32) = (60, 203, 180);

/// Rows of a glyph, 5 bits each (bit 4 = leftmost column).
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        _ => [0; 7],
    }
}

struct Canvas<'a> {
    buf: &'a mut [u32],
    w: usize,
    h: usize,
}

impl Canvas<'_> {
    fn rect(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) {
        let (x0, y0) = ((x.max(0) as usize).min(self.w), y.max(0) as usize);
        let (x1, y1) = (((x + w).max(0) as usize).min(self.w), ((y + h).max(0) as usize).min(self.h));
        for row in y0..y1 {
            self.buf[row * self.w + x0..row * self.w + x1.max(x0)].fill(color);
        }
    }

    fn disc(&mut self, cx: f32, cy: f32, r: f32, color: u32) {
        let (x0, x1) = ((cx - r).floor().max(0.0) as usize, ((cx + r).ceil() as usize).min(self.w));
        let (y0, y1) = ((cy - r).floor().max(0.0) as usize, ((cy + r).ceil() as usize).min(self.h));
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                if dx * dx + dy * dy <= r * r {
                    self.buf[y * self.w + x] = color;
                }
            }
        }
    }

    /// Text centred on `cy`, `scale` screen pixels per font pixel.
    fn text(&mut self, s: &str, cy: i64, scale: i64, color: u32) {
        let advance = 6 * scale;
        let width = s.chars().count() as i64 * advance - scale;
        let mut x = (self.w as i64 - width) / 2;
        let y = cy - 7 * scale / 2;
        for c in s.chars() {
            for (row, bits) in glyph(c).iter().enumerate() {
                for col in 0..5 {
                    if bits & (0x10 >> col) != 0 {
                        self.rect(x + col * scale, y + row as i64 * scale, scale, scale, color);
                    }
                }
            }
            x += advance;
        }
    }
}

/// `t` (seconds) drives the animation.
fn blend(t: f32) -> u32 {
    let (r, g, b) = ACCENT;
    let bg = |shift: u32| ((BACKGROUND >> shift) & 0xFF) as f32;
    let mix = |c: u32, shift: u32| (bg(shift) + (c as f32 - bg(shift)) * t) as u32;
    (mix(r, 16) << 16) | (mix(g, 8) << 8) | mix(b, 0)
}

/// Draw the connecting screen into a `w`x`h` 0RGB buffer.
pub fn draw(buf: &mut [u32], w: usize, h: usize, t: f32, status: &str) {
    let mut c = Canvas { buf, w, h };
    c.buf.fill(BACKGROUND);
    let unit = (w.min(h) as f32 / 270.0).clamp(1.0, 6.0);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0 - 18.0 * unit);

    // Spinner: twelve dots, a bright head chasing round.
    let radius = 14.0 * unit;
    let head = (t * 12.0 / 1.1) % 12.0;
    for i in 0..12 {
        let angle = i as f32 / 12.0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let behind = (head - i as f32).rem_euclid(12.0);
        let strength = (1.0 - behind / 12.0).powf(2.2).max(0.12);
        c.disc(cx + angle.cos() * radius, cy + angle.sin() * radius, 2.2 * unit, blend(strength));
    }

    let scale = unit.round().max(1.0) as i64;
    let dots = ".".repeat((t * 2.0) as usize % 4);
    let line = format!("{status}{dots}{}", " ".repeat(3 - dots.len()));
    c.text(&line, (cy + radius + 22.0 * unit) as i64, scale, TEXT);
    let hint_scale = (scale / 2).max(1);
    c.text("CTRL+ALT+SHIFT+Q TO DISCONNECT", h as i64 - 16 * hint_scale - 8, hint_scale, DIM);
}

#[cfg(test)]
mod tests {
    #[test]
    fn draws_at_any_size_without_panicking() {
        for (w, h) in [(1, 1), (37, 9), (320, 240), (1280, 720), (3840, 2160)] {
            let mut buf = vec![0u32; w * h];
            super::draw(&mut buf, w, h, 1.3, "Connecting to the host");
            assert!(buf.contains(&super::BACKGROUND) || w * h < 4);
        }
    }
}

#[cfg(test)]
mod preview {
    /// Writes a raw 0RGB frame to `$MIST_SPLASH_OUT` for a visual check.
    #[test]
    #[ignore]
    fn dump() {
        let (w, h) = (1280usize, 720usize);
        let mut buf = vec![0u32; w * h];
        super::draw(&mut buf, w, h, 2.3, "Waiting for the host to accept");
        let bytes: Vec<u8> = buf.iter().flat_map(|p| p.to_le_bytes()).collect();
        std::fs::write(std::env::var("MIST_SPLASH_OUT").unwrap(), bytes).unwrap();
    }
}
