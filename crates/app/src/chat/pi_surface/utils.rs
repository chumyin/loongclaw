use crate::constants::spinners::*;
use ratatui::style::Color;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const FOCUS_RING_FRAMES: [&str; 18] = [
    "·", "·", "◦", "○", "◎", "◉", "●", "●", "●", "◉", "◎", "○", "◦", "·", "·", " ", " ", " ",
];

// LOONG Branding & Identity - Primary Palette
pub const LOONG_AMETHYST_SMOKE: Color = Color::Rgb(199, 131, 194); // #c783c2
pub const LOONG_EMERALD: Color = Color::Rgb(109, 190, 126); // #6dbe7e
pub const LOONG_POWDER_BLUE: Color = Color::Rgb(159, 184, 217); // #9fb8d9
pub const LOONG_COTTON_CANDY: Color = Color::Rgb(248, 146, 158); // #f8929e

// Final Targeted Block Colors (User Requested Refinements)
pub const LOONG_USER_HI_BG: Color = Color::Rgb(133, 180, 209); // #85B4D1 (The "hi" block)
pub const LOONG_TOOL_READ_BG: Color = Color::Rgb(197, 220, 169); // #C5DCA9 (The "read" block)
pub const LOONG_COMPACTION_TAG: Color = Color::Rgb(168, 234, 235); // #A8EAEB (The "compaction" label)

// Functional Aliases
pub const PI_CYAN: Color = LOONG_MAYA_BLUE_FALLBACK;
pub const PI_YELLOW: Color = Color::Rgb(255, 217, 122);
pub const PI_GREEN: Color = LOONG_EMERALD;
pub const PI_RED: Color = Color::Rgb(255, 46, 0);
pub const PI_HEADING: Color = LOONG_AMETHYST_SMOKE;
pub const PI_ACCENT: Color = LOONG_POWDER_BLUE;
pub const PI_GRAY: Color = Color::Rgb(128, 128, 128);
pub const PI_DIM_GRAY: Color = Color::Rgb(102, 102, 102);
pub const PI_DARK_GRAY: Color = Color::Rgb(40, 40, 40);

const LOONG_MAYA_BLUE_FALLBACK: Color = Color::Rgb(112, 193, 255);

// Dynamic Backgrounds for blocks
pub const PI_USER_MSG_BG: Color = LOONG_USER_HI_BG;
pub const PI_TOOL_BG: Color = LOONG_TOOL_READ_BG;
pub const PI_ERROR_BG: Color = Color::Rgb(54, 22, 28);
pub const PI_COMPACTION_BG: Color = Color::Rgb(40, 40, 50); // Muted base for the tag to sit on
pub const PI_COTTON_CANDY: Color = LOONG_COTTON_CANDY;

/// Dynamic Focus Ring Animation
pub fn focus_ring_frame(start_time: Instant) -> &'static str {
    let elapsed_ms = start_time.elapsed().as_millis() as u64;
    let current_interval = if elapsed_ms < 5000 {
        80 + (70 * elapsed_ms / 5000)
    } else {
        150
    };
    let frame_index = (elapsed_ms / current_interval) as usize;
    FOCUS_RING_FRAMES[frame_index % FOCUS_RING_FRAMES.len()]
}

pub fn spinner_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u64) << 32)
}

/// Session-randomized "Working..." verb order while keeping time-based animation.
pub fn get_spinner_verb_with_seed(start_time: Instant, seed: u64) -> &'static str {
    let elapsed_ms = start_time.elapsed().as_millis() as u64;
    let current_interval = if elapsed_ms < 5000 {
        80 + (70 * elapsed_ms / 5000)
    } else {
        150
    };
    let cycle_count = (elapsed_ms / current_interval) / FOCUS_RING_FRAMES.len() as u64;
    let mut h = cycle_count
        .wrapping_add(seed)
        .wrapping_add(0x9E3779B97F4A7C15);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D049BB133111EB);
    h = h ^ (h >> 31);
    SPINNERS_ZH_CN[h as usize % SPINNERS_ZH_CN.len()]
}
