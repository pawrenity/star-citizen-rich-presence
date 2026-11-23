//! Star Citizen Wiki artwork for ships and contract types, bundled as URLs.
//! Each entry lists numbered choices; choice 1 is the default.
//! Regenerate the data with `scripts/refresh-artwork.mjs`.
use std::{collections::HashMap, sync::OnceLock};

type Choices = HashMap<String, Vec<String>>;

fn load(cell: &'static OnceLock<Choices>, json: &str) -> &'static Choices {
    cell.get_or_init(|| serde_json::from_str(json).expect("bundled artwork must be valid JSON"))
}

fn ships() -> &'static Choices {
    static SHIPS: OnceLock<Choices> = OnceLock::new();
    load(&SHIPS, include_str!("../data/ship-artwork.json"))
}

/// Wiki artwork choices for a ship, keyed by its catalogue display name.
pub fn ship_choices(name: &str) -> &'static [String] {
    ships().get(name).map_or(&[], Vec::as_slice)
}

/// Every ship with bundled artwork, alphabetically.
pub fn ship_names() -> Vec<&'static str> {
    let mut names: Vec<_> = ships().keys().map(String::as_str).collect();
    names.sort_unstable();
    names
}

/// Default wiki artwork for a ship.
pub fn ship(name: &str) -> Option<&'static str> {
    ship_choices(name).first().map(String::as_str)
}

/// Wiki artwork choices for a contract category key (see [`mission_category`]).
pub fn mission_choices(key: &str) -> &'static [String] {
    static MISSIONS: OnceLock<Choices> = OnceLock::new();
    load(&MISSIONS, include_str!("../data/mission-artwork.json"))
        .get(key)
        .map_or(&[], Vec::as_slice)
}

/// Contract categories, most specific first. A keyword ending in `*` matches word prefixes.
pub const CATEGORIES: &[(&str, &str, &[&str])] = &[
    ("xenothreat", "XenoThreat", &["xenothreat", "vanduul"]),
    ("wikelo", "Wikelo contract", &["wikelo"]),
    (
        "arena",
        "Arena Commander",
        &["gauntlet", "arena", "scenario"],
    ),
    (
        "racing",
        "Racing",
        &["race*", "racing", "racetrack", "checkpoint*"],
    ),
    (
        "rescue",
        "Search and rescue",
        &[
            "rescu*", "beacon*", "medivac", "medical", "injur*", "stranded", "evacuat*",
        ],
    ),
    (
        "salvage",
        "Salvage",
        &["salvag*", "wreck*", "scrap*", "reclaim*", "derelict*"],
    ),
    (
        "mining",
        "Mining",
        &[
            "mining",
            "miner*",
            "ore",
            "ores",
            "quantanium",
            "prospect*",
            "mineral*",
        ],
    ),
    (
        "bounty",
        "Bounty hunting",
        &[
            "bount*",
            "vlrt",
            "lrt",
            "mrt",
            "hrt",
            "vhrt",
            "ert",
            "wanted",
            "assassinat*",
            "fugitive*",
        ],
    ),
    (
        "delivery",
        "Delivery",
        &[
            "deliver*", "package*", "parcel*", "courier*", "covalex", "dropoff*",
        ],
    ),
    (
        "hauling",
        "Cargo hauling",
        &[
            "haul*",
            "cargo",
            "freight",
            "transport*",
            "supply",
            "supplies",
            "shipment*",
            "logistic*",
            "linehaul",
        ],
    ),
    (
        "investigation",
        "Investigation",
        &[
            "investigat*",
            "missing",
            "recover*",
            "retriev*",
            "search*",
            "evidence",
            "locate",
            "data",
        ],
    ),
    (
        "mercenary",
        "Mercenary contract",
        &[
            "mercenar*",
            "strike",
            "defend*",
            "defen*",
            "ambush*",
            "clear*",
            "patrol*",
            "escort*",
            "assault*",
            "eliminat*",
            "security",
            "siege",
            "hostile*",
            "outlaw*",
            "kill*",
            "combat",
            "attack*",
            "raid*",
            "protect*",
            "guard*",
            "bunker*",
            "occupant*",
            "infiltrat*",
            "sabotag*",
        ],
    ),
];

