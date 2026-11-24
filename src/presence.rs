use crate::{
    config::Config,
    session::{Phase, Session},
};
use anyhow::Result;
use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    /// Optional custom game name; empty keeps the Discord application's default name.
    pub name: String,
    pub details: String,
    pub state: String,
    pub large_image: String,
    pub large_text: String,
    /// Empty means the default badge (the logo, when the large image is artwork).
    pub small_image: String,
    pub small_text: String,
    /// Discord activity type: 0 playing, 2 listening, 3 watching, 5 competing.
    pub activity_type: u8,
    /// Discord status_display_type: 0 app name, 1 state, 2 details.
    pub status_display: u8,
    pub profile_url: Option<String>,
    pub buttons: Vec<crate::config::CardButton>,
    pub started: i64,
}

pub fn snapshot(session: &Session, config: &Config, now: i64) -> Option<Snapshot> {
    let (details, mut state) = match session.phase {
        Phase::Stopped => return None,
        Phase::Starting => ("Starting Star Citizen".into(), config.channel.clone()),
        Phase::Menu => ("In the main menu".into(), config.channel.clone()),
        Phase::Loading => ("Loading into the verse".into(), config.channel.clone()),
        Phase::Playing => ("Exploring the verse".into(), config.channel.clone()),
    };
    let mut snapshot = Snapshot {
        name: custom_name(config),
        details,
        state: String::new(),
        large_image: "logo".into(),
        large_text: "Star Citizen".into(),
        small_image: String::new(),
        small_text: String::new(),
        activity_type: match config.activity_type.as_str() {
            "listening" => 2,
            "watching" => 3,
            "competing" => 5,
            _ => 0,
        },
        status_display: match config.status_display.as_str() {
            "state" => 1,
            "details" => 2,
            _ => 0,
        },
        profile_url: None,
        buttons: config.buttons.clone(),
        started: session.started,
    };
    if session.phase == Phase::Playing {
        let fresh = |(_, seen): &&(crate::locations::Location, i64)| {
            (0..=config.location_ttl_seconds as i64).contains(&(now - seen))
        };
        if config.show_location
            && let Some((location, _)) = session
                .location
                .as_ref()
                .filter(fresh)
                .or_else(|| session.route_origin.as_ref().filter(fresh))
        {
            snapshot.details = location.name.clone();
            state = format!("{} · {}", location.system, config.channel);
            snapshot.large_text = location.name.clone();
            if config.show_artwork {
                snapshot.large_image = location_art(location, config).into();
            }
        }
        if config.show_contracts {
            if session.contracts.len() == 1 {
                state = session.contracts.first().unwrap().clone();
            } else if session.contracts.len() > 1 {
                state = format!("{} active contracts", session.contracts.len());
            }
        }
        if config.show_notifications
            && let Some((details, secondary, at)) = &session.notice
            && (0..=60).contains(&(now - at))
        {
            if details == "Hangar ready" {
                if session
                    .last_atc
                    .is_some_and(|contact| contact.abs_diff(*at) <= 60)
                    && session.location.as_ref().is_some_and(|(_, seen)| {
                        (0..=config.location_ttl_seconds as i64).contains(&(now - seen))
                    })
                {
                    state = "In hangar".into();
                } else {
                    state.clone_from(details);
                }
            } else {
                snapshot.details.clone_from(details);
                if !secondary.is_empty() {
                    state.clone_from(secondary);
                }
            }
        }
        if session
            .incapacitated_at
            .is_some_and(|at| (0..=120).contains(&(now - at)))
        {
            snapshot.details = "Incapacitated".into();
        }
    }
    if session.phase == Phase::Playing
        && config.show_ship
        && let Some(ship) = &session.ship
    {
        let context = config.channel == state
            || [session.location.as_ref(), session.route_origin.as_ref()]
                .into_iter()
                .flatten()
                .any(|(location, _)| state == format!("{} · {}", location.system, config.channel));
        state = if context {
            ship.clone()
        } else {
            format!("{ship} · {state}")
        };
    }
    if session.phase == Phase::Playing && config.show_party && session.party.is_some() {
        state = format!("{state} · In a party");
    }
    if session.phase == Phase::Playing && config.show_artwork {
        apply_artwork(&mut snapshot, session, config);
    }
    if config.show_profile
        && let Some(handle) = &session.handle
        && !handle.is_empty()
        && handle.len() <= 64
        && handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        snapshot.profile_url = Some(format!(
            "https://robertsspaceindustries.com/citizens/{handle}"
        ));
    }
    snapshot.details = match custom_line(&config.custom_details, session.phase) {
        Some(custom) => custom,
        // Defense in depth: log-derived text must never leak UI markup.
        None => limit(&crate::parser::strip_markup(&snapshot.details), 128),
    };
    snapshot.state = match custom_line(&config.custom_state, session.phase) {
        Some(custom) => custom,
        None => limit(&crate::parser::strip_markup(&state), 128),
    };
    snapshot.large_text = limit(&crate::parser::strip_markup(&snapshot.large_text), 128);
    snapshot.small_text = limit(&crate::parser::strip_markup(&snapshot.small_text), 128);
    Some(snapshot)
}

