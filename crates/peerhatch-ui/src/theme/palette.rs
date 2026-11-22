//! shadcn/ui Neutral in sRGB, with desktop interaction states and terminal ANSI colors.
pub struct ColorPair {
    pub background: u32,
    pub foreground: u32,
}

const fn pair(background: u32, foreground: u32) -> ColorPair {
    ColorPair {
        background,
        foreground,
    }
}

pub struct Palette {
    pub canvas: ColorPair,
    pub card: u32,
    pub muted_foreground: u32,
    pub border: u32,
    pub modal_overlay: u32,
    pub input_border: u32,
    pub ring: u32,
    pub selection: u32,
    pub primary: ColorPair,
    pub primary_hover: u32,
    pub secondary: ColorPair,
    pub secondary_hover: u32,
    pub accent: ColorPair,
    pub chrome: ColorPair,
    pub tab_hover: u32,
    pub destructive: ColorPair,
    pub destructive_hover: u32,
    pub alert: ColorPair,
    pub success: u32,
    pub warning: u32,
    pub terminal: TerminalPalette,
}

pub struct TerminalPalette {
    pub surface: ColorPair,
    pub selection: ColorPair,
    pub composition: u32,
    pub cursor: u32,
    pub ansi: [u32; 16],
}

pub static LIGHT: Palette = Palette {
    canvas: pair(0xffffff, 0x0a0a0a),
    card: 0xffffff,
    muted_foreground: 0x707070,
    border: 0xe5e5e5,
    modal_overlay: 0x00000055,
    input_border: 0xd4d4d4,
    ring: 0x737373,
    selection: 0xbdbdbd,
    primary: pair(0x171717, 0xfafafa),
    primary_hover: 0x2e2e2e,
    secondary: pair(0xf5f5f5, 0x171717),
    secondary_hover: 0xe5e5e5,
    accent: pair(0xf5f5f5, 0x171717),
    chrome: pair(0xf5f5f5, 0x171717),
    tab_hover: 0xfafafa,
    destructive: pair(0xe7000b, 0xffffff),
    destructive_hover: 0xc10007,
    alert: pair(0xfef2f2, 0x991b1b),
    success: 0x404040,
    warning: 0xa16207,
    terminal: TerminalPalette {
        surface: pair(0xfafafa, 0x0a0a0a),
        selection: pair(0xa1a1a1, 0x0a0a0a),
        composition: 0xe5e5e5,
        cursor: 0x171717,
        ansi: [
            0x171717, 0xa33536, 0x24633e, 0x805e14, 0x285c9c, 0x833f91, 0x196869, 0xb3b3b3,
            0x595959, 0xb33939, 0x29733f, 0x89640d, 0x315fb0, 0x905099, 0x146e76, 0xffffff,
        ],
    },
};

pub static DARK: Palette = Palette {
    canvas: pair(0x0a0a0a, 0xfafafa),
    card: 0x171717,
    muted_foreground: 0xa1a1a1,
    border: 0x2e2e2e,
    modal_overlay: 0x00000088,
    input_border: 0x3a3a3a,
    ring: 0xa1a1a1,
    selection: 0x525252,
    primary: pair(0xe5e5e5, 0x171717),
    primary_hover: 0xd0d0d0,
    secondary: pair(0x262626, 0xfafafa),
    secondary_hover: 0x353535,
    accent: pair(0x262626, 0xfafafa),
    chrome: pair(0x0a0a0a, 0xfafafa),
    tab_hover: 0x232323,
    destructive: pair(0xff6467, 0x171717),
    destructive_hover: 0xff8083,
    alert: pair(0x391919, 0xfecaca),
    success: 0xd4d4d4,
    warning: 0xfbbf24,
    terminal: TerminalPalette {
        surface: pair(0x0a0a0a, 0xfafafa),
        selection: pair(0x525252, 0xfafafa),
        composition: 0x262626,
        cursor: 0xe5e5e5,
        ansi: [
            0x171717, 0xe07572, 0x82c998, 0xe3c780, 0x80aee1, 0xc896d6, 0x70c5c1, 0xd4d4d4,
            0x737373, 0xff9290, 0xa3eeb2, 0xffdd94, 0xa2c7ff, 0xedb5ff, 0x94e9e2, 0xffffff,
        ],
    },
};

// A cool dark terminal with its own normal and bright ANSI colors.
pub(super) const MIDNIGHT: TerminalPalette = TerminalPalette {
    surface: pair(0x152033, 0xe0e9f5),
    selection: pair(0x354d70, 0xffffff),
    composition: 0x30425e,
    cursor: 0x82dce4,
    ansi: [
        0x29364b, 0xef9097, 0x91d4aa, 0xeac388, 0x88b4ef, 0xc1a2eb, 0x80d3dc, 0xcbd6e5, 0x7589a4,
        0xffabb1, 0xadeac1, 0xffdda5, 0xa9cafa, 0xd9bfff, 0xaff0f4, 0xf4f8ff,
    ],
};