/// The category key and label a contract title belongs to, if recognisable.
pub fn mission_category(title: &str) -> Option<(&'static str, &'static str)> {
    let words: Vec<String> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    CATEGORIES.iter().find_map(|(key, label, keywords)| {
        keywords
            .iter()
            .any(|keyword| match keyword.strip_suffix('*') {
                Some(prefix) => words.iter().any(|w| w.starts_with(prefix)),
                None => words.iter().any(|w| w == keyword),
            })
            .then_some((*key, *label))
    })
}

/// Default icon URL and label for a contract title.
pub fn mission(title: &str) -> Option<(&'static str, &'static str)> {
    let (key, label) = mission_category(title)?;
    Some((mission_choices(key).first()?.as_str(), label))
}

/// Resolves a user override: a number picks that wiki choice (1-based); anything else
/// (an HTTPS URL or a Discord asset key) is used as given. Out-of-range numbers fall
/// back to the default choice.
pub fn resolve(value: &str, choices: &'static [String]) -> Option<String> {
    let value = value.trim().trim_start_matches('#');
    match value.parse::<usize>() {
        Ok(n) => choices
            .get(n.wrapping_sub(1))
            .or_else(|| choices.first())
            .cloned(),
        Err(_) if value.is_empty() => None,
        Err(_) => Some(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classifies_common_contract_titles() {
        let cases = [
            (
                "Tactical Strike Group Needed [BP] [300 Rep]",
                Some("mercenary"),
            ),
            ("Combat Gauntlet - Scenario #5", Some("arena")),
            ("VHRT Bounty: Kill Target", Some("bounty")),
            ("Bounties Wanted", Some("bounty")),
            ("Covalex Local Delivery Route", Some("delivery")),
            ("Haul Cargo to Seraphim Station", Some("hauling")),
            ("Salvage Wrecked Ship Hulls", Some("salvage")),
            ("Quantanium Mining Request", Some("mining")),
            ("Rescue Stranded Pilot", Some("rescue")),
            ("Investigate Missing Persons", Some("investigation")),
            ("XenoThreat Incursion", Some("xenothreat")),
            ("Wikelo Wants Things", Some("wikelo")),
            ("New Alliance Aid contract", None),
            // Words are matched whole, so "more" is not mining "ore".
            ("More Before Core", None),
        ];
        for (title, expected) in cases {
            assert_eq!(mission_category(title).map(|(k, _)| k), expected, "{title}");
        }
    }
    #[test]
    fn every_category_and_common_ship_has_bundled_https_artwork() {
        for (key, label, keywords) in CATEGORIES {
            let title = keywords[0].trim_end_matches('*');
            assert_eq!(mission_category(title), Some((*key, *label)), "{title}");
            let (url, _) = mission(title).unwrap_or_else(|| panic!("no artwork for {key}"));
            for url in std::iter::once(url).chain(mission_choices(key).iter().map(String::as_str)) {
                assert!(url.starts_with("https://media.starcitizen.tools/"), "{url}");
                assert!(!url.ends_with(".svg"), "{url}");
            }
        }
        for ship_name in [
            "Aegis Gladius",
            "Anvil F7C-M Super Hornet Mk II",
            "RSI Apollo Medivac",
            "RSI Galaxy",
        ] {
            assert!(
                ship(ship_name).is_some_and(|u| u.starts_with("https://")),
                "{ship_name}"
            );
        }
        assert!(ship_choices("Aegis Idris-P").len() > 1);
        // Discord rejects image URLs longer than 256 bytes.
        for (name, urls) in [
            ("ships", include_str!("../data/ship-artwork.json")),
            ("missions", include_str!("../data/mission-artwork.json")),
        ]
        .map(|(n, json)| (n, serde_json::from_str::<Choices>(json).unwrap()))
        {
            for url in urls.values().flatten() {
                assert!(url.len() <= 256, "{name}: {url}");
            }
        }
        assert_eq!(ship("Unmapped New Ship"), None);
    }
    #[test]
    fn overrides_pick_choices_or_pass_through_custom_images() {
        let choices = ship_choices("Aegis Gladius");
        assert_eq!(resolve("2", choices).as_ref(), choices.get(1));
        assert_eq!(resolve("#1", choices).as_ref(), choices.first());
        assert_eq!(resolve("999", choices).as_ref(), choices.first());
        assert_eq!(resolve("0", choices).as_ref(), choices.first());
        assert_eq!(resolve(" my_asset ", choices).as_deref(), Some("my_asset"));
        assert_eq!(
            resolve("https://example.com/a.png", &[]).as_deref(),
            Some("https://example.com/a.png")
        );
        assert_eq!(resolve("", choices), None);
    }
}