/// Ship art takes the large slot (pushing location art to the badge); a recognised
/// contract type takes the badge. User overrides win over bundled wiki artwork.
fn apply_artwork(snapshot: &mut Snapshot, session: &Session, config: &Config) {
    let mut badge = None;
    if config.show_ship
        && let Some(ship) = &session.ship
        && let Some(url) = ship_image(ship, config)
    {
        if snapshot.large_image != "logo" {
            badge = Some((
                std::mem::take(&mut snapshot.large_image),
                std::mem::take(&mut snapshot.large_text),
            ));
        }
        snapshot.large_image = url;
        snapshot.large_text = ship.clone();
    }
    if config.show_contracts {
        // Several contracts only get an icon when they all resolve to the same one.
        let mut kinds = session
            .contracts
            .iter()
            .map(|title| mission_image(title, config));
        if let Some(Some(first)) = kinds.next()
            && kinds.all(|kind| kind.as_ref() == Some(&first))
        {
            badge = Some(first);
        }
    }
    if let Some((image, text)) = badge {
        snapshot.small_image = image;
        snapshot.small_text = text;
    }
}

fn find_ci<'a>(
    map: &'a std::collections::BTreeMap<String, String>,
    key: &str,
) -> Option<&'a String> {
    map.get(key).or_else(|| {
        map.iter()
            .find(|(k, _)| k.trim().eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    })
}

fn ship_image(ship: &str, config: &Config) -> Option<String> {
    let choices = crate::artwork::ship_choices(ship);
    match find_ci(&config.ship_images, ship).or_else(|| config.image_assets.get(ship)) {
        Some(value) => crate::artwork::resolve(value, choices),
        None => choices.first().cloned(),
    }
}

/// Override keys may be words from the contract title (most specific) or a mission
/// type key/label such as `bounty` or `Bounty hunting`.
fn mission_image(title: &str, config: &Config) -> Option<(String, String)> {
    let category = crate::artwork::mission_category(title);
    let choices = category.map_or(&[][..], |(key, _)| crate::artwork::mission_choices(key));
    let label = category.map_or("Contract", |(_, label)| label).to_string();
    if config.mission_images.is_empty() {
        return Some((choices.first()?.clone(), label));
    }
    let lower = title.to_ascii_lowercase();
    let by_title = config
        .mission_images
        .iter()
        .filter(|(key, _)| {
            let key = key.trim().to_ascii_lowercase();
            !key.is_empty() && lower.contains(&key)
        })
        .max_by_key(|(key, _)| key.trim().len())
        .map(|(_, value)| value);
    let by_type = category.and_then(|(key, label)| {
        find_ci(&config.mission_images, key).or_else(|| find_ci(&config.mission_images, label))
    });
    match by_title.or(by_type) {
        Some(value) => crate::artwork::resolve(value, choices).map(|url| (url, label)),
        None => Some((choices.first()?.clone(), label)),
    }
}

fn location_art<'a>(location: &crate::locations::Location, config: &'a Config) -> &'a str {
    if let Some(asset) = config.image_assets.get(&location.name) {
        return asset;
    }
    static ARTWORK: std::sync::OnceLock<std::collections::HashMap<String, String>> =
        std::sync::OnceLock::new();
    let artwork = ARTWORK.get_or_init(|| {
        serde_json::from_str(include_str!("../data/location-artwork.json"))
            .expect("bundled location artwork must be valid JSON")
    });
    if let Some(url) = artwork.get(&format!("{}:{}", location.system, location.name)) {
        return url;
    }
    if location.system == "Stanton" {
        // Preserve parent artwork for unmapped POIs, preferring moons over planets.
        const PARENTS: &[&str] = &[
            "Grim HEX",
            "Area18",
            "Daymar",
            "Yela",
            "Cellin",
            "Crusader",
            "Hurston",
            "microTech",
            "ArcCorp",
        ];
        for name in PARENTS {
            if location
                .scopes
                .iter()
                .any(|scope| scope.eq_ignore_ascii_case(name))
                && let Some(url) = artwork.get(&format!("Stanton:{name}"))
            {
                return url;
            }
        }
    }
    "logo"
}

