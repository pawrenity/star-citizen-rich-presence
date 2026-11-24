use crate::{
    locations::{Location, Locations},
    parser::Event,
};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Phase {
    Starting,
    Menu,
    Loading,
    Playing,
    Stopped,
}

#[derive(Debug)]
pub struct Session {
    pub started: i64,
    pub phase: Phase,
    pub handle: Option<String>,
    pub ship: Option<String>,
    pub contracts: BTreeSet<String>,
    pub location: Option<(Location, i64)>,
    pub route_origin: Option<(Location, i64)>,
    pub incapacitated_at: Option<i64>,
    pub notice: Option<(String, String, i64)>,
    pub last_atc: Option<i64>,
    /// Other party members seen joining; `Some` while the player is in a party.
    pub party: Option<BTreeSet<String>>,
    target: Option<(String, String, i64)>,
    loading_screen_open: bool,
    inventory_zone: Option<(u64, i64)>,
    inventory_zones: HashMap<u64, Location>,
}

impl Session {
    pub fn new(started: i64) -> Self {
        Self {
            started,
            phase: Phase::Starting,
            handle: None,
            ship: None,
            contracts: BTreeSet::new(),
            location: None,
            route_origin: None,
            incapacitated_at: None,
            notice: None,
            last_atc: None,
            party: None,
            target: None,
            loading_screen_open: false,
            inventory_zone: None,
            inventory_zones: HashMap::new(),
        }
    }

    fn clear_gameplay(&mut self) {
        self.ship = None;
        self.contracts.clear();
        self.location = None;
        self.route_origin = None;
        self.target = None;
        self.incapacitated_at = None;
        self.notice = None;
        self.last_atc = None;
        self.inventory_zone = None;
        self.inventory_zones.clear();
    }

    fn remember_inventory_zone(&mut self, at: i64, window: i64) {
        if let (Some((zone, changed)), Some((known, _))) =
            (self.inventory_zone, self.location.as_ref())
            && (0..=window).contains(&(at - changed))
            && (self.inventory_zones.len() < 128 || self.inventory_zones.contains_key(&zone))
        {
            self.inventory_zones.insert(zone, known.clone());
        }
    }

