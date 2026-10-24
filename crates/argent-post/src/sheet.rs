//! Contact sheets with captions. TODO(agent).

use crate::Rgb8;

pub struct Tile {
    pub image: Rgb8,
    pub caption: String,
}

pub struct SheetStyle {
    pub columns: u32,
    pub tile_width: u32,
    pub background: [u8; 3],
    pub text: [u8; 3],
    pub title: Option<String>,
    pub subtitle: Option<String>,
}

pub fn contact_sheet(_tiles: &[Tile], _style: &SheetStyle) -> Rgb8 {
    Rgb8::new(1, 1)
}
