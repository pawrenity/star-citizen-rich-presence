use star_citizen_rich_presence::{
    config::Config, locations::Locations, parser, presence, session::Session,
};

#[test]
fn stanton_crossing_uses_the_gateway_on_the_observed_side() {
    let locations = Locations::load(None).unwrap();
    let mut session = Session::new(1);
    for (at, line) in [
        (2, "User Login Success - Handle[TestPilot]"),
        (
            3,
            "<RequestLocationInventory> Player[TestPilot] requested inventory for Location[RR_JP_PyroStanton]",
        ),
        (
            12,
            "<RequestLocationInventory> Player[TestPilot] requested inventory for Location[RR_JP_StantonPyro]",
        ),
        (
            15,
            "<Player Selected Quantum Target - Local> RSI_Galaxy_123[123]|CSCItemNavigation::OnPlayerSelectedQuantumTarget|Player has selected point ObjectContainer_Lorville_City as their destination",
        ),
        (
            15,
            "<Calculate Route> RSI_Galaxy_123[123]|CSCItemNavigation::CalculateRoute|Projected Start Location is Pyro Gateway for route to destination ObjectContainer_Lorville_City",
        ),
    ] {
        session.apply(parser::parse(line).unwrap(), at, &locations);
        if at == 3 {
            assert_eq!(
                session.location.as_ref().unwrap().0.name,
                "Stanton Gateway (Pyro)"
            );
        }
    }
    assert_eq!(
        session.location.as_ref().unwrap().0.name,
        "Pyro Gateway (Stanton)"
    );
    assert_eq!(
        session.route_origin.as_ref().unwrap().0.name,
        "Pyro Gateway (Stanton)"
    );
    assert!(locations.find_route_origin("Pyro Gateway", None).is_none());
    assert_eq!(
        locations
            .find("ObjectContainer_Lorville_City")
            .unwrap()
            .name,
        "Lorville"
    );
}

#[test]
fn route_origin_requires_the_matching_recent_local_selection() {
    let locations = Locations::load(None).unwrap();
    let mut session = Session::new(1);
    let route = "<Calculate Route> RSI_Galaxy_123[123]|CSCItemNavigation::CalculateRoute|Projected Start Location is Checkmate for route to destination rs_ext_pyro-stan_jp1";
    let selection = "<Player Selected Quantum Target - Local> RSI_Galaxy_123[123]|CSCItemNavigation::OnPlayerSelectedQuantumTarget|Player has selected point rs_ext_pyro-stan_jp1 as their destination";
    for (at, line, expected) in [
        (2, route, false),
        (3, selection, false),
        (6, route, false),
        (7, selection, false),
        (7, &route.replace("123[123]", "456[456]"), false),
        (7, &route.replace("rs_ext_pyro-stan_jp1", "Area18"), false),
        (7, route, true),
    ] {
        session.apply(parser::parse(line).unwrap(), at, &locations);
        assert_eq!(session.route_origin.is_some(), expected);
    }
    assert_eq!(
        presence::snapshot(&session, &Config::default(), 8)
            .unwrap()
            .details,
        "Checkmate"
    );
    session.apply(
        parser::parse("<Quantum Drive Arrived - Arrived at Final Destination> RSI_Galaxy_123[123]|CSCItemNavigation::OnQuantumDriveArrived|Quantum Drive has arrived at final destination").unwrap(),
        20,
        &locations,
    );
    assert!(session.route_origin.is_none());
    assert_eq!(session.location.unwrap().0.name, "Pyro-Stanton Jump Point");
}

