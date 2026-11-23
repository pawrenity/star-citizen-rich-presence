use chrono::DateTime;
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Login(String),
    Mode(String),
    Loading,
    LoadingScreenOpened,
    LoadingScreenClosed {
        menu: bool,
    },
    Ready,
    Disconnect,
    Shutdown,
    Inventory {
        player: String,
        location: String,
    },
    InventoryTransition {
        player: String,
        location_id: u64,
    },
    InventoryTerminal {
        player: String,
        location_id: u64,
    },
    AtcContact {
        player: String,
        location: Option<String>,
    },
    QuantumTarget {
        vehicle: String,
        destination: String,
    },
    RouteOrigin {
        vehicle: String,
        destination: String,
        origin: String,
    },
    QuantumArrival {
        vehicle: String,
    },
    Board(String),
    Disembark(String),
    ContractAccepted(String),
    ContractEnded(String),
    Incapacitated,
    Spawned,
    Notice {
        details: String,
        state: String,
    },
    /// Someone (possibly the player) joined the player's party.
    PartyJoined(String),
    PartyLeft(String),
    /// The player is in a party (leader change, party launch) without a roster change.
    PartyActive(Option<String>),
    PartyEnded,
}

struct Patterns {
    login: Regex,
    mode: Regex,
    inventory: Regex,
    inventory_transition: Regex,
    show_inventory: Regex,
    inventory_terminal: Regex,
    atc: Regex,
    target: Regex,
    route_origin: Regex,
    vehicle: Regex,
    notification: Regex,
    markup: Regex,
    party: Regex,
    open_markup: Regex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| Patterns {
        login: Regex::new(r"User Login Success - Handle\[([^\]]+)\]").unwrap(),
        mode: Regex::new(r"GameMode\[([^\]]+)\]").unwrap(),
        inventory: Regex::new(r"Player\[([^\]]+)\] requested inventory for Location\[([^\]]+)\]")
            .unwrap(),
        inventory_transition: Regex::new(r"Player \[([^\]]+)\] is changing location\. Landing \[\d+\] -> \[\d+\]\. Location \[\d+\] -> \[(\d+)\]\. Pending \[0\]").unwrap(),
        show_inventory: Regex::new(r"Player \[([^\]]+)\] location inventory not cached\. Request Inventory location \[([^\]]+)\]").unwrap(),
        inventory_terminal: Regex::new(r"Querying location inventory for Player \[([^\]]+)\] at location \[(\d+)\]").unwrap(),
        atc: Regex::new(r"for ([A-Za-z0-9_-]+) \[\d+\] to track their communication partner AImodule_ATC_(?:([A-Za-z0-9_]+?)ATC\d+_)?\d+ \[\d+\]").unwrap(),
        target: Regex::new(r"Player has selected point (\S+) as their destination").unwrap(),
        route_origin: Regex::new(r"\|CSCItemNavigation::CalculateRoute\|Projected Start Location is (.{1,128}?) for route to destination (\S+)").unwrap(),
        vehicle: Regex::new(r"([A-Za-z0-9_]+\[\d+\])\|CSCItemNavigation::").unwrap(),
        // Some HUD notifications continue on the next line. Their first line is enough here.
        notification: Regex::new(r#"Added notification "([^"\r\n]*)"#).unwrap(),
        // Only known UI/rich-text tag names count as markup, so text such as `a < b > c`
        // or `<unknown>` placeholders survives untouched.
        // Party notifications put the member on a second, separately timestamped line.
        party: Regex::new(
            r#"^(?:<[^>]+> )?(?:([A-Za-z0-9_-]{1,64}) (has joined the party|has left the party|is now party leader)|Initiated by party leader ([A-Za-z0-9_-]{1,64}))\.: " \[\d+\] to queue"#,
        )
        .unwrap(),
        markup: Regex::new(&format!(r"(?i)</?({MARKUP_TAGS})(?:[ \t\r\n\x0C=][^<>]*)?/?>")).unwrap(),
        // Quoted attributes can cut a notification capture off in the middle of a tag.
        open_markup: Regex::new(&format!(r"(?i)</?(?:{MARKUP_TAGS})(?:[ \t\r\n\x0C=][^<>]*)?$")).unwrap(),
    })
}

