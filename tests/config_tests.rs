use std::path::Path;
use systui::config::*;

fn parse(s: &str) -> Result<Config, ConfigError> {
    Config::from_toml(s, Path::new("test.toml"))
}

#[test]
fn defaults_are_valid() {
    let c = Config::default();
    c.validate().unwrap();
    assert_eq!(c.ui.refresh_ms, 1000);
    assert_eq!(c.ui.history_samples, 300);
    assert!(c.processes.confirm_terminate);
}

#[test]
fn empty_file_uses_defaults() {
    assert_eq!(parse("").unwrap().ui.refresh_ms, 1000);
}

#[test]
fn partial_file_merges_with_defaults() {
    let c = parse("[ui]\ntheme = \"light\"\n").unwrap();
    assert_eq!(c.ui.theme, ThemeName::Light);
    assert_eq!(c.ui.refresh_ms, 1000);
}

#[test]
fn zero_refresh_is_rejected_with_actionable_message() {
    let e = parse("[ui]\nrefresh_ms = 0\n").unwrap_err().to_string();
    assert!(e.contains("ui.refresh_ms") && e.contains("500-5000"), "{e}");
}

#[test]
fn out_of_range_values_rejected() {
    assert!(parse("[ui]\nrefresh_ms = 60000\n").is_err());
    assert!(parse("[ui]\nhistory_samples = 5\n").is_err());
    assert!(parse("[ui]\nhistory_samples = 100000\n").is_err());
}

#[test]
fn bad_type_and_bad_enum_are_parse_errors() {
    assert!(matches!(
        parse("[ui]\nrefresh_ms = \"fast\"\n"),
        Err(ConfigError::Parse { .. })
    ));
    assert!(matches!(
        parse("[ui]\ntheme = \"neon\"\n"),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn unknown_keys_do_not_break_use() {
    assert!(parse("[future]\nthing = 1\n[ui]\nmystery = true\n").is_ok());
}

#[test]
fn cli_override_takes_precedence_and_is_validated() {
    let mut c = parse("[ui]\nrefresh_ms = 2000\n").unwrap();
    c.apply_overrides(Some(750)).unwrap();
    assert_eq!(c.ui.refresh_ms, 750);
    assert!(c.apply_overrides(Some(10)).is_err());
    assert_eq!(c.ui.refresh_ms, 750, "failed override must not mutate");
}

#[test]
fn missing_file_defaults_unless_explicit() {
    let p = Path::new("definitely/not/here.toml");
    assert!(Config::load(p, false).is_ok());
    assert!(matches!(
        Config::load(p, true),
        Err(ConfigError::Read { .. })
    ));
}

#[test]
fn example_config_parses() {
    let text = std::fs::read_to_string("config/config.example.toml").unwrap();
    parse(&text).unwrap();
}