#[test]
fn nyx_pyro_arrival_and_inventory_resolve_the_gateway() {
    let locations = Locations::load(None).unwrap();
    let mut session = Session::new(1);
    for (at, line) in [
        (2, "User Login Success - Handle[TestPilot]"),
        (
            3,
            "<Player Selected Quantum Target - Local> RSI_Galaxy_123[123]|CSCItemNavigation::OnPlayerSelectedQuantumTarget|Player has selected point rs_ext_nyx-pyro_jp1 as their destination",
        ),
        (
            215,
            "<Quantum Drive Arrived - Arrived at Final Destination> RSI_Galaxy_123[123]|CSCItemNavigation::OnQuantumDriveArrived|Quantum Drive has arrived at final destination",
        ),
        (
            216,
            "<RequestLocationInventory> Player[TestPilot] requested inventory for Location[RR_JP_NyxPyro]",
        ),
    ] {
        session.apply(parser::parse(line).unwrap(), at, &locations);
        if at == 215 {
            assert_eq!(
                session.location.as_ref().unwrap().0.name,
                "Nyx - Pyro Jump Point"
            );
        }
    }
    let card = presence::snapshot(&session, &Config::default(), 217).unwrap();
    assert_eq!(card.details, "Pyro Gateway (Nyx)");
    assert!(card.large_image.contains("Pyro_Gateway_Station_%28Nyx%29"));
    let custom = Config {
        custom_details: "Hauling cargo".into(),
        custom_state: "Taking the scenic route".into(),
        ..Config::default()
    };
    let card = presence::snapshot(&session, &custom, 217).unwrap();
    assert_eq!(card.details, "Hauling cargo");
    assert_eq!(card.state, "Taking the scenic route");
}

#[test]
fn pyro_log_identifiers_resolve_at_the_supported_precision() {
    let locations = Locations::load(None).unwrap();
    for (id, expected) in [
        ("RR_JP_PyroNyx", "Nyx Gateway (Pyro)"),
        ("rs_ext_pyro-stan_jp1", "Pyro-Stanton Jump Point"),
        ("RR_P3_LEO", "Orbituary"),
        ("rs_ext_pyro3_leo", "Orbituary"),
        ("rs_ext_pyro6_leo", "Ruin Station"),
        ("RR_P2_L4", "Checkmate"),
        ("rs_ext_pyro2_l4", "PYR2 L4"),
        ("Pyro3_Outpost_col_m_trdpst_otlw_006", "Bloom"),
        ("Pyro5b_Outpost_col_m_scrp_otlw_001", "Vatra"),
        ("Pyro6_Outpost_col_m_trdpst_indy_001", "Terminus"),
    ] {
        let location = locations.find(id).unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(
            (location.name.as_str(), location.system.as_str()),
            (expected, "Pyro"),
            "{id}"
        );
    }
    assert!(locations.find("NavPoint_Dynamic_123").is_none());
}

#[test]
fn local_inventory_transitions_reuse_a_location_learned_in_the_session() {
    let locations = Locations::load(None).unwrap();
    let mut session = Session::new(1);
    for (at, line, expected) in [
        (2, "User Login Success - Handle[TestPilot]", None),
        (
            3,
            "<Update Inventory Location> Player [TestPilot] is changing location. Landing [0] -> [10]. Location [0] -> [42]. Pending [0]",
            None,
        ),
        (
            3,
            "<RequestLocationInventory> Player[TestPilot] requested inventory for Location[RR_P2_L4]",
            Some("Checkmate"),
        ),
        (
            4,
            "<Update Inventory Location> Player [Other] is changing location. Landing [10] -> [0]. Location [42] -> [99]. Pending [0]",
            Some("Checkmate"),
        ),
        (
            5,
            "<Update Inventory Location> Player [TestPilot] is changing location. Landing [10] -> [0]. Location [42] -> [99]. Pending [0]",
            Some("Checkmate"),
        ),
        (
            6,
            "<Update Inventory Location> Player [TestPilot] is changing location. Landing [0] -> [10]. Location [99] -> [42]. Pending [0]",
            Some("Checkmate"),
        ),
    ] {
        session.apply(parser::parse(line).unwrap(), at, &locations);
        assert_eq!(
            session
                .location
                .as_ref()
                .map(|(location, _)| location.name.as_str()),
            expected
        );
        if at == 5 {
            assert_eq!(session.location.as_ref().unwrap().1, 3);
        }
    }
}

