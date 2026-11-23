use star_citizen_rich_presence::{
    config::Config,
    locations::Locations,
    parser, presence,
    session::{Phase, Session},
};

#[test]
fn live_loading_screen_and_ruin_station_use_client_events() {
    let locations = Locations::load(None).unwrap();
    let mut session = Session::new(0);
    for (line, expected) in [
        ("User Login Success - Handle[TestPilot]", Phase::Starting),
        (
            "<2026-11-21T21:01:31.450Z> [CGlobalGameUI::OpenLoadingScreen] Started",
            Phase::Loading,
        ),
        (
            "<[GameRules] GameRulesActionEvent_GameModeCreated>",
            Phase::Loading,
        ),
        (
            "[CSessionManager::OnClientSpawned] Spawned!",
            Phase::Loading,
        ),
        (
            "Loading screen for Pyro : SC_Frontend closed after 36.67 seconds",
            Phase::Playing,
        ),
        (
            "<RequestLocationInventory> Player[TestPilot] requested inventory for Location[RR_P6_LEO]",
            Phase::Playing,
        ),
    ] {
        session.apply(parser::parse(line).unwrap(), 1, &locations);
        assert_eq!(session.phase, expected, "{line}");
    }
    assert_eq!(session.location.as_ref().unwrap().0.name, "Ruin Station");
    assert_eq!(session.location.as_ref().unwrap().0.system, "Pyro");
}

#[test]
fn complete_410_session_tracks_local_events_without_overlay() {
    let locations = Locations::load(None).unwrap();
    let mut session = Session::new(1791993600);
    let mut at = session.started;
    for line in include_str!("fixtures/session-4.10.log").lines() {
        at = parser::timestamp(line).unwrap_or(at);
        if let Some(event) = parser::parse(line) {
            session.apply(event, at, &locations);
        }
    }
    assert_eq!(session.phase, Phase::Playing);
    assert!(session.contracts.is_empty());
    assert!(session.ship.is_none());
    let result = presence::snapshot(&session, &Config::default(), at).unwrap();
    assert_eq!(result.details, "Everus Harbor");
    assert_eq!(result.state, "Stanton · LIVE");
    assert_eq!(
        result.profile_url.as_deref(),
        Some("https://robertsspaceindustries.com/citizens/TestPilot")
    );
    assert_eq!(result.started, 1791993600);
}

#[test]
fn executable_replay_prints_json_and_respects_privacy() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_star-citizen-rich-presence"))
        .args([
            "--config",
            "scrp.example.toml",
            "--replay",
            "tests/fixtures/session-4.10.log",
            "--no-profile",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["phase"], "Playing");
    assert_eq!(result["presence"]["details"], "Everus Harbor");
    assert!(result["presence"]["profile_url"].is_null());
}

#[test]
fn mission_title_markup_never_reaches_the_presence_card() {
    let locations = Locations::load(None).unwrap();
    let mut session = Session::new(0);
    for line in [
        "User Login Success - Handle[TestPilot]",
        "<2026-11-23T20:00:00.000Z> [Notice] <SHUDEvent_OnNotification> Added notification \"You have joined channel 'Anvil Asgard : TestPilot'.",
        "<2026-11-23T20:00:01.000Z> [Notice] <SHUDEvent_OnNotification> Added notification \"Contract Accepted: Tactical Strike Group Needed <EM4>[BP]</EM4> <EM4>[300 Rep]</EM4>: ",
    ] {
        session.apply(parser::parse(line).unwrap(), 1, &locations);
    }
    let card = presence::snapshot(&session, &Config::default(), 1).unwrap();
    assert_eq!(
        card.state,
        "Anvil Asgard · Tactical Strike Group Needed [BP] [300 Rep]"
    );
    assert!(!card.state.contains('<') && !card.details.contains('<'));

    session.apply(
        parser::parse("<SHUDEvent_OnNotification> Added notification \"Contract Complete: Tactical Strike Group Needed <em4>[BP]</em4> <EM4>[300 Rep]</EM4>: ").unwrap(),
        2,
        &locations,
    );
    assert!(session.contracts.is_empty());
}
