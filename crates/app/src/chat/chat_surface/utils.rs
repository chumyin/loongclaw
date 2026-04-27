use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use serde_json::Value;
use std::env;
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

// Surface palette
pub const SURFACE_CYAN: Color = LOONG_MAYA_BLUE_FALLBACK;
pub const SURFACE_GREEN: Color = LOONG_EMERALD;
pub const SURFACE_RED: Color = Color::Rgb(255, 46, 0);
pub const SURFACE_HEADING: Color = LOONG_AMETHYST_SMOKE;
pub const SURFACE_ACCENT: Color = LOONG_POWDER_BLUE;
pub const SURFACE_GRAY: Color = Color::Rgb(168, 168, 168);
pub const SURFACE_DIM_GRAY: Color = Color::Rgb(142, 142, 142);

const LOONG_MAYA_BLUE_FALLBACK: Color = Color::Rgb(112, 193, 255);

// Dynamic Backgrounds for blocks
pub const SURFACE_USER_MSG_BG: Color = LOONG_USER_HI_BG;
pub const SURFACE_TOOL_BG: Color = LOONG_TOOL_READ_BG;
pub const SURFACE_COMPACTION_BG: Color = Color::Rgb(40, 40, 50); // Muted base for the tag to sit on
pub const SURFACE_COTTON_CANDY: Color = LOONG_COTTON_CANDY;

pub(crate) const REQUEST_COMMAND_KEYS: &[&str] = &["cmd", "command", "script"];
pub(crate) const REQUEST_TEXT_KEYS: &[&str] = &["query", "pattern", "needle", "text"];
pub(crate) const REQUEST_GLOB_KEYS: &[&str] = &["glob", "pattern", "query", "pathspec"];
pub(crate) const REQUEST_PATH_KEYS: &[&str] =
    &["path", "file_path", "absolute_path", "source", "url"];
pub(crate) const REQUEST_SCOPE_KEYS: &[&str] = &["scope"];
pub(crate) const REQUEST_HIDDEN_KEYS: &[&str] = &["includeHidden", "hidden"];

pub fn reduced_motion_enabled() -> bool {
    env_truthy("LOONG_TUI_REDUCED_MOTION")
        || env::var("TERM")
            .map(|term| term.eq_ignore_ascii_case("dumb"))
            .unwrap_or(false)
}

fn env_truthy(name: &str) -> bool {
    env::var(name)
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "" | "0" | "false" | "off" | "no")
        })
        .unwrap_or(false)
}

/// Dynamic Focus Ring Animation
pub fn focus_ring_frame(start_time: Instant) -> &'static str {
    if reduced_motion_enabled() {
        return "•";
    }
    let elapsed_ms = start_time.elapsed().as_millis() as u64;
    let current_interval = if elapsed_ms < 5000 {
        80 + (70 * elapsed_ms / 5000)
    } else {
        150
    };
    let frame_index = (elapsed_ms / current_interval) as usize;
    let selected_index = frame_index % FOCUS_RING_FRAMES.len();
    FOCUS_RING_FRAMES
        .get(selected_index)
        .copied()
        .unwrap_or(FOCUS_RING_FRAMES.first().copied().unwrap_or("·"))
}

pub fn spinner_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u64) << 32)
}

/// Session-randomized spinner verb order while keeping time-based animation.
pub fn get_spinner_verb_with_seed(
    start_time: Instant,
    seed: u64,
    spinner_verbs: &'static [&'static str],
) -> &'static str {
    let default_verb = spinner_verbs.first().copied().unwrap_or("thinking");
    if reduced_motion_enabled() {
        return default_verb;
    }
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
    let selected_index = h as usize % spinner_verbs.len().max(1);
    spinner_verbs
        .get(selected_index)
        .copied()
        .unwrap_or(default_verb)
}

pub fn compact_structured_preview(text: &str, max_fields: usize) -> Option<String> {
    let value = serde_json::from_str::<Value>(text.trim()).ok()?;
    let object = value.as_object()?;
    if object.is_empty() {
        return Some("{}".to_owned());
    }

    let mut parts = object
        .iter()
        .filter_map(|(key, value)| {
            compact_preview_value(value).map(|value| format!("{key}={value}"))
        })
        .take(max_fields)
        .collect::<Vec<_>>();

    if object.len() > max_fields {
        parts.push("…".to_owned());
    }

    if parts.is_empty() {
        Some("…".to_owned())
    } else {
        Some(parts.join(" · "))
    }
}