    pub fn apply(&mut self, event: Event, at: i64, locations: &Locations) {
        match event {
            Event::Login(handle) => {
                if self.handle.as_ref().is_some_and(|old| old != &handle) {
                    self.clear_gameplay();
                    self.party = None;
                }
                self.handle = Some(handle);
            }
            Event::Mode(mode) => {
                self.clear_gameplay();
                self.phase = if mode == "SC_Frontend" {
                    Phase::Menu
                } else {
                    Phase::Playing
                };
            }
            Event::Loading => {
                self.clear_gameplay();
                self.phase = Phase::Loading;
            }
            Event::LoadingScreenOpened => {
                self.clear_gameplay();
                self.loading_screen_open = true;
            }
            Event::LoadingScreenClosed { menu } => {
                self.loading_screen_open = false;
                self.phase = if menu { Phase::Menu } else { Phase::Playing };
            }
            Event::Ready => {
                if self.phase != Phase::Menu {
                    self.phase = Phase::Playing;
                }
            }
            Event::Disconnect => {
                self.clear_gameplay();
                self.loading_screen_open = false;
                self.phase = Phase::Menu;
            }
            Event::Shutdown => {
                self.clear_gameplay();
                // Parties survive server hops and the menu, but not quitting the game.
                self.party = None;
                self.loading_screen_open = false;
                self.phase = Phase::Stopped;
            }
            Event::Inventory { player, location } => {
                // This log includes other actors too. Require the authenticated local handle.
                if self
                    .handle
                    .as_ref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(&player))
                {
                    self.phase = Phase::Playing;
                    self.route_origin = None;
                    self.location = locations.find(&location).map(|l| (l, at));
                    self.remember_inventory_zone(at, 2);
                    self.incapacitated_at = None;
                }
            }
            Event::InventoryTransition {
                player,
                location_id,
            } => {
                if self
                    .handle
                    .as_ref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(&player))
                {
                    self.inventory_zone = (location_id != 0).then_some((location_id, at));
                    if let Some(known) = self.inventory_zones.get(&location_id) {
                        self.location = Some((known.clone(), at));
                    }
                    // Unknown zones do not refresh the last verified sighting.
                }
            }
            Event::InventoryTerminal {
                player,
                location_id,
            } => {
                if self
                    .handle
                    .as_ref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(&player))
                    && let Some(location) = self.inventory_zones.get(&location_id)
                {
                    self.location = Some((location.clone(), at));
                }
            }
            Event::QuantumTarget {
                vehicle,
                destination,
            } => {
                self.phase = Phase::Playing;
                if let Some(ship) = crate::vehicles::find(&vehicle) {
                    self.ship = Some(ship);
                }
                self.target = Some((vehicle, destination, at));
            }
            Event::RouteOrigin {
                vehicle,
                destination,
                origin,
            } => {
                if self.phase == Phase::Playing
                    && self
                        .target
                        .as_ref()
                        .is_some_and(|(tracked, selected, time)| {
                            tracked == &vehicle
                                && selected == &destination
                                && (0..=2).contains(&(at - time))
                        })
                    && let Some(location) = locations.find_route_origin(
                        &origin,
                        self.location
                            .as_ref()
                            .filter(|(_, seen)| (0..=120).contains(&(at - seen)))
                            .map(|(location, _)| location.system.as_str()),
                    )
                {
                    self.route_origin = Some((location, at));
                }
            }
            Event::AtcContact { player, location } => {
                if self.phase == Phase::Playing
                    && self
                        .handle
                        .as_ref()
                        .is_some_and(|h| h.eq_ignore_ascii_case(&player))
                {
                    self.last_atc = Some(at);
                    if let Some(location) =
                        location.as_deref().and_then(|name| locations.find(name))
                    {
                        self.location = Some((location, at));
                    }
                }
            }
            Event::QuantumArrival { vehicle } => {
                if self
                    .target
                    .as_ref()
                    .is_some_and(|(tracked, _, _)| tracked == &vehicle)
                    && let Some((_, target, selected)) = self.target.take()
                {
                    self.last_atc = None;
                    self.route_origin = None;
                    if (0..=600).contains(&(at - selected)) {
                        self.phase = Phase::Playing;
                        // Unknown/mission destinations invalidate the previous location.
                        self.location = locations.find(&target).map(|l| (l, at));
                        if self
                            .inventory_zone
                            .is_some_and(|(_, changed)| changed >= selected)
                        {
                            self.remember_inventory_zone(at, 15);
                        }
                    } else {
                        self.location = None;
                    }
                    self.inventory_zone = None;
                }
            }
            Event::Board(ship) => {
                self.phase = Phase::Playing;
                self.ship = Some(ship);
                self.incapacitated_at = None;
            }
            Event::Disembark(ship) => {
                if self.ship.as_ref() == Some(&ship) {
                    self.ship = None;
                    self.target = None;
                }
            }
            Event::ContractAccepted(contract) => {
                self.phase = Phase::Playing;
                if self.contracts.len() < 32 {
                    self.contracts.insert(contract);
                }
            }
            Event::ContractEnded(contract) => {
                self.contracts.remove(&contract);
            }
            Event::Incapacitated => {
                self.incapacitated_at = Some(at);
                self.target = None;
                self.notice = None;
            }
            Event::Spawned => {
                if matches!(self.phase, Phase::Menu | Phase::Stopped) {
                    return;
                }
                self.phase = Phase::Playing;
                self.ship = None;
                self.location = None;
                self.route_origin = None;
                self.target = None;
                self.incapacitated_at = None;
                self.notice = None;
                self.last_atc = None;
                self.inventory_zone = None;
            }
            Event::PartyJoined(name) | Event::PartyActive(Some(name)) => {
                let is_local = self
                    .handle
                    .as_ref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(&name));
                let party = self.party.get_or_insert_default();
                if !is_local
                    && party.len() < 64
                    && !party.iter().any(|m| m.eq_ignore_ascii_case(&name))
                {
                    party.insert(name);
                }
            }
            Event::PartyActive(None) => {
                self.party.get_or_insert_default();
            }
            Event::PartyLeft(name) => {
                if self
                    .handle
                    .as_ref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(&name))
                {
                    self.party = None;
                } else if let Some(party) = &mut self.party {
                    party.retain(|member| !member.eq_ignore_ascii_case(&name));
                }
            }
            Event::PartyEnded => self.party = None,
            Event::Notice { details, state } => {
                if self.phase == Phase::Playing {
                    self.notice = Some((details, state, at));
                }
            }
        }
        if self.loading_screen_open {
            self.phase = Phase::Loading;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn locations() -> Locations {
        Locations::load(None).unwrap()
    }
    #[test]
    fn respawn_clears_stale_vehicle_and_location_but_keeps_active_contracts() {
        let locations = locations();
        let mut s = Session::new(123);
        s.apply(Event::Login("Pilot".into()), 124, &locations);
        s.apply(
            Event::Inventory {
                player: "Pilot".into(),
                location: "Area18".into(),
            },
            125,
            &locations,
        );
        s.apply(Event::Board("RSI Apollo Medivac".into()), 126, &locations);
        s.apply(
            Event::ContractAccepted("New Alliance Aid contract".into()),
            127,
            &locations,
        );
        s.apply(Event::Incapacitated, 128, &locations);
        s.apply(Event::Spawned, 129, &locations);
        assert_eq!(s.phase, Phase::Playing);
        assert!(s.ship.is_none() && s.location.is_none() && s.incapacitated_at.is_none());
        assert_eq!(s.contracts.len(), 1);
        assert_eq!(s.started, 123);
    }
    #[test]
    fn inventory_is_attributed_and_unknown_locations_clear_stale_state() {
        let mut s = Session::new(0);
        s.apply(Event::Login("Pilot".into()), 1, &locations());
        s.apply(
            Event::Inventory {
                player: "Other".into(),
                location: "Area18".into(),
            },
            2,
            &locations(),
        );
        assert!(s.location.is_none());
        s.apply(
            Event::Inventory {
                player: "Pilot".into(),
                location: "Area18".into(),
            },
            3,
            &locations(),
        );
        assert_eq!(s.location.as_ref().unwrap().0.name, "Area18");
        s.apply(
            Event::Inventory {
                player: "Pilot".into(),
                location: "NewUnmappedArea".into(),
            },
            4,
            &locations(),
        );
        assert!(s.location.is_none());
    }
    #[test]
    fn selection_is_not_arrival_and_arrivals_require_the_same_vehicle() {
        let mut s = Session::new(0);
        s.apply(
            Event::QuantumTarget {
                vehicle: "ship1".into(),
                destination: "Area18".into(),
            },
            1,
            &locations(),
        );
        assert!(s.location.is_none());
        s.apply(
            Event::QuantumArrival {
                vehicle: "ship2".into(),
            },
            2,
            &locations(),
        );
        assert!(s.location.is_none());
        s.apply(
            Event::QuantumTarget {
                vehicle: "ship1".into(),
                destination: "Area18".into(),
            },
            3,
            &locations(),
        );
        s.apply(
            Event::QuantumArrival {
                vehicle: "ship1".into(),
            },
            4,
            &locations(),
        );
        assert_eq!(s.location.as_ref().unwrap().0.name, "Area18");
        s.apply(
            Event::QuantumTarget {
                vehicle: "ship1".into(),
                destination: "MISSION_QT_Bounty_Beacon_123".into(),
            },
            5,
            &locations(),
        );
        s.apply(
            Event::QuantumArrival {
                vehicle: "unrelated_ship".into(),
            },
            6,
            &locations(),
        );
        assert_eq!(s.location.as_ref().unwrap().0.name, "Area18");
        s.apply(
            Event::QuantumArrival {
                vehicle: "ship1".into(),
            },
            6,
            &locations(),
        );
        assert!(s.location.is_none());
    }
    #[test]
    fn party_handles_match_case_insensitively() {
        let mut s = Session::new(0);
        s.apply(Event::Login("Pilot".into()), 1, &locations());
        s.apply(Event::PartyJoined("Friend".into()), 2, &locations());
        s.apply(Event::PartyJoined("friend".into()), 3, &locations());
        assert_eq!(s.party.as_ref().unwrap().len(), 1);
        s.apply(Event::PartyLeft("FRIEND".into()), 4, &locations());
        assert_eq!(s.party.as_ref().unwrap().len(), 0);
        s.apply(Event::PartyJoined("pilot".into()), 5, &locations());
        assert_eq!(s.party.as_ref().unwrap().len(), 0);
    }
    #[test]
    fn lowercase_pilot_leaving_the_party_clears_it_for_login_pilot() {
        let mut s = Session::new(0);
        s.apply(Event::Login("Pilot".into()), 1, &locations());
        s.apply(Event::PartyJoined("Friend".into()), 2, &locations());
        // "pilot has left the party" from the log.
        s.apply(Event::PartyLeft("pilot".into()), 3, &locations());
        assert!(s.party.is_none());
    }
    #[test]
    fn teardown_clears_gameplay_and_preserves_session_time() {
        let mut s = Session::new(123);
        s.apply(Event::Board("Ship".into()), 124, &locations());
        s.apply(Event::ContractAccepted("Haul".into()), 125, &locations());
        s.apply(Event::Disconnect, 126, &locations());
        assert_eq!(s.started, 123);
        assert_eq!(s.phase, Phase::Menu);
        assert!(s.ship.is_none() && s.contracts.is_empty());
    }
}
