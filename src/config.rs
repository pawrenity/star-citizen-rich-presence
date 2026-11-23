use anyhow::{Context, Result, ensure};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_APP_ID: &str = "983874700440645672";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CardButton {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Star Citizen presence from Game.log. No overlay or OCR required."
)]
pub struct Args {
    /// TOML settings file (otherwise scrp.toml beside the executable or in this folder).
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Explicit Game.log. Also accepts SCRP_LOG in the environment.
    #[arg(long, env = "SCRP_LOG")]
    pub log: Option<PathBuf>,
    /// Game channel folder, e.g. C:\Games\StarCitizen\LIVE.
    #[arg(long, conflicts_with = "log")]
    pub game_dir: Option<PathBuf>,
    /// Preferred channel: LIVE, PTU, EPTU, or TECH-PREVIEW.
    #[arg(long)]
    pub channel: Option<String>,
    /// Your Discord application ID (the original project's ID is the default).
    #[arg(long)]
    pub app_id: Option<String>,
    /// Print presence updates without connecting to Discord.
    #[arg(long)]
    pub dry_run: bool,
    /// Parse a saved log once, print the resulting presence, and exit. No Discord connection.
    #[arg(long, conflicts_with_all = ["log", "game_dir", "discover"])]
    pub replay: Option<PathBuf>,
    /// List discovered game/log paths and exit. No Discord connection.
    #[arg(long)]
    pub discover: bool,
    /// Verify Discord's handshake and a temporary test presence, then clear it and exit.
    #[arg(long, conflicts_with_all = ["replay", "discover", "dry_run"])]
    pub check_discord: bool,
    /// Omit the public RSI profile button.
    #[arg(long)]
    pub no_profile: bool,
    /// Omit contract names from presence.
    #[arg(long)]
    pub no_contracts: bool,
    /// Omit short-lived blueprint, refinery, and hangar notifications.
    #[arg(long)]
    pub no_notifications: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub buttons: Vec<CardButton>,
    pub log_path: Option<PathBuf>,
    pub channel: String,
    pub discord_app_id: String,
    pub poll_seconds: u64,
    pub update_seconds: u64,
    pub location_ttl_seconds: u64,
    pub show_profile: bool,
    pub show_contracts: bool,
    pub show_notifications: bool,
    pub show_location: bool,
    pub show_ship: bool,
    pub show_party: bool,
    pub show_artwork: bool,
    pub custom_state: String,
    pub custom_details: String,
    /// Optional additional aliases; embedded aliases remain available.
    pub locations_file: Option<PathBuf>,
    /// Asset keys must exist in YOUR Discord application. Unlisted locations use logo.
    pub image_assets: std::collections::BTreeMap<String, String>,
    /// Ship name -> wiki choice number, HTTPS image URL, or Discord asset key.
    pub ship_images: std::collections::BTreeMap<String, String>,
    /// Mission type (e.g. `bounty`) or words from a contract title -> image, as above.
    pub mission_images: std::collections::BTreeMap<String, String>,
    /// Discord verb: playing, listening, watching, or competing.
    pub activity_type: String,
    /// What the member list shows after the verb: name (the Discord app name), details, or state.
    pub status_display: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            buttons: Vec::new(),
            log_path: None,
            channel: "LIVE".into(),
            discord_app_id: DEFAULT_APP_ID.into(),
            poll_seconds: 2,
            update_seconds: 15,
            location_ttl_seconds: 900,
            show_profile: true,
            show_contracts: true,
            show_notifications: true,
            show_location: true,
            show_ship: true,
            show_party: true,
            show_artwork: true,
            custom_state: String::new(),
            custom_details: String::new(),
            locations_file: None,
            image_assets: Default::default(),
            ship_images: Default::default(),
            mission_images: Default::default(),
            activity_type: "playing".into(),
            status_display: "name".into(),
        }
    }
}

