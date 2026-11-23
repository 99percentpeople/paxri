use std::sync::LazyLock;

static LINEAR_SRGB: LazyLock<[f32; 256]> = LazyLock::new(|| {
    std::array::from_fn(|i| {
        let s = i as f32 / 255.;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    })
});

fn luminance(rgb: u32) -> f32 {
    let [_, r, g, b] = rgb.to_be_bytes();
    let linear = &*LINEAR_SRGB;
    0.2126 * linear[r as usize] + 0.7152 * linear[g as usize] + 0.0722 * linear[b as usize]
}

fn contrasting_channel(luminance: f32) -> u8 {
    if (luminance + 0.05).powi(2) >= 0.05 * 1.05 {
        0
    } else {
        255
    }
}

pub(super) fn black_or_white(background: u32) -> u32 {
    let c = contrasting_channel(luminance(background));
    u32::from_be_bytes([0, c, c, c])
}

fn legible(ink: f32, paper: f32) -> bool {
    ink.max(paper) + 0.05 >= 4.5 * (ink.min(paper) + 0.05)
}

/// Move a low-contrast text color just far enough toward black or white.
/// The background and the palette reported to the application stay intact.
pub(super) fn readable_text(fg: u32, bg: u32) -> u32 {
    let paper = luminance(bg);
    if fg == bg || legible(luminance(fg), paper) {
        return fg;
    }
    let target = u32::from(contrasting_channel(paper));
    let [_, r, g, b] = fg.to_be_bytes();
    let mix = |amount: u32| {
        let channel = |c: u8| ((u32::from(c) * (256 - amount) + target * amount + 128) / 256) as u8;
        u32::from_be_bytes([0, channel(r), channel(g), channel(b)])
    };
    let (mut low, mut high) = (0, 256);
    while high - low > 1 {
        let mid = (low + high) / 2;
        if legible(luminance(mix(mid)), paper) {
            high = mid;
        } else {
            low = mid;
        }
    }
    mix(high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pastel_terminal_text_remains_colored_and_readable() {
        let paper = 0xfafafa;
        for ink in [0xcdd6f4, 0xa6e3a1, 0x89b4fa, 0x5fafff, 0xffffff] {
            let adjusted = readable_text(ink, paper);
            assert!(legible(luminance(adjusted), luminance(paper)));
            assert_eq!(readable_text(adjusted, paper), adjusted);
        }
        let [_, red, green, blue] = readable_text(0xa6e3a1, paper).to_be_bytes();
        assert!(green > red && green > blue);
        assert_eq!(readable_text(0x0a0a0a, paper), 0x0a0a0a);
        assert_eq!(readable_text(0x6a6a6a, paper), 0x6a6a6a);
        assert_eq!(readable_text(paper, paper), paper);
        let badge = 0x196869;
        assert!(legible(
            luminance(readable_text(0x171717, badge)),
            luminance(badge)
        ));
    }
}