pub fn render_inline_token_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut in_whitespace = None;

    let flush =
        |spans: &mut Vec<Span<'static>>, current: &mut String, in_whitespace: &mut Option<bool>| {
            let Some(is_whitespace) = *in_whitespace else {
                return;
            };
            if current.is_empty() {
                return;
            }
            let text = std::mem::take(current);
            if is_whitespace {
                spans.push(Span::raw(text));
            } else {
                spans.push(Span::styled(
                    text.clone(),
                    inline_token_style(&text, base_style),
                ));
            }
            *in_whitespace = None;
        };

    for ch in text.chars() {
        let is_whitespace = ch.is_whitespace();
        match in_whitespace {
            Some(mode) if mode == is_whitespace => current.push(ch),
            Some(_) => {
                flush(&mut spans, &mut current, &mut in_whitespace);
                current.push(ch);
                in_whitespace = Some(is_whitespace);
            }
            None => {
                current.push(ch);
                in_whitespace = Some(is_whitespace);
            }
        }
    }

    flush(&mut spans, &mut current, &mut in_whitespace);
    spans
}

fn inline_token_style(token: &str, base_style: Style) -> Style {
    let trimmed = token.trim_matches(|ch: char| ",.;:!?()[]{}".contains(ch));
    if trimmed.starts_with('/') || trimmed.starts_with('$') {
        return Style::default()
            .fg(SURFACE_ACCENT)
            .add_modifier(Modifier::BOLD);
    }

    if matches!(
        trimmed,
        "Ctrl+C"
            | "ctrl+c"
            | "Ctrl+O"
            | "ctrl+o"
            | "Tab"
            | "Shift+Enter"
            | "PgDn"
            | "End"
            | "Option"
            | "Alt"
    ) {
        return Style::default()
            .fg(SURFACE_CYAN)
            .add_modifier(Modifier::BOLD);
    }

    base_style
}

fn compact_preview_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Null => Some("null".to_owned()),
        Value::Array(items) => Some(if items.is_empty() {
            "[]".to_owned()
        } else {
            "…".to_owned()
        }),
        Value::Object(object) => Some(if object.is_empty() {
            "{}".to_owned()
        } else {
            "…".to_owned()
        }),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn first_string_field_recursive(
    value: &Value,
    keys: &[&str],
    depth: usize,
) -> Option<String> {
    if depth > 3 {
        return None;
    }
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(text) = object.get(*key).and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    return Some(text.trim().to_owned());
                }
            }
            object
                .values()
                .find_map(|value| first_string_field_recursive(value, keys, depth + 1))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|value| first_string_field_recursive(value, keys, depth + 1)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn numeric_json_field(value: &Value, key: &str) -> Option<u64> {
    numeric_json_field_recursive(value, key, 0)
}

