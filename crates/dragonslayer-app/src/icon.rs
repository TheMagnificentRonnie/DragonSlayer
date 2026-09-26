//! Chunky 16×16 pixel-art window icon. Green-on-black phosphor CRT vibe: a
//! rectangular monitor frame around a play triangle with a diagonal sword-slash
//! streaking off the bottom-left corner.
//!
//! Colours match the app's teal accent so the system taskbar and dock read as
//! consistent with the running UI. Kept small and hand-authored so there's no
//! decoder dependency at start-up.

const W: usize = 16;
const H: usize = 16;

// Palette indices into COLOURS. `.` = transparent.
const PIXELS: [&[u8; W]; H] = [
    b"................",
    b"..OOOOOOOOOOOO..",
    b".O............O.",
    b".O............O.",
    b".O.....A......O.",
    b".O.....AA.....O.",
    b".O.....AAA....O.",
    b".O.....AAAA...O.",
    b".O.....AAA....O.",
    b".O.....AA.....O.",
    b".O.....A......O.",
    b".O............O.",
    b".O............O.",
    b"..OOOOOOOOOOOO..",
    b"..S.............",
    b"...SS...........",
];

/// Palette: (r, g, b, a) per glyph.
fn colour(byte: u8) -> [u8; 4] {
    match byte {
        b'O' => [72, 190, 205, 255],   // Frame — teal accent.
        b'A' => [102, 255, 170, 255],  // Play triangle — bright phosphor.
        b'S' => [255, 184, 77, 255],   // Sword slash — amber accent.
        _ => [0, 0, 0, 0],
    }
}

/// RGBA pixel buffer for eframe's `IconData`.
pub fn rgba() -> Vec<u8> {
    let mut out = Vec::with_capacity(W * H * 4);
    for row in PIXELS.iter() {
        for &b in row.iter() {
            out.extend_from_slice(&colour(b));
        }
    }
    out
}

pub const WIDTH: u32 = W as u32;
pub const HEIGHT: u32 = H as u32;
