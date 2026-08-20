use std::collections::HashSet;

use npc_system::npc::MAX_ATTRIBUTE;
use npc_system::{Simulation, SimulationConfig};

const INITIAL_POPULATION: usize = 100;
const YEARS: u16 = 100;
const SEED: u64 = 123;

#[test]
fn test_d_one_town_hundred_npcs_run_for_hundred_years_reproducibly() {
    let config = SimulationConfig::normal();
    let mut simulation =
        Simulation::new(1, INITIAL_POPULATION, SEED, config.clone()).expect("初期世界を生成できる");
    simulation.run(YEARS).expect("100年間を完走できる");

    assert_eq!(simulation.world.year, YEARS);
    assert_eq!(simulation.world.statistics.years.len(), usize::from(YEARS));
    simulation
        .world
        .validate()
        .expect("100年後もWorldの不変条件を満たす");

    assert_yearly_population_conservation(&simulation);
    assert_world_references_and_numeric_ranges(&simulation);
    assert_generation_turnover(&simulation);

    let health = simulation.health_metrics();
    assert!(health.average_active_relationships.is_finite());
    assert!(health.average_strong_relationships.is_finite());
    assert!(health.extreme_relationship_fraction.is_finite());
    assert!((0.0..=1.0).contains(&health.extreme_relationship_fraction));

    let expected_statistics = simulation.world.statistics.years.clone();
    let mut replay = Simulation::new(1, INITIAL_POPULATION, SEED, config)
        .expect("同じseedの初期世界を生成できる");
    replay.run(YEARS).expect("同じseedでも100年間を完走できる");
    assert_eq!(
        replay.world.statistics.years, expected_statistics,
        "同一seedでは全YearStatisticsが一致する"
    );
}

fn assert_yearly_population_conservation(simulation: &Simulation) {
    let mut previous_population = INITIAL_POPULATION;

    for (index, statistics) in simulation.world.statistics.years.iter().enumerate() {
        assert_eq!(statistics.year as usize, index + 1);
        assert_eq!(statistics.town_populations.len(), 1);
        assert_eq!(
            statistics.town_populations.iter().sum::<usize>(),
            statistics.total_population
        );
        assert!((statistics.total_population as f64).is_finite());

        let expected = previous_population
            .saturating_add(statistics.births)
            .saturating_add(statistics.external_immigration)
            .saturating_sub(statistics.deaths)
            .saturating_sub(statistics.external_emigration);
        assert_eq!(
            statistics.total_population, expected,
            "{}年目の人口収支が一致する",
            statistics.year
        );
        previous_population = statistics.total_population;
    }

    assert_eq!(previous_population, simulation.world.active_population());
}

fn assert_world_references_and_numeric_ranges(simulation: &Simulation) {
    let world = &simulation.world;
    let active_ids = world.active_npcs.iter().copied().collect::<HashSet<_>>();
    assert_eq!(active_ids.len(), world.active_npcs.len());

    for (index, npc) in world.npcs.iter().enumerate() {
        assert_eq!(npc.id.0 as usize, index, "NpcIdとVec indexが一致する");
        assert_eq!(active_ids.contains(&npc.id), npc.is_active());
        if !npc.alive {
            assert!(!active_ids.contains(&npc.id), "死亡NPCはactiveではない");
        }
        if npc.is_active() {
            assert!(world.town(npc.town).is_some(), "生存NPCの都市が存在する");
        }

        assert!(npc.attributes.values_in_range());
        assert!(
            [
                npc.attributes.physical,
                npc.attributes.dexterity,
                npc.attributes.intelligence,
                npc.attributes.charisma,
                npc.attributes.willpower,
            ]
            .into_iter()
            .all(|value| value <= MAX_ATTRIBUTE)
        );
        assert!(npc.beliefs.iter().all(|belief| belief.strength <= 10));
        assert!(npc.goal.is_valid());

        for (&other_id, relationship) in &npc.relationships {
            assert_ne!(other_id, npc.id, "自分自身へのrelationshipを持たない");
            assert!(
                world.npc(other_id).is_some(),
                "relationship先のNpcIdが存在する"
            );
            assert!(relationship.values_in_range());
            assert!(relationship.affinity <= 10);
            assert!(relationship.relation <= 10);
        }

        if let Some(partner_id) = npc.partner {
            let partner = world.npc(partner_id).expect("partnerのNpcIdが存在する");
            assert_eq!(partner.partner, Some(npc.id), "partner参照が相互である");
        }
        for &parent_id in &npc.parents {
            let parent = world.npc(parent_id).expect("親のNpcIdが存在する");
            assert!(
                parent.children.contains(&npc.id),
                "親から子への参照が存在する"
            );
        }
        for &child_id in &npc.children {
            let child = world.npc(child_id).expect("子のNpcIdが存在する");
            assert!(
                child.parents.contains(&npc.id),
                "子から親への参照が存在する"
            );
        }
    }
}

fn assert_generation_turnover(simulation: &Simulation) {
    let cumulative = simulation.world.statistics.cumulative();
    assert!(cumulative.births > 0, "100年間に出生が発生する");
    assert!(cumulative.deaths > 0, "100年間に死亡が発生する");
    assert!(
        simulation.world.total_unique_npcs() > INITIAL_POPULATION,
        "初期世代以外のNPCが登場する"
    );
    assert!(
        simulation.world.npcs[..INITIAL_POPULATION]
            .iter()
            .any(|npc| !npc.alive),
        "初期NPCの死亡後もシミュレーションが継続する"
    );
    assert!(
        simulation.world.npcs[INITIAL_POPULATION..]
            .iter()
            .any(|npc| !npc.parents.is_empty()),
        "親を持つ新世代が出生する"
    );
    assert!(
        simulation.world.npcs[INITIAL_POPULATION..]
            .iter()
            .any(|npc| npc.is_active() && !npc.parents.is_empty()),
        "100年後も新世代が社会を引き継いでいる"
    );
}
