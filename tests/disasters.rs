use npc_system::config::SimulationConfig;
use npc_system::disaster::{FamineConditions, FamineEvent, NaturalDisaster, NaturalDisasterEvent};
use npc_system::disease::DiseaseEvent;
use npc_system::event::WorldEvent;
use npc_system::id::TownId;
use npc_system::town::TownConnection;
use npc_system::war::WarEvent;
use npc_system::{Simulation, WorldDanger};

fn events_off_config() -> SimulationConfig {
    let mut config = SimulationConfig::normal();
    config.birth_rates.clear();
    config.mortality_rates.clear();
    config.immigration_rate = 0.0;
    config.emigration_rate = 0.0;
    config.internal_migration_monthly_probability = 0.0;
    config.partnership_monthly_probability = 0.0;
    config.social_event_monthly_probability = 0.0;
    config.disaster_probability = 0.0;
    config.disease_probability = 0.0;
    config.war_probability = 0.0;
    config.goal_reassessment_probability = 0.0;
    config.disaster_max_mortality = 0.0;
    config.disease_max_mortality = 0.0;
    config.war_max_mortality = 0.0;
    config.famine_max_mortality = 0.0;
    config
}

fn connect(simulation: &mut Simulation, a: usize, b: usize, distance: u8) {
    let a_id = simulation.world.towns[a].id;
    let b_id = simulation.world.towns[b].id;
    simulation.world.towns[a].add_neighbor(TownConnection::new(b_id, distance));
    simulation.world.towns[b].add_neighbor(TownConnection::new(a_id, distance));
}

#[test]
fn negative_event_types_expose_bounded_damage_and_effects() {
    let mut lethal = events_off_config();
    lethal.disaster_max_mortality = f64::INFINITY;
    lethal.disease_max_mortality = f64::INFINITY;
    lethal.war_max_mortality = f64::INFINITY;
    lethal.famine_max_mortality = f64::INFINITY;

    for kind in NaturalDisaster::ALL {
        let event = NaturalDisasterEvent::new(kind, TownId(0), 10);
        let damage = event.town_damage(1_000);
        assert_eq!(event.kind, kind);
        assert!(damage.jobs_loss > 0);
        assert!(damage.safety_loss > 0);
        assert!(damage.capacity_loss > 0);
        assert_eq!(event.mortality_probability(&lethal), 1.0);
    }

    let disease = DiseaseEvent::new(10, TownId(0), 12);
    assert!(disease.town_damage().jobs_loss > 0);
    assert_eq!(disease.additional_mortality(30, &lethal), 1.0);
    assert!(disease.birth_rate_multiplier() < 1.0);
    assert!(disease.mobility_multiplier() < 1.0);

    let war = WarEvent::new([TownId(0), TownId(1)], 10, 2);
    assert!(war.includes(TownId(0)) && war.includes(TownId(1)));
    assert!(war.town_damage().jobs_loss > 0);
    assert!(war.town_damage().safety_loss > 0);
    assert_eq!(war.additional_mortality(&lethal), 1.0);
    assert!(war.birth_rate_multiplier() < 1.0);
    assert!(war.emigration_multiplier() > 1.0);

    let famine = FamineEvent::from_conditions(
        TownId(0),
        FamineConditions {
            disaster_severity: 10,
            war_severity: 10,
            occupancy: 1.5,
            effective_jobs: 0,
            effective_safety: 0,
        },
    )
    .expect("複合的な危機条件では飢饉が発生する");
    assert!(famine.town_damage().jobs_loss > 0);
    assert!(famine.town_damage().safety_loss > 0);
    assert_eq!(famine.mortality_probability(&lethal), 1.0);
    assert!(famine.birth_rate_multiplier() < 1.0);
    assert!(famine.emigration_multiplier() > 1.0);
}