fn limit(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.into();
    }
    let mut end = max - 3;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// Discord needs at least 2 characters; shorter custom names fall back to the app default.
fn custom_name(config: &Config) -> String {
    let name = crate::parser::strip_markup(&config.custom_name);
    if name.chars().count() < 2 {
        String::new()
    } else {
        limit(&name, 128)
    }
}

/// Custom details/state apply only while playing, and only when they are at least 2 characters.
fn custom_line(text: &str, phase: Phase) -> Option<String> {
    let trimmed = text.trim();
    (phase == Phase::Playing && trimmed.chars().count() >= 2).then(|| limit(trimmed, 128))
}

pub trait Transport {
    fn connect(&mut self) -> Result<()>;
    fn publish(&mut self, snapshot: &Snapshot) -> Result<()>;
    fn clear(&mut self) -> Result<()>;
    fn close(&mut self);
}

/// Remember only successful sends. Reconnects always republish, even if state is unchanged.
pub struct Publisher<T: Transport> {
    transport: T,
    connected: bool,
    last: Option<Snapshot>,
    sent_at: Option<Instant>,
    retry_at: Instant,
    interval: Duration,
}

impl<T: Transport> Publisher<T> {
    pub fn new(transport: T, interval: Duration, now: Instant) -> Self {
        Self {
            transport,
            connected: false,
            last: None,
            sent_at: None,
            retry_at: now,
            interval,
        }
    }

    pub fn sync(&mut self, desired: Option<&Snapshot>, now: Instant) -> Result<bool> {
        if desired.is_none() {
            if self.connected {
                let result = self.transport.clear();
                self.transport.close();
                self.connected = false;
                self.last = None;
                self.sent_at = None;
                result?;
                return Ok(true);
            }
            return Ok(false);
        }
        if now < self.retry_at {
            return Ok(false);
        }
        if let Some(sent) = self.sent_at {
            if now.duration_since(sent) < self.interval {
                return Ok(false);
            }
            // Once a minute, refresh to detect a closed/restarted Discord pipe.
            if desired == self.last.as_ref() && now.duration_since(sent) < Duration::from_secs(60) {
                return Ok(false);
            }
        }
        let result = (|| {
            if !self.connected {
                self.transport.connect()?;
                self.connected = true;
            }
            self.transport.publish(desired.unwrap())
        })();
        if let Err(error) = result {
            self.transport.close();
            self.connected = false;
            self.last = None;
            self.sent_at = None;
            self.retry_at = now + self.interval;
            return Err(error);
        }
        self.last = desired.cloned();
        self.sent_at = Some(now);
        Ok(true)
    }

