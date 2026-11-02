//! Size-tier selection (SPEC §4.2).

use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeTier {
    Wide,
    Standard,
    Compact,
    TooSmall,
}

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 18;

pub fn tier(area: Rect) -> SizeTier {
    let (w, h) = (area.width, area.height);
    if w < MIN_WIDTH || h < MIN_HEIGHT {
        SizeTier::TooSmall
    } else if w >= 110 && h >= 30 {
        SizeTier::Wide
    } else if w >= 80 && h >= 24 {
        SizeTier::Standard
    } else {
        SizeTier::Compact
    }
}