impl Config {
    pub fn load(args: &Args) -> Result<Self> {
        let implicit = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|parent| parent.join("scrp.toml")))
            .filter(|p| p.is_file())
            .or_else(|| {
                Path::new("scrp.toml")
                    .is_file()
                    .then(|| PathBuf::from("scrp.toml"))
            });
        let path = args.config.as_ref().or(implicit.as_ref());
        let mut config: Self = if let Some(path) = path {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("Cannot read settings: {}", path.display()))?;
            let mut config: Self = toml::from_str(&text).context("Invalid settings")?;
            let base = path.parent().unwrap_or(Path::new("."));
            for relative in [&mut config.log_path, &mut config.locations_file]
                .into_iter()
                .flatten()
            {
                if relative.is_relative() {
                    *relative = base.join(&*relative);
                }
            }
            config
        } else {
            Self::default()
        };
        if let Some(log) = &args.log {
            config.log_path = Some(log.clone());
        }
        if let Some(dir) = &args.game_dir {
            config.log_path = Some(dir.join("Game.log"));
        }
        if let Some(channel) = &args.channel {
            config.channel = channel.clone();
        }
        if let Some(app) = &args.app_id {
            config.discord_app_id = app.clone();
        }
        config.channel = config.channel.to_uppercase();
        config.activity_type = config.activity_type.trim().to_ascii_lowercase();
        config.status_display = config.status_display.trim().to_ascii_lowercase();
        config.show_profile &= !args.no_profile;
        config.show_contracts &= !args.no_contracts;
        config.show_notifications &= !args.no_notifications;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.buttons.len() <= 2,
            "Discord supports up to two buttons"
        );
        for (index, button) in self.buttons.iter().enumerate() {
            let number = index + 1;
            ensure!(
                !button.label.trim().is_empty()
                    && button.label.len() <= 32
                    && !button.label.chars().any(char::is_control),
                "Button {number} needs a label of 1–32 bytes"
            );
            let host = button
                .url
                .strip_prefix("https://")
                .and_then(|rest| rest.split(['/', '?', '#']).next());
            ensure!(
                button.url.len() <= 512
                    && !button.url.contains(['\\', '@'])
                    && !button
                        .url
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control())
                    && host.is_some_and(|host| !host.is_empty()
                        && host.contains('.')
                        && host
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'))),
                "Button {number} needs a public HTTPS URL of up to 512 bytes"
            );
        }
        ensure!(
            matches!(
                self.channel.as_str(),
                "LIVE" | "PTU" | "EPTU" | "TECH-PREVIEW"
            ),
            "Channel must be LIVE, PTU, EPTU, or TECH-PREVIEW"
        );
        ensure!(
            matches!(
                self.activity_type.as_str(),
                "playing" | "listening" | "watching" | "competing"
            ),
            "activity_type must be playing, listening, watching, or competing"
        );
        ensure!(
            matches!(self.status_display.as_str(), "name" | "details" | "state"),
            "status_display must be name, details, or state"
        );
        for (key, value) in self.ship_images.iter().chain(&self.mission_images) {
            ensure!(
                value.len() <= 256 && !value.chars().any(char::is_control),
                "Image for {key:?} must be a choice number, HTTPS URL, or asset key of up to 256 bytes"
            );
            ensure!(
                !value.contains("://") || value.starts_with("https://"),
                "Image URL for {key:?} must use HTTPS"
            );
        }
        ensure!(
            (1..=60).contains(&self.poll_seconds),
            "poll_seconds must be between 1 and 60"
        );
        ensure!(
            (15..=300).contains(&self.update_seconds),
            "update_seconds must be between 15 and 300"
        );
        ensure!(
            (30..=86400).contains(&self.location_ttl_seconds),
            "location_ttl_seconds must be between 30 and 86400"
        );
        ensure!(
            (17..=20).contains(&self.discord_app_id.len())
                && self.discord_app_id.bytes().all(|b| b.is_ascii_digit()),
            "Discord application ID must contain 17–20 digits"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_are_validated_and_unknown_fields_rejected() {
        Config::default().validate().unwrap();
        assert!(
            Config {
                update_seconds: 1,
                ..Config::default()
            }
            .validate()
            .is_err()
        );
        assert!(toml::from_str::<Config>("typo = true").is_err());
        assert!(Args::try_parse_from(["scrp", "--log", "a", "--game-dir", "b"]).is_err());
        assert!(Args::try_parse_from(["scrp", "--check-discord", "--dry-run"]).is_err());
        assert!(Args::try_parse_from(["scrp", "--check-discord", "--replay", "a"]).is_err());
    }
}