#[test]
fn levski_survives_incapacitation_and_local_atc_refreshes_it() {
    let locations = Locations::load(None).unwrap();
    let mut session = Session::new(1);
    for (at, line) in [
        (2, "User Login Success - Handle[TestPilot]"),
        (
            3,
            "<RequestLocationInventory> Player[TestPilot] requested inventory for Location[Nyx_Levski]",
        ),
        (
            4,
            "<SHUDEvent_OnNotification> Added notification \"Incapacitated: Example\"",
        ),
        (
            1200,
            "<Connection Flow> CSCCommsComponent::DoEstablishCommunicationCommon: Update bubble created for communication connection '12' on channel '0' for OtherPilot [34] to track their communication partner AImodule_ATC_LevskiATC01_56 [56]",
        ),
        (
            1300,
            "<Connection Flow> CSCCommsComponent::DoEstablishCommunicationCommon: Update bubble created for communication connection '12' on channel '0' for TestPilot [34] to track their communication partner AImodule_ATC_LevskiATC01_56 [56]",
        ),
    ] {
        session.apply(parser::parse(line).unwrap(), at, &locations);
        assert_eq!(
            session.location.as_ref().map(|(_, seen)| *seen),
            match at {
                2 => None,
                1300 => Some(1300),
                _ => Some(3),
            }
        );
    }
    let card = presence::snapshot(&session, &Config::default(), 1301).unwrap();
    assert_eq!(card.details, "Levski");
    assert_eq!(card.state, "Nyx · LIVE");
    assert!(card.large_image.contains("Levski_-_Above_complex"));
}

#[test]
fn modern_poi_and_vehicle_identifiers_reach_the_discord_payload() {
    let locations = Locations::load(None).unwrap();
    let mut session = Session::new(1);
    for (at, line) in [
        "[CIG-net] User Login Success - Handle[TestPilot]",
        "[CSessionManager::OnClientSpawned] Spawned!",
        "<RequestLocationInventory> Player[TestPilot] requested inventory for Location[Stanton4_DistributionCentre_Astro_Plant34-12]",
        "<SHUDEvent_OnNotification> Added notification \"You have joined channel '@vehicle_NameRSI_Apollo_Medivac : TestPilot'.",
    ].iter().enumerate() {
        session.apply(parser::parse(line).unwrap(), at as i64 + 2, &locations);
    }
    let payload = presence::snapshot(&session, &Config::default(), 6).unwrap();
    assert_eq!(payload.details, "Cry-Astro Processing Plant 34-12");
    assert_eq!(payload.state, "RSI Apollo Medivac");
    session.apply(parser::parse("<SHUDEvent_OnNotification> Added notification \"Received Blueprint: New Research Equipment: ").unwrap(), 7, &locations);
    let payload = presence::snapshot(&session, &Config::default(), 8).unwrap();
    assert_eq!(payload.details, "Blueprint acquired");
    assert_eq!(payload.state, "RSI Apollo Medivac · New Research Equipment");
    assert_eq!(
        presence::snapshot(&session, &Config::default(), 68)
            .unwrap()
            .state,
        "RSI Apollo Medivac"
    );
}

#[test]
fn source_metadata_matches_bundled_catalogue_counts_and_version() {
    let provenance: serde_json::Value =
        serde_json::from_str(include_str!("../data/content-source.json")).unwrap();
    let locations: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../data/locations.json")).unwrap();
    let vehicles: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../data/vehicles.json")).unwrap();
    assert_eq!(
        provenance["catalogued_locations"].as_u64().unwrap() as usize,
        locations.len()
    );
    assert_eq!(
        provenance["catalogued_vehicles"].as_u64().unwrap() as usize,
        vehicles.len()
    );
    assert_eq!(provenance["game_version"], "4.10.2-LIVE.12881860");
}