#[cfg_attr(not(test), allow(dead_code))]
fn numeric_json_field_recursive(value: &Value, key: &str, depth: usize) -> Option<u64> {
    if depth > 3 {
        return None;
    }
    match value {
        Value::Object(object) => object.get(key).and_then(json_value_as_u64).or_else(|| {
            object
                .values()
                .find_map(|value| numeric_json_field_recursive(value, key, depth + 1))
        }),
        Value::Array(items) => items
            .iter()
            .find_map(|value| numeric_json_field_recursive(value, key, depth + 1)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn json_value_as_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn bool_json_field(value: &Value, keys: &[&str]) -> Option<bool> {
    bool_json_field_recursive(value, keys, 0)
}

#[cfg_attr(not(test), allow(dead_code))]
fn bool_json_field_recursive(value: &Value, keys: &[&str], depth: usize) -> Option<bool> {
    if depth > 3 {
        return None;
    }
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(value) = object.get(*key).and_then(json_value_as_bool) {
                    return Some(value);
                }
            }
            object
                .values()
                .find_map(|value| bool_json_field_recursive(value, keys, depth + 1))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|value| bool_json_field_recursive(value, keys, depth + 1)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn json_value_as_bool(value: &Value) -> Option<bool> {
    value.as_bool().or_else(|| match value.as_str()?.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn first_path_field(value: &Value) -> Option<String> {
    first_path_field_recursive(value, 0)
}

#[cfg_attr(not(test), allow(dead_code))]
fn first_path_field_recursive(value: &Value, depth: usize) -> Option<String> {
    if depth > 3 {
        return None;
    }
    match value {
        Value::Object(object) => {
            for &key in REQUEST_PATH_KEYS {
                if let Some(value) = object.get(key).and_then(Value::as_str)
                    && !value.trim().is_empty()
                {
                    return Some(value.trim().to_owned());
                }
            }
            object
                .values()
                .find_map(|value| first_path_field_recursive(value, depth + 1))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|value| first_path_field_recursive(value, depth + 1)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

pub(crate) fn embedded_json_value(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&text[start..=end]).ok()
}

pub(crate) fn embedded_json_string_field(text: &str, keys: &[&str]) -> Option<String> {
    let value = embedded_json_value(text)?;
    first_string_field_recursive(&value, keys, 0)
}

pub(crate) fn embedded_json_numeric_field(text: &str, key: &str) -> Option<u64> {
    let value = embedded_json_value(text)?;
    numeric_json_field(&value, key)
}

pub(crate) fn embedded_json_bool_field(text: &str, keys: &[&str]) -> Option<bool> {
    let value = embedded_json_value(text)?;
    bool_json_field(&value, keys)
}

pub(crate) fn embedded_json_path_field(text: &str) -> Option<String> {
    let value = embedded_json_value(text)?;
    first_path_field(&value)
}

pub(crate) fn request_string_from_candidates(
    candidates: &[Option<&str>],
    keys: &[&str],
) -> Option<String> {
    let texts = candidates.iter().flatten().copied().collect::<Vec<_>>();
    request_string_from_texts(texts.as_slice(), keys)
}

pub(crate) fn request_numeric_from_candidates(
    candidates: &[Option<&str>],
    key: &str,
) -> Option<u64> {
    let texts = candidates.iter().flatten().copied().collect::<Vec<_>>();
    request_numeric_from_texts(texts.as_slice(), key)
}

pub(crate) fn request_bool_from_candidates(
    candidates: &[Option<&str>],
    keys: &[&str],
) -> Option<bool> {
    let texts = candidates.iter().flatten().copied().collect::<Vec<_>>();
    request_bool_from_texts(texts.as_slice(), keys)
}

pub(crate) fn request_path_from_candidates(candidates: &[Option<&str>]) -> Option<String> {
    let texts = candidates.iter().flatten().copied().collect::<Vec<_>>();
    request_path_from_texts(texts.as_slice())
}

pub(crate) fn request_string_from_texts(texts: &[&str], keys: &[&str]) -> Option<String> {
    texts.iter().find_map(|text| {
        embedded_json_string_field(text, keys).or_else(|| {
            keys.iter()
                .find_map(|key| line_key_value(text, key))
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
    })
}

pub(crate) fn request_numeric_from_texts(texts: &[&str], key: &str) -> Option<u64> {
    texts.iter().find_map(|text| {
        embedded_json_numeric_field(text, key).or_else(|| line_numeric_key_value(text, key))
    })
}

pub(crate) fn request_bool_from_texts(texts: &[&str], keys: &[&str]) -> Option<bool> {
    texts.iter().find_map(|text| {
        embedded_json_bool_field(text, keys)
            .or_else(|| keys.iter().find_map(|key| line_bool_key_value(text, key)))
    })
}

pub(crate) fn request_path_from_texts(texts: &[&str]) -> Option<String> {
    texts.iter().find_map(|text| {
        embedded_json_path_field(text)
            .or_else(|| line_key_value(text, "path"))
            .or_else(|| line_key_value(text, "file_path"))
            .or_else(|| line_key_value(text, "absolute_path"))
            .or_else(|| extract_path_like_text(text))
    })
}

pub(crate) fn line_key_value(text: &str, key: &str) -> Option<String> {
    let marker = format!("{key}=");
    let start = text.find(marker.as_str())? + marker.len();
    let rest = &text[start..];
    let end = rest
        .find(" · ")
        .or_else(|| rest.find(", "))
        .or_else(|| rest.find('}'))
        .unwrap_or(rest.len());
    let value = rest[..end]
        .trim()
        .trim_matches(',')
        .trim_matches('"')
        .trim_matches('\'')
        .to_owned();
    (!value.is_empty()).then_some(value)
}

pub(crate) fn line_numeric_key_value(text: &str, key: &str) -> Option<u64> {
    line_key_value(text, key)?.parse::<u64>().ok()
}

pub(crate) fn line_bool_key_value(text: &str, key: &str) -> Option<bool> {
    match line_key_value(text, key)?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn extract_path_like_text(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_matches('"').trim_matches('\'');
    if trimmed.starts_with('/')
        || trimmed.starts_with("~/")
        || trimmed.starts_with("./")
        || trimmed.starts_with("../")
        || trimmed.starts_with("file://")
    {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

pub(crate) fn normalized_tool_name(name: &str) -> String {
    name.trim_matches(|ch: char| ch == '`' || ch == '"' || ch == '\'')
        .rsplit(['.', '/', ':'])
        .next()
        .unwrap_or(name)
        .to_owned()
}

pub(crate) fn is_read_tool_name(name: &str) -> bool {
    matches!(
        name,
        "read" | "read_file" | "read-file" | "readfile" | "open_file" | "open-file" | "cat"
    )
}

pub(crate) fn is_run_tool_name(name: &str) -> bool {
    matches!(
        name,
        "bash" | "shell" | "sh" | "exec_command" | "run_command" | "terminal" | "cmd"
    )
}

pub(crate) fn is_search_tool_name(name: &str) -> bool {
    matches!(
        name,
        "search" | "grep" | "ripgrep" | "rg" | "find" | "find_text"
    )
}

pub(crate) fn is_list_tool_name(name: &str) -> bool {
    matches!(
        name,
        "list" | "ls" | "list_directory" | "list_dir" | "read_dir" | "dir"
    )
}

pub(crate) fn is_glob_tool_name(name: &str) -> bool {
    matches!(name, "glob" | "find_files" | "find_file" | "walk")
}

pub(crate) fn format_search_summary(query: &str, path: Option<&str>, limit: Option<u64>) -> String {
    let mut summary = if let Some(path) = path {
        format!("\"{query}\" in {path}")
    } else {
        format!("\"{query}\"")
    };
    if let Some(limit) = limit {
        summary.push_str(format!(" · limit {limit}").as_str());
    }
    summary
}

pub(crate) fn format_list_summary(path: &str, limit: Option<u64>) -> String {
    let mut summary = path.to_owned();
    if let Some(limit) = limit {
        summary.push_str(format!(" · limit {limit}").as_str());
    }
    summary
}

pub(crate) fn format_glob_summary(pattern: &str, path: Option<&str>, limit: Option<u64>) -> String {
    let mut summary = if let Some(path) = path {
        format!("{pattern} in {path}")
    } else {
        pattern.to_owned()
    };
    if let Some(limit) = limit {
        summary.push_str(format!(" · limit {limit}").as_str());
    }
    summary
}

pub(crate) fn format_inspect_secondary_details(
    depth: Option<u64>,
    hidden: Option<bool>,
) -> Option<String> {
    let mut details = Vec::new();
    if let Some(depth) = depth {
        details.push(format!("depth {depth}"));
    }
    if hidden == Some(true) {
        details.push("hidden on".to_owned());
    }
    if details.is_empty() {
        None
    } else {
        Some(details.join(" · "))
    }
}

pub(crate) fn format_read_line_range(offset: u64, limit: Option<u64>) -> String {
    let start = offset.max(1);
    match limit.and_then(|limit| limit.checked_sub(1)) {
        Some(limit_tail) if limit_tail > 0 => format!(":{start}-{}", start + limit_tail),
        _ => format!(":{start}"),
    }
}

pub(crate) fn format_read_summary(path: &str, offset: Option<u64>, limit: Option<u64>) -> String {
    let mut display = shorten_display_path(path);
    if let Some(offset) = offset {
        display.push_str(format_read_line_range(offset, limit).as_str());
    }
    display
}

pub(crate) fn shorten_display_path(path: &str) -> String {
    let path = path.trim();
    if let Some(home) = env::var_os("HOME").and_then(|home| home.into_string().ok())
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(home.as_str())
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("~{rest}");
    }
    path.to_owned()
}

#[allow(dead_code)]
pub fn split_inline_list_runs(line: &str) -> Option<Vec<String>> {
    split_inline_bullet_runs(line).or_else(|| split_inline_numbered_runs(line))
}

#[allow(dead_code)]
fn split_inline_bullet_runs(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if trimmed.matches("• ").count() < 2 {
        return None;
    }

    let items = trimmed
        .split("• ")
        .filter_map(|segment| {
            let segment = segment.trim();
            (!segment.is_empty()).then(|| format!("• {segment}"))
        })
        .collect::<Vec<_>>();

    (items.len() >= 2).then_some(items)
}

#[allow(dead_code)]
fn split_inline_numbered_runs(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    let chars = trimmed.char_indices().collect::<Vec<_>>();
    let mut starts = Vec::new();
    let mut idx = 0usize;

    while idx < chars.len() {
        let Some(&(byte_idx, ch)) = chars.get(idx) else {
            break;
        };
        let previous_is_non_whitespace = idx > 0
            && chars
                .get(idx - 1)
                .is_some_and(|(_, previous)| !previous.is_whitespace());
        if previous_is_non_whitespace {
            idx += 1;
            continue;
        }
        if !ch.is_ascii_digit() {
            idx += 1;
            continue;
        }

        let mut end_idx = idx + 1;
        while chars
            .get(end_idx)
            .is_some_and(|(_, next)| next.is_ascii_digit())
        {
            end_idx += 1;
        }
        let Some((_, marker)) = chars.get(end_idx) else {
            idx = end_idx;
            continue;
        };
        let Some((_, after)) = chars.get(end_idx + 1) else {
            idx = end_idx;
            continue;
        };
        if (*marker == '.' || *marker == ')') && *after == ' ' {
            starts.push(byte_idx);
        }
        idx = end_idx;
    }

    if starts.len() < 2 {
        return None;
    }

    let mut items = Vec::new();
    for (index, start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(trimmed.len());
        let segment = trimmed[*start..end].trim();
        if !segment.is_empty() {
            items.push(segment.to_owned());
        }
    }

    (items.len() >= 2).then_some(items)
}

#[cfg(test)]
mod tests {
    use crate::chat::chat_surface::i18n::{Language, spinner_verbs_for};
    use serde_json::json;
    use std::time::Instant;

    #[test]
    fn spinner_verbs_follow_selected_language_family() {
        let start = Instant::now();

        let zh_cn = super::get_spinner_verb_with_seed(start, 7, spinner_verbs_for(Language::ZhCn));
        let zh_tw = super::get_spinner_verb_with_seed(start, 7, spinner_verbs_for(Language::ZhTw));
        let ja = super::get_spinner_verb_with_seed(start, 7, spinner_verbs_for(Language::Ja));
        let ru = super::get_spinner_verb_with_seed(start, 7, spinner_verbs_for(Language::Ru));
        let en = super::get_spinner_verb_with_seed(start, 7, spinner_verbs_for(Language::En));

        assert!(spinner_verbs_for(Language::ZhCn).contains(&zh_cn));
        assert!(spinner_verbs_for(Language::ZhTw).contains(&zh_tw));
        assert!(spinner_verbs_for(Language::Ja).contains(&ja));
        assert!(spinner_verbs_for(Language::Ru).contains(&ru));
        assert!(spinner_verbs_for(Language::En).contains(&en));
    }

    #[test]
    fn split_inline_list_runs_supports_bullets_and_numbered_items() {
        let bullets = super::split_inline_list_runs("• one • two • three").expect("bullets");
        assert_eq!(
            bullets,
            vec!["• one".to_owned(), "• two".to_owned(), "• three".to_owned()]
        );

        let numbered = super::split_inline_list_runs("1. one 2. two 3. three").expect("numbered");
        assert_eq!(
            numbered,
            vec![
                "1. one".to_owned(),
                "2. two".to_owned(),
                "3. three".to_owned()
            ]
        );
    }

    #[test]
    fn recursive_json_field_helpers_extract_nested_string_number_bool_and_path() {
        let value = json!({
            "outer": {
                "query": "rust",
                "nested": {
                    "limit": "5",
                    "includeHidden": true,
                    "source": "~/chat/.omx"
                }
            }
        });

        assert_eq!(
            super::first_string_field_recursive(&value, &["query"], 0).as_deref(),
            Some("rust")
        );
        assert_eq!(super::numeric_json_field(&value, "limit"), Some(5));
        assert_eq!(
            super::bool_json_field(&value, &["includeHidden", "hidden"]),
            Some(true)
        );
        assert_eq!(
            super::first_path_field(&value).as_deref(),
            Some("~/chat/.omx")
        );
    }

    #[test]
    fn tool_name_helpers_normalize_namespaced_aliases() {
        let normalized = super::normalized_tool_name("demo_mcp.find_files");
        assert_eq!(normalized, "find_files");
        assert!(super::is_glob_tool_name(normalized.as_str()));
        assert!(super::is_search_tool_name(
            super::normalized_tool_name("foo.search").as_str()
        ));
        assert!(super::is_list_tool_name(
            super::normalized_tool_name("pkg/list_directory").as_str()
        ));
        assert!(super::is_run_tool_name(
            super::normalized_tool_name("shell").as_str()
        ));
        assert!(super::is_read_tool_name(
            super::normalized_tool_name("filesystem.open_file").as_str()
        ));
    }

    #[test]
    fn request_summary_helpers_format_shared_semantics() {
        assert_eq!(
            super::format_search_summary("rust", Some("~/chat"), Some(5)),
            "\"rust\" in ~/chat · limit 5"
        );
        assert_eq!(
            super::format_list_summary("~/chat/.omx", Some(20)),
            "~/chat/.omx · limit 20"
        );
        assert_eq!(
            super::format_glob_summary("src/**/*.rs", Some("~/chat"), Some(5)),
            "src/**/*.rs in ~/chat · limit 5"
        );
        assert_eq!(
            super::format_inspect_secondary_details(Some(3), Some(true)).as_deref(),
            Some("depth 3 · hidden on")
        );
        assert_eq!(super::format_read_line_range(10, Some(3)), ":10-12");
        let home = std::env::var("HOME").expect("home");
        let path = format!("{home}/chat/file.rs");
        assert_eq!(
            super::format_read_summary(path.as_str(), Some(10), Some(3)),
            "~/chat/file.rs:10-12"
        );
    }

    #[test]
    fn embedded_json_value_extracts_object_from_prefixed_line() {
        let value = super::embedded_json_value(
            "request: {\"query\":\"rust\",\"limit\":5,\"nested\":{\"hidden\":true}}",
        )
        .expect("embedded json");

        assert_eq!(
            super::first_string_field_recursive(&value, &["query"], 0).as_deref(),
            Some("rust")
        );
        assert_eq!(super::numeric_json_field(&value, "limit"), Some(5));
        assert_eq!(super::bool_json_field(&value, &["hidden"]), Some(true));
    }

    #[test]
    fn embedded_json_field_helpers_extract_nested_scalars_and_path() {
        let line = "args: {\"outer\":{\"query\":\"rust\",\"limit\":\"5\",\"includeHidden\":true,\"path\":\"~/chat/.omx\"}}";

        assert_eq!(
            super::embedded_json_string_field(line, &["query"]).as_deref(),
            Some("rust")
        );
        assert_eq!(super::embedded_json_numeric_field(line, "limit"), Some(5));
        assert_eq!(
            super::embedded_json_bool_field(line, &["includeHidden", "hidden"]),
            Some(true)
        );
        assert_eq!(
            super::embedded_json_path_field(line).as_deref(),
            Some("~/chat/.omx")
        );
    }

    #[test]
    fn request_candidate_helpers_extract_fields_across_candidates() {
        let candidates = [
            Some("request: {\"query\":\"rust\",\"limit\":5}"),
            Some("args: {\"includeHidden\":true,\"path\":\"~/chat/.omx\"}"),
        ];

        assert_eq!(
            super::request_string_from_candidates(&candidates, &["query"]).as_deref(),
            Some("rust")
        );
        assert_eq!(
            super::request_numeric_from_candidates(&candidates, "limit"),
            Some(5)
        );
        assert_eq!(
            super::request_bool_from_candidates(&candidates, &["includeHidden", "hidden"]),
            Some(true)
        );
        assert_eq!(
            super::request_path_from_candidates(&candidates).as_deref(),
            Some("~/chat/.omx")
        );
    }

    #[test]
    fn request_text_helpers_extract_fields_across_multiple_lines() {
        let texts = [
            "request: {\"query\":\"rust\",\"limit\":5}",
            "includeHidden=true · path=~/chat/.omx",
        ];

        assert_eq!(
            super::request_string_from_texts(&texts, &["query"]).as_deref(),
            Some("rust")
        );
        assert_eq!(super::request_numeric_from_texts(&texts, "limit"), Some(5));
        assert_eq!(
            super::request_bool_from_texts(&texts, &["includeHidden", "hidden"]),
            Some(true)
        );
        assert_eq!(
            super::request_path_from_texts(&texts).as_deref(),
            Some("~/chat/.omx")
        );
    }

    #[test]
    fn line_key_value_helpers_extract_string_number_and_bool() {
        let line = "query=rust · limit=5 · includeHidden=true";

        assert_eq!(
            super::line_key_value(line, "query").as_deref(),
            Some("rust")
        );
        assert_eq!(super::line_numeric_key_value(line, "limit"), Some(5));
        assert_eq!(
            super::line_bool_key_value(line, "includeHidden"),
            Some(true)
        );
    }
}