const MARKUP_TAGS: &str =
    r"em\d?|b|i|u|s|strong|sub|sup|br|p|hr|font|color|colour|size|span|img|h[1-6]";

/// Strips Star Citizen / CryEngine UI markup (e.g. `<EM4>[BP]</EM4>`) while keeping the
/// inner text, and collapses whitespace.
pub fn strip_markup(text: &str) -> String {
    if !text.contains('<') {
        return text.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    let p = patterns();
    let text = p.markup.replace_all(text, |tag: &regex::Captures| {
        // Line-breaking tags separate words; inline tags must not split them.
        let name = &tag[1];
        if ["br", "p", "hr"]
            .iter()
            .any(|b| name.eq_ignore_ascii_case(b))
        {
            " "
        } else {
            ""
        }
    });
    let text = p.open_markup.replace(&text, "");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn timestamp(line: &str) -> Option<i64> {
    let time = line.strip_prefix('<')?.split_once('>')?.0;
    DateTime::parse_from_rfc3339(time)
        .ok()
        .map(|t| t.timestamp())
}

/// Only explicit client events are interpreted. Bare mentions of planets/ships are ignored.
pub fn parse(line: &str) -> Option<Event> {
    let p = patterns();
    let message = line
        .split_once("> ")
        .filter(|(prefix, _)| prefix.starts_with('<') && prefix.ends_with('Z'))
        .map_or(line, |(_, message)| message);
    if message == "[CGlobalGameUI::OpenLoadingScreen] Started" {
        return Some(Event::LoadingScreenOpened);
    }
    if let Some((scene, _)) = message
        .strip_prefix("Loading screen for ")
        .and_then(|message| message.split_once(" closed after "))
    {
        let (map, _) = scene.split_once(" : ")?;
        return Some(Event::LoadingScreenClosed {
            menu: map == "Frontend_Main",
        });
    }
    // Cheap guard: a failed `captures` still allocates, and this runs on every line.
    if line.contains("User Login Success")
        && let Some(m) = p.login.captures(line)
    {
        let handle = m[1].trim();
        if !handle.is_empty() {
            return Some(Event::Login(handle.into()));
        }
    }
    if line.contains("System Fast Shutdown") || line.contains("[CIG] CCIGBroker::FastShutdown") {
        return Some(Event::Shutdown);
    }
    if line.contains("[CSessionManager::OnClientSpawned] Spawned!") {
        return Some(Event::Spawned);
    }
    if line.contains("<Channel Disconnected>") && line.contains("hostType=\"GameClient\"") {
        // Local frontend teardown happens while JOINING a server, not returning to menu.
        if line.contains("gamerules=\"SC_Frontend\"") {
            return Some(Event::Loading);
        }
        return Some(Event::Disconnect);
    }
    if line.contains("SeedGameRulesAndMode Success>") {
        return p.mode.captures(line).map(|m| Event::Mode(m[1].into()));
    }
    if line.contains("Loading level ") {
        return Some(Event::Loading);
    }
    if line.contains("<[GameRules] GameRulesActionEvent_GameModeCreated>") {
        return Some(Event::Ready);
    }
    if line.contains("<RequestLocationInventory>") {
        return p.inventory.captures(line).map(|m| Event::Inventory {
            player: m[1].into(),
            location: m[2].into(),
        });
    }
    if line.contains("<Update Inventory Location>") {
        let m = p.inventory_transition.captures(line)?;
        return Some(Event::InventoryTransition {
            player: m[1].into(),
            location_id: m[2].parse().ok()?,
        });
    }
    if line.contains("<Show Location Inventory>") {
        return p.show_inventory.captures(line).map(|m| Event::Inventory {
            player: m[1].into(),
            location: m[2].into(),
        });
    }
    if line.contains("<OnRequestFetchVehicles>") {
        let m = p.inventory_terminal.captures(line)?;
        return Some(Event::InventoryTerminal {
            player: m[1].into(),
            location_id: m[2].parse().ok()?,
        });
    }
    if line.contains("<Connection Flow> CSCCommsComponent::DoEstablishCommunicationCommon: Update bubble created") {
        return p.atc.captures(line).map(|m| Event::AtcContact {
            player: m[1].into(),
            location: m.get(2).map(|name| name.as_str().to_owned()),
        });
    }
    if line.contains("<Player Selected Quantum Target - Local>") {
        let vehicle = p.vehicle.captures(line)?;
        let target = p.target.captures(line)?;
        return Some(Event::QuantumTarget {
            vehicle: vehicle[1].into(),
            destination: target[1].into(),
        });
    }
    if line.contains("<Calculate Route>") {
        let vehicle = p.vehicle.captures(line)?;
        let route = p.route_origin.captures(line)?;
        return Some(Event::RouteOrigin {
            vehicle: vehicle[1].into(),
            origin: route[1].into(),
            destination: route[2].into(),
        });
    }
    if line.contains("<Quantum Drive Arrived - Arrived at Final Destination>") {
        return p.vehicle.captures(line).map(|m| Event::QuantumArrival {
            vehicle: m[1].into(),
        });
    }
    if line.contains("party")
        && line.contains("to queue")
        && let Some(m) = p.party.captures(line)
    {
        return Some(match (m.get(1), m.get(2).map(|v| v.as_str()), m.get(3)) {
            (Some(name), Some("has joined the party"), _) => {
                Event::PartyJoined(name.as_str().into())
            }
            (Some(name), Some("has left the party"), _) => Event::PartyLeft(name.as_str().into()),
            (Some(name), _, _) | (None, _, Some(name)) => {
                Event::PartyActive(Some(name.as_str().into()))
            }
            _ => Event::PartyActive(None),
        });
    }
    if !line.contains("<SHUDEvent_OnNotification>") {
        return None;
    }
    let message = p.notification.captures(line)?;
    // Mission titles carry UI markup such as `<EM4>[BP]</EM4>`; clean it once so
    // accepted/ended contract names still match and every view gets plain text.
    let message = strip_markup(&message[1]);
    let message = message.as_str();
    if let Some(channel) = message
        .strip_prefix("You have joined channel '")
        .and_then(|s| s.split_once('\'').map(|(s, _)| s))
    {
        return ship_name(channel).map(Event::Board);
    }
    if let Some(channel) = message
        .strip_prefix("You have left the channel '")
        .and_then(|s| s.split_once('\'').map(|(s, _)| s))
    {
        return ship_name(channel).map(Event::Disembark);
    }
    if let Some(name) = message.strip_prefix("Contract Accepted:") {
        return nonempty_contract(name).map(Event::ContractAccepted);
    }
    for prefix in [
        "Contract Complete:",
        "Contract Failed:",
        "Contract Abandoned:",
    ] {
        if let Some(name) = message.strip_prefix(prefix) {
            return nonempty_contract(name).map(Event::ContractEnded);
        }
    }
    if message == "Party Disbanded" || message.starts_with("You have left the party") {
        return Some(Event::PartyEnded);
    }
    if message.starts_with("Incapacitated:") {
        return Some(Event::Incapacitated);
    }
    if let Some(name) = message
        .strip_prefix("Received Blueprint:")
        .and_then(nonempty_contract)
    {
        return Some(Event::Notice {
            details: "Blueprint acquired".into(),
            state: name,
        });
    }
    if let Some(location) = message
        .strip_prefix("A Refinery Work Order has been Completed at ")
        .and_then(nonempty_contract)
    {
        // This can be a remote refinery. Never treat it as the player's current location.
        return Some(Event::Notice {
            details: "Refinery order completed".into(),
            state: location,
        });
    }
    if message.starts_with("Hangar Request Completed:") {
        return Some(Event::Notice {
            details: "Hangar ready".into(),
            state: String::new(),
        });
    }
    None
}

fn ship_name(channel: &str) -> Option<String> {
    // Vehicle channels include an owner. Party/global channel joins must not become ships.
    let (ship, _) = channel.rsplit_once(" : ")?;
    let ship = ship
        .trim()
        .strip_prefix("@vehicle_Name")
        .unwrap_or(ship.trim());
    (!ship.is_empty()).then(|| crate::vehicles::display_name(ship))
}

fn nonempty_contract(name: &str) -> Option<String> {
    let name = name.trim().trim_end_matches(':').trim();
    (!name.is_empty()).then(|| name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newer_vehicle_names_and_local_notices_are_parsed_precisely() {
        assert_eq!(
            parse(
                "<SHUDEvent_OnNotification> Added notification \"You have joined channel '@vehicle_NameRSI_Apollo_Medivac : TestPilot'."
            ),
            Some(Event::Board("RSI Apollo Medivac".into()))
        );
        assert_eq!(
            parse(
                "<SHUDEvent_OnNotification> Added notification \"Received Blueprint: Geist Armor Core Desert: "
            ),
            Some(Event::Notice {
                details: "Blueprint acquired".into(),
                state: "Geist Armor Core Desert".into()
            })
        );
        assert_eq!(
            parse(
                "<SHUDEvent_OnNotification> Added notification \"A Refinery Work Order has been Completed at CRU-L1 Ambitious Dream Station: "
            ),
            Some(Event::Notice {
                details: "Refinery order completed".into(),
                state: "CRU-L1 Ambitious Dream Station".into()
            })
        );
        assert_eq!(
            parse("[CSessionManager::OnClientSpawned] Spawned!"),
            Some(Event::Spawned)
        );
        assert_eq!(
            parse("A mission description about Received Blueprint: Mining"),
            None
        );
    }
    #[test]
    fn strips_ui_markup_but_keeps_inner_and_plain_text() {
        let cases = [
            (
                "Tactical Strike Group Needed <EM4>[BP]</EM4> <EM4>[300 Rep]</EM4>",
                "Tactical Strike Group Needed [BP] [300 Rep]",
            ),
            ("<em4>[BP]</Em4>  <EM1>x</EM1>", "[BP] x"),
            ("<b><i>Bold</i> <u>under</u></b>", "Bold under"),
            ("<font color=\"#ff0000\">Red</font> text", "Red text"),
            ("<color=#00ff00>Green</color>", "Green"),
            (
                "<em class='x'>Nested <EM2><b>deep</b></EM2></em>",
                "Nested deep",
            ),
            (
                "Line one<br/>Line two<BR>three<br />four",
                "Line one Line two three four",
            ),
            ("  spaced \t  out\n ", "spaced out"),
            ("Bounty <EM4>[BP]</EM4> <font color=", "Bounty [BP]"),
            // Not markup: comparisons, unknown placeholders, unmatched brackets.
            ("a < b > c", "a < b > c"),
            ("1<2 and 3>2", "1<2 and 3>2"),
            ("Reward <unknown> <>", "Reward <unknown> <>"),
            ("<bold>not a tag</bold>", "<bold>not a tag</bold>"),
            ("dangling < and >", "dangling < and >"),
            ("", ""),
        ];
        for (input, expected) in cases {
            assert_eq!(strip_markup(input), expected, "{input}");
        }
    }
    #[test]
    fn contract_titles_are_cleaned_and_still_match_on_completion() {
        assert_eq!(
            parse(
                "<SHUDEvent_OnNotification> Added notification \"Contract Accepted: Tactical Strike Group Needed <EM4>[BP]</EM4> <EM4>[300 Rep]</EM4>: "
            ),
            Some(Event::ContractAccepted(
                "Tactical Strike Group Needed [BP] [300 Rep]".into()
            ))
        );
        assert_eq!(
            parse(
                "<SHUDEvent_OnNotification> Added notification \"Contract Complete: Tactical Strike Group Needed <EM4>[BP]</EM4>  <EM4>[300 Rep]</EM4>: "
            ),
            Some(Event::ContractEnded(
                "Tactical Strike Group Needed [BP] [300 Rep]".into()
            ))
        );
        assert_eq!(
            parse(
                "<SHUDEvent_OnNotification> Added notification \"Received Blueprint: <EM4>Geist Armor</EM4>: "
            ),
            Some(Event::Notice {
                details: "Blueprint acquired".into(),
                state: "Geist Armor".into()
            })
        );
    }
    #[test]
    fn party_notifications_track_membership_from_their_second_line() {
        let q = "\" [51] to queue. New queue size: 1, MissionId: [00000000-0000-0000-0000-000000000000], ObjectiveId: [] [Team_CoreGameplayFeatures][Missions][Comms]";
        assert_eq!(
            parse(&format!(
                "<2026-11-23T20:30:13.632Z> CJBurny has joined the party.: {q}"
            )),
            Some(Event::PartyJoined("CJBurny".into()))
        );
        assert_eq!(
            parse(&format!(
                "<2026-11-23T20:16:49.639Z> Lord_Saxon has left the party.: {q}"
            )),
            Some(Event::PartyLeft("Lord_Saxon".into()))
        );
        assert_eq!(
            parse(&format!(
                "<2026-10-26T20:17:32.111Z> Commander_Dante is now party leader.: {q}"
            )),
            Some(Event::PartyActive(Some("Commander_Dante".into())))
        );
        assert_eq!(
            parse(&format!(
                "<2026-10-26T20:10:43.487Z> Initiated by party leader NickSpaceKnight.: {q}"
            )),
            Some(Event::PartyActive(Some("NickSpaceKnight".into())))
        );
        assert_eq!(
            parse(
                "<2026-11-22T21:09:03.813Z> [Notice] <SHUDEvent_OnNotification> Added notification \"Party Disbanded"
            ),
            Some(Event::PartyEnded)
        );
        assert_eq!(
            parse(
                "<2026-10-26T20:24:28.575Z> [Notice] <SHUDEvent_OnNotification> Added notification \"You have left the party."
            ),
            Some(Event::PartyEnded)
        );
        // The same text repeats in queue updates; only the "to queue" line counts.
        assert_eq!(
            parse(
                "<2026-11-23T20:30:13.634Z> CJBurny has joined the party.: \" [51], Action: Next [Team_CoreGameplayFeatures][Missions][Comms]"
            ),
            None
        );
        assert_eq!(
            parse("<2026-11-23T20:30:13.632Z> CJBurny has joined the party.: \" [51]"),
            None
        );
        // Incapacitation tips mention parties but are not party events.
        assert_eq!(
            parse(
                "<SHUDEvent_OnNotification> Added notification \"Incapacitated: While incapacitated, ask others in your party, in chat, or through rescue service beacons to revive you before the 'Time to Death' timer expires.\" [40] to queue."
            ),
            Some(Event::Incapacitated)
        );
    }
    #[test]
    fn ignores_world_noise_and_remote_clients() {
        assert_eq!(parse("Streaming entity Stanton3_Area18"), None);
        assert_eq!(parse("<Channel Disconnected> hostType=\"Replicant\""), None);
        assert_eq!(
            parse(
                "<SHUDEvent_OnNotification> Added notification \"You have joined channel 'Global'.\""
            ),
            None
        );
        assert_eq!(parse("<RequestLocationInventory> malformed"), None);
    }
    #[test]
    fn parses_real_410_shapes_with_redacted_identity() {
        assert_eq!(
            parse(
                "<RequestLocationInventory> Player[TestPilot] requested inventory for Location[RR_HUR_LEO] [Team_CoreGameplayFeatures][Inventory]"
            ),
            Some(Event::Inventory {
                player: "TestPilot".into(),
                location: "RR_HUR_LEO".into()
            })
        );
        assert_eq!(
            parse(
                "<SHUDEvent_OnNotification> Added notification \"Contract Accepted:  Combat Gauntlet - Scenario #5: "
            ),
            Some(Event::ContractAccepted(
                "Combat Gauntlet - Scenario #5".into()
            ))
        );
        assert_eq!(
            parse(
                "<SHUDEvent_OnNotification> Added notification \"You have joined channel 'Anvil F7C-M Super Hornet Mk II : TestPilot'."
            ),
            Some(Event::Board("Anvil F7C-M Super Hornet Mk II".into()))
        );
        assert_eq!(
            parse("<Channel Disconnected> gamerules=\"SC_Frontend\" hostType=\"GameClient\""),
            Some(Event::Loading)
        );
    }
    #[test]
    fn timestamps_are_optional_and_checked() {
        assert_eq!(
            timestamp("<2026-10-14T16:17:22.616Z> FileVersion: 4.10.197.2878"),
            Some(1791994642)
        );
        assert_eq!(timestamp("<not-a-date>"), None);
    }
}