#[test]
fn certain_disaster_kills_only_the_affected_town_population() {
    let mut config = events_off_config();
    config.disaster_probability = 1.0;
    config.disaster_max_mortality = f64::INFINITY;
    let mut simulation = Simulation::new(3, 90, 31, config).unwrap();
    let initial_populations = simulation.world.town_populations();

    let statistics = simulation.run_year().unwrap().clone();
    let affected = simulation
        .world
        .important_events
        .iter()
        .find_map(|event| match event {
            WorldEvent::NaturalDisaster { town } => Some(*town),
            _ => None,
        })
        .expect("発生確率1のため災害イベントが記録される");

    assert_eq!(
        statistics.disaster_deaths,
        initial_populations[usize::from(affected.0)]
    );
    assert_eq!(statistics.deaths, statistics.disaster_deaths);
    simulation.world.validate().unwrap();
}

#[test]
fn certain_war_causes_casualties_in_two_towns() {
    let mut config = events_off_config();
    config.war_probability = 1.0;
    config.war_max_mortality = f64::INFINITY;
    let mut simulation = Simulation::new(4, 120, 37, config).unwrap();

    let statistics = simulation.run_year().unwrap().clone();

    assert!(
        simulation
            .world
            .important_events
            .iter()
            .any(|event| matches!(event, WorldEvent::WarStarted))
    );
    assert_eq!(statistics.war_deaths, 60);
    assert_eq!(statistics.deaths, statistics.war_deaths);
    simulation.world.validate().unwrap();
}

#[test]
fn disease_spreads_one_connection_at_a_time_with_certain_probability() {
    let mut config = events_off_config();
    config.disease_spread_monthly_probability = 1.0;
    let mut simulation = Simulation::new(3, 90, 41, config).unwrap();

    for town in &mut simulation.world.towns {
        town.neighbors.clear();
        town.population_capacity = 30;
    }
    connect(&mut simulation, 0, 1, 1);
    connect(&mut simulation, 1, 2, 1);
    simulation
        .world
        .active_diseases
        .push(DiseaseEvent::new(10, TownId(0), 24));

    simulation.run_year().unwrap();

    let disease = simulation.world.active_diseases.first().unwrap();
    assert_eq!(
        disease.infected_towns_sorted(),
        vec![TownId(0), TownId(1), TownId(2)]
    );
    assert_eq!(disease.remaining_months, 12);
    assert_eq!(simulation.world.statistics.cumulative().disease_deaths, 0);
    simulation.world.validate().unwrap();
}

fn aggregate_negative_event_deaths(danger: WorldDanger) -> usize {
    const SEEDS: [u64; 5] = [2, 3, 5, 7, 11];
    SEEDS
        .into_iter()
        .map(|seed| {
            let mut simulation =
                Simulation::new(4, 100, seed, SimulationConfig::for_danger(danger)).unwrap();
            simulation.run(15).unwrap();
            let statistics = simulation.world.statistics.cumulative();
            statistics
                .disaster_deaths
                .saturating_add(statistics.disease_deaths)
                .saturating_add(statistics.war_deaths)
                .saturating_add(statistics.famine_deaths)
        })
        .sum()
}

#[test]
fn danger_presets_order_average_negative_event_impact_across_seeds() {
    let peaceful = aggregate_negative_event_deaths(WorldDanger::Peaceful);
    let normal = aggregate_negative_event_deaths(WorldDanger::Normal);
    let harsh = aggregate_negative_event_deaths(WorldDanger::Harsh);

    assert!(peaceful < normal, "peaceful={peaceful}, normal={normal}");
    assert!(normal < harsh, "normal={normal}, harsh={harsh}");
}

fn aggregate_final_population(danger: WorldDanger) -> usize {
    [1_u64, 2, 3, 4, 5]
        .into_iter()
        .map(|seed| {
            let mut simulation =
                Simulation::new(3, 100, seed, SimulationConfig::for_danger(danger)).unwrap();
            simulation.run(30).unwrap();
            simulation.world.active_population()
        })
        .sum()
}

#[test]
fn danger_presets_order_average_final_population_across_seeds() {
    let peaceful = aggregate_final_population(WorldDanger::Peaceful);
    let normal = aggregate_final_population(WorldDanger::Normal);
    let harsh = aggregate_final_population(WorldDanger::Harsh);

    assert!(peaceful > normal, "peaceful={peaceful}, normal={normal}");
    assert!(normal > harsh, "normal={normal}, harsh={harsh}");
}
