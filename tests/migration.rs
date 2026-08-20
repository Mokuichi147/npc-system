use std::collections::BTreeSet;

use npc_system::config::SimulationConfig;
use npc_system::event::WorldEvent;
use npc_system::goal::{Goal, GoalKind};
use npc_system::id::{NpcId, TownId};
use npc_system::migration::{best_destination, candidate_towns, household_members, move_household};
use npc_system::npc::{Attributes, Npc, Sex};
use npc_system::town::{Town, TownConnection};
use npc_system::{Simulation, World};

fn town(id: u16, capacity: u32, quality: u8) -> Town {
    Town::new(
        TownId(id),
        format!("Town {id}"),
        capacity,
        quality,
        quality,
        quality,
        quality,
        quality,
    )
}

fn npc(id: u32, age: u8, town: TownId) -> Npc {
    Npc::with_default_name(
        NpcId(id),
        age,
        Sex::Male,
        town,
        town,
        Attributes::default(),
        Vec::new(),
        Goal::new(GoalKind::LivePeacefully, 0),
    )
}

fn connect(towns: &mut [Town], a: usize, b: usize, distance: u8) {
    let a_id = towns[a].id;
    let b_id = towns[b].id;
    towns[a].add_neighbor(TownConnection::new(b_id, distance));
    towns[b].add_neighbor(TownConnection::new(a_id, distance));
}

fn migration_only_config() -> SimulationConfig {
    let mut config = SimulationConfig::normal();
    config.birth_rates.clear();
    config.mortality_rates.clear();
    config.immigration_rate = 0.0;
    config.emigration_rate = 0.0;
    config.internal_migration_monthly_probability = 1.0;
    config.partnership_monthly_probability = 0.0;
    config.social_event_monthly_probability = 0.0;
    config.disaster_probability = 0.0;
    config.disease_probability = 0.0;
    config.war_probability = 0.0;
    config.goal_reassessment_probability = 0.0;
    config
}

#[test]
fn household_migration_keeps_partner_and_minor_children_together() {
    let origin = TownId(0);
    let destination = TownId(1);
    let mut world = World::empty(7);
    world.towns = vec![town(0, 100, 5), town(1, 100, 8)];

    let mut parent_a = npc(0, 36, origin);
    let mut parent_b = npc(1, 34, origin);
    let mut minor = npc(2, 12, origin);
    let mut adult_child = npc(3, 18, origin);

    assert!(parent_a.set_partner(parent_b.id));
    assert!(parent_b.set_partner(parent_a.id));
    for child in [minor.id, adult_child.id] {
        assert!(parent_a.add_child(child));
        assert!(parent_b.add_child(child));
    }
    for parent in [parent_a.id, parent_b.id] {
        assert!(minor.add_parent(parent));
        assert!(adult_child.add_parent(parent));
    }

    world.npcs = vec![parent_a, parent_b, minor, adult_child];
    world.rebuild_active_npcs();
    world.validate().unwrap();

    assert_eq!(
        household_members(&world, NpcId(0)),
        vec![NpcId(0), NpcId(1), NpcId(2)]
    );
    assert_eq!(
        move_household(&mut world, NpcId(0), destination),
        vec![NpcId(0), NpcId(1), NpcId(2)]
    );

    for id in [NpcId(0), NpcId(1), NpcId(2)] {
        assert_eq!(world.npc(id).unwrap().town, destination);
    }
    assert_eq!(world.npc(NpcId(3)).unwrap().town, origin);
    world.validate().unwrap();
}

#[test]
fn migration_candidates_stop_at_two_hops() {
    let mut world = World::empty(11);
    world.towns = (0..=4).map(|id| town(id, 100, 6)).collect();
    connect(&mut world.towns, 0, 1, 2);
    connect(&mut world.towns, 1, 2, 3);
    connect(&mut world.towns, 2, 3, 1);

    assert_eq!(
        candidate_towns(&world, TownId(0)),
        vec![(TownId(1), 2), (TownId(2), 5)]
    );
    assert!(
        candidate_towns(&world, TownId(0))
            .iter()
            .all(|(id, _)| !matches!(id, TownId(3) | TownId(4)))
    );
}

#[test]
fn capacity_pressure_changes_the_best_destination() {
    let mut world = World::empty(13);
    world.towns = vec![town(0, 100, 2), town(1, 20, 10), town(2, 200, 10)];
    connect(&mut world.towns, 0, 1, 1);
    connect(&mut world.towns, 0, 2, 1);
    world.npcs = vec![npc(0, 30, TownId(0))];
    world.rebuild_active_npcs();

    let destination_with_room = best_destination(&world, NpcId(0), &[1, 10, 20]).unwrap();
    assert_eq!(destination_with_room.destination, TownId(1));

    let destination_after_overcrowding = best_destination(&world, NpcId(0), &[1, 30, 50]).unwrap();
    assert_eq!(destination_after_overcrowding.destination, TownId(2));
    assert!(
        world.towns[1].attractiveness(30) < world.towns[2].attractiveness(50),
        "収容力を超えた都市は、余裕のある同品質都市より魅力度が低い必要がある"
    );
}

#[test]
fn yearly_simulation_migrates_residents_from_multiple_towns() {
    let config = migration_only_config();
    let mut simulation = Simulation::new(4, 160, 23, config).unwrap();

    for town in &mut simulation.world.towns {
        town.neighbors.clear();
        town.population_capacity = 80;
        let quality = if town.id.0 < 2 { 1 } else { 10 };
        town.jobs = quality;
        town.safety = quality;
        town.education = quality;
        town.freedom = quality;
        town.wealth = quality;
    }
    connect(&mut simulation.world.towns, 0, 2, 1);
    connect(&mut simulation.world.towns, 0, 3, 1);
    connect(&mut simulation.world.towns, 1, 2, 1);
    connect(&mut simulation.world.towns, 1, 3, 1);

    simulation.run_year().unwrap();
    let sources = simulation
        .world
        .important_events
        .iter()
        .filter_map(|event| match event {
            WorldEvent::Migration { from, .. } => Some(*from),
            _ => None,
        })
        .collect::<BTreeSet<_>>();

    assert!(sources.contains(&TownId(0)));
    assert!(sources.contains(&TownId(1)));
    assert!(simulation.world.statistics.cumulative().internal_migrations > 0);
    assert_eq!(
        simulation.world.town_populations().iter().sum::<usize>(),
        simulation.world.active_population()
    );
    simulation.world.validate().unwrap();
}
