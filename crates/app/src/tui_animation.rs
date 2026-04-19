use std::time::{Duration, Instant};
use std::sync::OnceLock;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

pub const TERMINAL_TITLE_SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

fn elapsed_since_start() -> Duration {
    let start = PROCESS_START.get_or_init(Instant::now);
    start.elapsed()
}

pub fn spinner_frame(start_time: Instant) -> &'static str {
    let elapsed = start_time.elapsed().as_millis();
    let frame_index = (elapsed / 100) as usize;
    TERMINAL_TITLE_SPINNER_FRAMES[frame_index % TERMINAL_TITLE_SPINNER_FRAMES.len()]
}

pub fn set_terminal_title(title: &str) -> String {
    format!("\x1b]0;{}\x07", title)
}

pub fn fmt_elapsed_compact(elapsed_secs: u64) -> String {
    if elapsed_secs < 60 {
        return format!("{elapsed_secs}s");
    }
    if elapsed_secs < 3600 {
        let minutes = elapsed_secs / 60;
        let seconds = elapsed_secs % 60;
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = elapsed_secs / 3600;
    let minutes = (elapsed_secs % 3600) / 60;
    let seconds = elapsed_secs % 60;
    format!("{hours}h {minutes:02}m {seconds:02}s")
}

pub fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let r = (fg.0 as f32 * alpha + bg.0 as f32 * (1.0 - alpha)) as u8;
    let g = (fg.1 as f32 * alpha + bg.1 as f32 * (1.0 - alpha)) as u8;
    let b = (fg.2 as f32 * alpha + bg.2 as f32 * (1.0 - alpha)) as u8;
    (r, g, b)
}

pub fn shimmer_spans(text: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    
    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let sweep_seconds = 2.0f32;
    let pos_f = (elapsed_since_start().as_secs_f32() % sweep_seconds) / sweep_seconds * (period as f32);
    let pos = pos_f as usize;
    
    let has_true_color = std::env::var("COLORTERM")
        .map(|v| v == "truecolor" || v == "24bit")
        .unwrap_or(false);
    
    let band_half_width = 5.0;
    let mut spans = Vec::with_capacity(chars.len());
    
    // Default Codex-aligned colors: Cyan for shimmer highlight, default grey for base
    let base_rgb = (100, 100, 100);
    let highlight_rgb = (0, 255, 255); // Cyan

    for (i, ch) in chars.iter().enumerate() {
        let i_pos = i as isize + padding as isize;
        let p = pos as isize;
        let dist = (i_pos - p).abs() as f32;

        let t = if dist <= band_half_width {
            let x = std::f32::consts::PI * (dist / band_half_width);
            0.5 * (1.0 + x.cos())
        } else {
            0.0
        };

        let style = if has_true_color {
            let (r, g, b) = blend(highlight_rgb, base_rgb, t);
            Style::default().fg(Color::Rgb(r, g, b))
        } else if t > 0.8 {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else if t > 0.4 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        };
        
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}
