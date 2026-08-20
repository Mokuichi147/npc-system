use npc_system::{Simulation, SimulationConfig};

fn assert_stress_scenario(towns: usize, population: usize, seed: u64) {
    let mut simulation = Simulation::new(towns, population, seed, SimulationConfig::normal())
        .expect("ストレステスト用Worldを生成できる");
    simulation.run(100).expect("100年間を完走できる");

    simulation
        .world
        .validate()
        .expect("100年後も参照整合性を維持する");
    assert_eq!(simulation.world.statistics.years.len(), 100);
    let cumulative = simulation.world.statistics.cumulative();
    assert!(cumulative.births > 0);
    assert!(cumulative.deaths > 0);
    assert!(cumulative.internal_migrations > 0);
    assert!(cumulative.final_population > population / 10);
    assert!(cumulative.final_population < population.saturating_mul(5));

    let health = simulation.health_metrics();
    assert!(health.average_active_relationships <= 35.0);
    let largest_town = cumulative
        .town_populations
        .iter()
        .copied()
        .max()
        .unwrap_or_default();
    assert!(largest_town.saturating_mul(2) < cumulative.final_population.max(1));
}

#[test]
fn multi_town_smoke_run_is_healthy() {
    let mut simulation = Simulation::new(5, 500, 2026, SimulationConfig::normal())
        .expect("複数都市Worldを生成できる");
    simulation.run(30).expect("30年間を完走できる");
    simulation.world.validate().expect("不変条件を満たす");
    let cumulative = simulation.world.statistics.cumulative();
    assert!(cumulative.internal_migrations > 0);
    assert!(simulation.health_metrics().average_active_relationships <= 35.0);
}

#[test]
#[ignore = "release用ストレステスト: cargo test --release --test long_run -- --ignored"]
fn test_e_2500_npcs_ten_towns_hundred_years() {
    assert_stress_scenario(10, 2_500, 12_345);
}

#[test]
#[ignore = "release用ストレステスト: cargo test --release --test long_run -- --ignored"]
fn test_f_5000_npcs_twenty_towns_hundred_years() {
    assert_stress_scenario(20, 5_000, 12_345);
}
