use super::*;
use crate::test_support::ScopedEnv;
use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static UNIQUE_TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let pid = process::id();
    let counter = UNIQUE_TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}-{counter}"))
}

fn isolated_home(prefix: &str) -> (ScopedEnv, PathBuf) {
    let mut env = ScopedEnv::new();
    let home = unique_temp_dir(prefix);
    fs::create_dir_all(&home).expect("create isolated home");
    env.set("HOME", &home);
    env.remove("LOONG_HOME");
    env.remove("LOONG_CONFIG_PATH");
    (env, home)
}

#[test]
fn resolve_default_entry_command_routes_to_onboard_when_config_is_missing() {
    let (_env, _home) = isolated_home("loong-default-entry-missing");

    assert!(
        matches!(resolve_default_entry_command(), Commands::Onboard { .. }),
        "missing config should route to onboard"
    );
}

#[test]
fn resolve_default_entry_command_routes_to_welcome_when_default_config_exists() {
    let (_env, _home) = isolated_home("loong-default-entry-present");
    let config_path = mvp::config::default_config_path();
    mvp::config::write(
        Some(config_path.to_str().expect("utf8 config path")),
        &mvp::config::LoongConfig::default(),
        true,
    )
    .expect("write default config");

    assert!(
        matches!(resolve_default_entry_command(), Commands::Welcome),
        "present config should route to welcome"
    );
}

#[test]
fn resolve_default_entry_command_routes_to_import_when_only_legacy_config_exists() {
    let (_env, home) = isolated_home("loong-default-entry-legacy");
    let legacy_dir = home.join(mvp::config::LEGACY_HOME_DIR_NAME);
    let legacy_path = legacy_dir.join("config.toml");
    fs::create_dir_all(&legacy_dir).expect("create legacy config dir");
    mvp::config::write(
        Some(legacy_path.to_str().expect("utf8 legacy config path")),
        &mvp::config::LoongConfig::default(),
        true,
    )
    .expect("write legacy config");

    assert!(
        matches!(
            resolve_default_entry_command(),
            Commands::Import {
                output: None,
                force: false,
                preview: false,
                apply: false,
                json: false,
                from: None,
                source_path: None,
                provider: None,
                include: _,
                exclude: _,
            }
        ),
        "legacy config should route to import preview"
    );
}

#[test]
fn resolve_default_entry_command_honors_loong_config_path_override() {
    let mut env = ScopedEnv::new();
    let config_path = unique_temp_dir("loong-default-entry-env").join("custom-config.toml");
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).expect("create config parent");
    }
    mvp::config::write(
        Some(config_path.to_str().expect("utf8 config path")),
        &mvp::config::LoongConfig::default(),
        true,
    )
    .expect("write explicit config");
    env.set("LOONG_CONFIG_PATH", &config_path);

    assert!(
        matches!(resolve_default_entry_command(), Commands::Welcome),
        "env override config should route to welcome"
    );
}

#[test]
fn resolve_default_entry_command_routes_to_onboard_when_config_path_is_a_directory() {
    let mut env = ScopedEnv::new();
    let config_dir = unique_temp_dir("loong-default-entry-dir");
    fs::create_dir_all(&config_dir).expect("create config directory");
    env.set("LOONG_CONFIG_PATH", &config_dir);

    assert!(
        matches!(resolve_default_entry_command(), Commands::Onboard { .. }),
        "directory config path should still route to onboard"
    );
}

#[test]
fn redacted_command_name_omits_sensitive_command_payloads() {
    let command = Commands::RunTask {
        objective: "secret objective".to_owned(),
        payload: "{\"api_key\":\"secret\"}".to_owned(),
    };

    let redacted_name = redacted_command_name(&command);

    assert_eq!(redacted_name, "run_task");
}

#[test]
fn run_welcome_cli_rejects_missing_config_file() {
    let mut env = ScopedEnv::new();
    let config_path = unique_temp_dir("loong-welcome-missing").join("missing-config.toml");
    env.set("LOONG_CONFIG_PATH", &config_path);

    let error = run_welcome_cli().expect_err("missing config should fail welcome");

    assert!(
        error.contains("Config file not found"),
        "welcome should explain the missing config file: {error}"
    );
    assert!(
        error.contains("loong onboard"),
        "welcome should point users back to onboarding: {error}"
    );
}

#[test]
fn run_welcome_cli_rejects_directory_config_path() {
    let mut env = ScopedEnv::new();
    let config_dir = unique_temp_dir("loong-welcome-dir");
    fs::create_dir_all(&config_dir).expect("create config directory");
    env.set("LOONG_CONFIG_PATH", &config_dir);

    let error = run_welcome_cli().expect_err("directory config path should fail welcome");

    assert!(
        error.contains("Config file not found"),
        "welcome should reject directory config paths as missing config files: {error}"
    );
}

#[test]
fn render_welcome_banner_includes_version_and_next_commands() {
    let config = mvp::config::LoongConfig::default();
    let rendered = render_welcome_banner(Path::new("/tmp/loong's config.toml"), &config);

    assert!(
        rendered.contains(env!("CARGO_PKG_VERSION")),
        "welcome banner should include the current version: {rendered}"
    );
    assert!(
        rendered.contains("loong ask --config '/tmp/loong'\"'\"'s config.toml'"),
        "welcome banner should include a quoted ask command: {rendered}"
    );
    assert!(
        rendered.contains("loong chat --config '/tmp/loong'\"'\"'s config.toml'"),
        "welcome banner should include a quoted chat command: {rendered}"
    );
    assert!(
        rendered.contains("loong personalize --config '/tmp/loong'\"'\"'s config.toml'"),
        "welcome banner should include a quoted personalize command: {rendered}"
    );
    assert!(
        rendered.contains("loong --help"),
        "welcome banner should point users to root help: {rendered}"
    );
    assert!(
        rendered.contains("- first answer:"),
        "welcome banner should preserve the shared next-action label for ask: {rendered}"
    );
    assert!(
        rendered.contains("- working preferences:"),
        "welcome banner should preserve the shared next-action label for personalize: {rendered}"
    );
}