    pub fn shutdown(&mut self) {
        if self.connected {
            let _ = self.transport.clear();
            self.transport.close();
        }
        self.connected = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{locations::Locations, parser::Event};
    #[test]
    fn remote_refinery_and_blueprint_notices_expire_without_moving_the_player() {
        let locations = Locations::load(None).unwrap();
        let mut s = Session::new(1);
        s.apply(Event::Login("Pilot".into()), 2, &locations);
        s.apply(
            Event::Inventory {
                player: "Pilot".into(),
                location: "Stanton1_Lorville".into(),
            },
            3,
            &locations,
        );
        s.apply(
            Event::Notice {
                details: "Refinery order completed".into(),
                state: "CRU-L1 Ambitious Dream Station".into(),
            },
            4,
            &locations,
        );
        let fresh = snapshot(&s, &Config::default(), 5).unwrap();
        assert_eq!(fresh.details, "Refinery order completed");
        assert_eq!(s.location.as_ref().unwrap().0.name, "Lorville");
        assert_eq!(
            snapshot(&s, &Config::default(), 65).unwrap().details,
            "Lorville"
        );
        let hidden = Config {
            show_notifications: false,
            ..Config::default()
        };
        assert_eq!(snapshot(&s, &hidden, 5).unwrap().details, "Lorville");
    }
    #[test]
    fn ship_art_leads_and_location_or_mission_art_takes_the_badge() {
        let locations = Locations::load(None).unwrap();
        let mut s = Session::new(1);
        s.apply(Event::Login("Pilot".into()), 2, &locations);
        s.apply(
            Event::Inventory {
                player: "Pilot".into(),
                location: "Stanton1_Lorville".into(),
            },
            3,
            &locations,
        );
        let on_foot = snapshot(&s, &Config::default(), 4).unwrap();
        assert!(on_foot.large_image.starts_with("https://"));
        assert!(on_foot.small_image.is_empty());

        s.apply(Event::Board("Aegis Gladius".into()), 4, &locations);
        let flying = snapshot(&s, &Config::default(), 5).unwrap();
        assert_eq!(
            flying.large_image,
            crate::artwork::ship("Aegis Gladius").unwrap()
        );
        assert_eq!(flying.large_text, "Aegis Gladius");
        assert_eq!(flying.small_image, on_foot.large_image);
        assert_eq!(flying.small_text, "Lorville");

        s.apply(
            Event::ContractAccepted("VHRT Bounty: Kill Target".into()),
            5,
            &locations,
        );
        let hunting = snapshot(&s, &Config::default(), 6).unwrap();
        assert_eq!(hunting.small_text, "Bounty hunting");
        assert!(hunting.small_image.contains("BHG_logo"));

        // Mixed contract types fall back to the location badge.
        s.apply(Event::ContractAccepted("Haul Cargo".into()), 6, &locations);
        assert_eq!(
            snapshot(&s, &Config::default(), 7).unwrap().small_text,
            "Lorville"
        );

        let custom = Config {
            image_assets: [("Aegis Gladius".to_string(), "my_gladius".to_string())].into(),
            ..Config::default()
        };
        assert_eq!(snapshot(&s, &custom, 7).unwrap().large_image, "my_gladius");
        let picked = Config {
            ship_images: [("aegis gladius".to_string(), "2".to_string())].into(),
            mission_images: [
                (
                    "bounty".to_string(),
                    "https://example.com/bounty.png".to_string(),
                ),
                (
                    "haul cargo".to_string(),
                    "https://example.com/haul.png".to_string(),
                ),
            ]
            .into(),
            activity_type: "listening".into(),
            status_display: "details".into(),
            ..Config::default()
        };
        let card = snapshot(&s, &picked, 7).unwrap();
        assert_eq!(
            Some(&card.large_image),
            crate::artwork::ship_choices("Aegis Gladius").get(1)
        );
        // The two contracts now resolve to different custom icons, so the location stays.
        assert_eq!(card.small_text, "Lorville");
        assert_eq!((card.activity_type, card.status_display), (2, 2));
        s.apply(Event::ContractEnded("Haul Cargo".into()), 7, &locations);
        let card = snapshot(&s, &picked, 8).unwrap();
        assert_eq!(card.small_image, "https://example.com/bounty.png");
        assert_eq!(card.small_text, "Bounty hunting");
        let plain = Config {
            show_artwork: false,
            ..Config::default()
        };
        let plain = snapshot(&s, &plain, 7).unwrap();
        assert_eq!(
            (plain.large_image.as_str(), plain.small_image.as_str()),
            ("logo", "")
        );
    }
    #[test]
    fn party_membership_shows_until_the_party_ends() {
        let locations = Locations::load(None).unwrap();
        let mut s = Session::new(1);
        s.apply(Event::Login("Pilot".into()), 2, &locations);
        s.apply(Event::Board("Aegis Gladius".into()), 3, &locations);
        assert_eq!(
            snapshot(&s, &Config::default(), 4).unwrap().state,
            "Aegis Gladius"
        );
        s.apply(Event::PartyJoined("Wingman".into()), 4, &locations);
        assert_eq!(
            snapshot(&s, &Config::default(), 5).unwrap().state,
            "Aegis Gladius · In a party"
        );
        let hidden = Config {
            show_party: false,
            ..Config::default()
        };
        assert_eq!(snapshot(&s, &hidden, 5).unwrap().state, "Aegis Gladius");
        // Another member leaving keeps the party; the player leaving ends it.
        s.apply(Event::PartyLeft("Wingman".into()), 5, &locations);
        assert!(s.party.is_some());
        s.apply(Event::PartyLeft("Pilot".into()), 6, &locations);
        assert!(s.party.is_none());
        s.apply(Event::PartyActive(Some("Lead".into())), 7, &locations);
        s.apply(Event::PartyEnded, 8, &locations);
        assert_eq!(
            snapshot(&s, &Config::default(), 9).unwrap().state,
            "Aegis Gladius"
        );
    }
    #[test]
    fn markup_in_session_text_is_stripped_from_the_card() {
        let locations = Locations::load(None).unwrap();
        let mut s = Session::new(1);
        // Events built directly (bypassing the parser) are still cleaned at render time.
        s.apply(
            Event::ContractAccepted("Bounty <EM4>[BP]</EM4> <b>Hunt</b>".into()),
            2,
            &locations,
        );
        s.apply(
            Event::Notice {
                details: "<EM1>Blueprint</EM1> acquired".into(),
                state: String::new(),
            },
            3,
            &locations,
        );
        let card = snapshot(&s, &Config::default(), 4).unwrap();
        assert_eq!(card.details, "Blueprint acquired");
        assert_eq!(card.state, "Bounty [BP] Hunt");
    }
    use anyhow::anyhow;
    #[derive(Default)]
    struct Fake {
        connects: usize,
        sends: usize,
        clears: usize,
        fail: bool,
    }
    impl Transport for Fake {
        fn connect(&mut self) -> Result<()> {
            self.connects += 1;
            Ok(())
        }
        fn publish(&mut self, _: &Snapshot) -> Result<()> {
            self.sends += 1;
            if self.fail {
                Err(anyhow!("pipe closed"))
            } else {
                Ok(())
            }
        }
        fn clear(&mut self) -> Result<()> {
            self.clears += 1;
            Ok(())
        }
        fn close(&mut self) {}
    }
    #[test]
    fn deduplicates_rate_limits_and_republishes_after_failure() {
        let now = Instant::now();
        let mut p = Publisher::new(Fake::default(), Duration::from_secs(15), now);
        let s = snapshot(&Session::new(1), &Config::default(), 1).unwrap();
        assert!(!p.sync(None, now).unwrap());
        assert!(p.sync(Some(&s), now).unwrap());
        assert!(!p.sync(Some(&s), now + Duration::from_secs(20)).unwrap());
        p.transport.fail = true;
        assert!(p.sync(Some(&s), now + Duration::from_secs(60)).is_err());
        p.transport.fail = false;
        assert!(!p.sync(Some(&s), now + Duration::from_secs(65)).unwrap());
        assert!(p.sync(Some(&s), now + Duration::from_secs(75)).unwrap());
        assert_eq!(p.transport.connects, 2);
        assert!(p.sync(None, now + Duration::from_secs(76)).unwrap());
        assert_eq!(p.transport.clears, 1);
    }
    #[test]
    fn stale_location_and_profile_opt_out_work() {
        let mut s = Session::new(1);
        s.apply(
            Event::Login("Pilot".into()),
            2,
            &Locations::load(None).unwrap(),
        );
        s.apply(
            Event::Inventory {
                player: "Pilot".into(),
                location: "Area18".into(),
            },
            3,
            &Locations::load(None).unwrap(),
        );
        assert_eq!(
            snapshot(&s, &Config::default(), 4).unwrap().details,
            "Area18"
        );
        let config = Config {
            show_profile: false,
            ..Config::default()
        };
        let later = snapshot(&s, &config, 1000).unwrap();
        assert_eq!(later.details, "Exploring the verse");
        assert!(later.profile_url.is_none());
        assert!(limit(&"é".repeat(100), 128).len() <= 128);
    }
    #[test]
    fn single_character_custom_text_falls_back_to_automatic_text() {
        let mut s = Session::new(1);
        s.apply(
            Event::Login("Pilot".into()),
            2,
            &Locations::load(None).unwrap(),
        );
        s.apply(
            Event::Mode("Persistent_Universe".into()),
            3,
            &Locations::load(None).unwrap(),
        );
        let config = Config {
            custom_details: " a ".into(),
            custom_state: "é".into(),
            custom_name: "<b>x</b>".into(),
            ..Config::default()
        };
        let card = snapshot(&s, &config, 4).unwrap();
        assert_eq!(card.details, "Exploring the verse");
        assert_eq!(card.state, config.channel);
        assert_eq!(card.name, "");
    }
    #[test]
    fn two_character_custom_text_is_used_trimmed() {
        let mut s = Session::new(1);
        s.apply(
            Event::Login("Pilot".into()),
            2,
            &Locations::load(None).unwrap(),
        );
        s.apply(
            Event::Mode("Persistent_Universe".into()),
            3,
            &Locations::load(None).unwrap(),
        );
        let config = Config {
            custom_details: " Hauling ".into(),
            custom_state: "ok".into(),
            custom_name: "Verse Run".into(),
            ..Config::default()
        };
        let card = snapshot(&s, &config, 4).unwrap();
        assert_eq!(card.details, "Hauling");
        assert_eq!(card.state, "ok");
        assert_eq!(card.name, "Verse Run");
    }
}
