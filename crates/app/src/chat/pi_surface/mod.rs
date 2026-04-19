pub mod app;
pub mod command_palette;
pub mod composer;
pub mod diff_viewer;
pub mod i18n;
pub mod image_viewer;
pub mod markdown;
pub mod message_list;
pub mod utils;

use crate::CliResult;
use crate::chat::{CliChatOptions, ConcurrentCliHostOptions, initialize_cli_turn_runtime};
use crossterm::terminal;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;

pub(super) fn interactive_terminal_surface_supported() -> bool {
    true
}

pub(super) async fn run_cli_chat_surface(
    config_path: Option<&str>,
    session_hint: Option<&str>,
    options: &CliChatOptions,
) -> CliResult<()> {
    let runtime = initialize_cli_turn_runtime(config_path, session_hint, options, "cli-chat")?;

    terminal::enable_raw_mode().map_err(|e| format!("failed to enable raw mode: {}", e))?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)
        .map_err(|e| format!("failed to enter alternate screen: {}", e))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| format!("failed to create terminal: {}", e))?;
    terminal
        .show_cursor()
        .map_err(|e| format!("failed to show cursor: {}", e))?;

    let res = app::run_app(&mut terminal, runtime, options.clone()).await;

    terminal::disable_raw_mode().map_err(|e| format!("failed to disable raw mode: {}", e))?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    )
    .map_err(|e| format!("failed to leave alternate screen: {}", e))?;
    terminal
        .show_cursor()
        .map_err(|e| format!("failed to show cursor: {}", e))?;

    res
}

pub(super) fn run_concurrent_cli_host_surface(
    _options: &ConcurrentCliHostOptions,
) -> CliResult<()> {
    Ok(())
}
