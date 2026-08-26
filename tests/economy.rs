#![cfg(feature = "economy-extension")]

use npc_system::Simulation;
use npc_system::World;
use npc_system::config::SimulationConfig;
use npc_system::economy::{
    EconomyError, Good, MAX_PRICE_INDEX, MIN_PRICE_INDEX, Money, PRICE_INDEX_BASE,
};
use npc_system::extensions::SimulationExtension;
use npc_system::extensions::economy::EconomyExtension;
use npc_system::id::NpcId;
use npc_system::id::TownId;
use npc_system::town::Town;
use std::collections::BTreeSet;

fn stable_economy_config() -> SimulationConfig {
    let mut config = SimulationConfig::default();
    config.birth_rates.clear();
    config.mortality_rates.clear();
    config.immigration_rate = 0.0;
    config.emigration_rate = 0.0;
    config.internal_migration_monthly_probability = 0.0;
    config.disaster_probability = 0.0;
    config.disease_probability = 0.0;
    config.war_probability = 0.0;
    config
}

fn total_domestic_money(simulation: &Simulation) -> Money {
    let npc_money = simulation
        .world
        .active_npcs
        .iter()
        .filter_map(|&id| simulation.world.npc(id))
        .map(|npc| npc.money_cents)
        .sum::<Money>();
    let treasuries = simulation
        .world
        .towns
        .iter()
        .map(|town| town.economy.treasury_cents)
        .sum::<Money>();
    npc_money + treasuries
}

#[test]
fn purchase_and_transfer_are_atomic_and_move_real_assets() {
    let mut simulation = Simulation::new(1, 2, 7, stable_economy_config()).unwrap();
    let buyer = NpcId(0);
    let seller = NpcId(1);
    simulation.world.npc_mut(buyer).unwrap().money_cents = 10_000;
    simulation.world.npc_mut(seller).unwrap().money_cents = 1_000;
    simulation
        .world
        .npc_mut(seller)
        .unwrap()
        .inventory
        .add(Good::Food, 5);

    let receipt = simulation
        .world
        .purchase(buyer, seller, Good::Food, 3, 2_000)
        .unwrap();
    assert_eq!(receipt.total_cents, 6_000);
    assert_eq!(simulation.world.npc(buyer).unwrap().money_cents, 4_000);
    assert_eq!(simulation.world.npc(seller).unwrap().money_cents, 7_000);
    assert_eq!(
        simulation
            .world
            .npc(buyer)
            .unwrap()
            .inventory
            .quantity(Good::Food),
        3
    );

    let before = simulation.world.npcs.clone();
    assert!(matches!(
        simulation
            .world
            .purchase(buyer, seller, Good::Food, 3, 2_000),
        Err(EconomyError::InsufficientFunds { .. })
    ));
    assert_eq!(
        simulation.world.npcs, before,
        "失敗した購入は状態を変えない"
    );

    simulation
        .world
        .transfer_money(seller, buyer, 1_500)
        .unwrap();
    assert_eq!(simulation.world.npc(buyer).unwrap().money_cents, 5_500);
    assert_eq!(simulation.world.npc(seller).unwrap().money_cents, 5_500);

    let market_receipt = simulation
        .world
        .purchase_at_market_price(buyer, seller, Good::Food, 1)
        .unwrap();
    assert_eq!(market_receipt.total_cents, Good::Food.base_price_cents());
}

#[test]
fn monthly_economy_can_create_inflation_and_deflation_without_creating_money() {
    let mut simulation = Simulation::new(2, 100, 11, stable_economy_config()).unwrap();
    for npc in &mut simulation.world.npcs {
        npc.age = 30;
    }
    simulation.world.towns[0].jobs = 1;
    simulation.world.towns[0].economy.productivity_index = PRICE_INDEX_BASE;
    simulation.world.towns[1].jobs = 10;
    simulation.world.towns[1].economy.productivity_index = PRICE_INDEX_BASE;
    let money_before = total_domestic_money(&simulation);

    let year = simulation.run_year().unwrap().clone();
    let low_employment = &year.town_economies[0];
    let high_employment = &year.town_economies[1];
    assert!(low_employment.inflation_basis_points > 0);
    assert!(high_employment.inflation_basis_points < 0);
    assert!(low_employment.unemployment_basis_points > high_employment.unemployment_basis_points);
    assert!(year.gross_product_cents > 0);
    assert!(year.trade_volume_cents > 0);
    assert!(year.economic_transactions > 0);
    assert_eq!(low_employment.goods.len(), Good::ALL.len());
    assert!(
        low_employment
            .goods
            .iter()
            .map(|good| good.price_index)
            .collect::<BTreeSet<_>>()
            .len()
            > 1,
        "商品ごとの需給により単価指数が分かれる"
    );
    assert!(
        low_employment
            .goods
            .iter()
            .all(|good| good.annual_quantity > 0)
    );
    assert_eq!(total_domestic_money(&simulation), money_before);
}

#[test]
fn empty_town_does_not_report_phantom_inflation() {
    let mut simulation = Simulation::new(2, 1, 19, stable_economy_config()).unwrap();
    let empty_town = simulation
        .world
        .towns
        .iter()
        .find(|town| simulation.world.residents_by_town()[town.id.0 as usize].is_empty())
        .unwrap()
        .id;
    simulation.run_year().unwrap();
    let economy = simulation
        .world
        .town_economic_statistics()
        .into_iter()
        .find(|economy| economy.town == empty_town)
        .unwrap();
    assert_eq!(economy.inflation_basis_points, 0);
}

#[test]
fn abandoned_market_returns_from_a_crisis_price() {
    let mut world = World::empty(1);
    world
        .towns
        .push(Town::new(TownId(0), "Empty", 100, 5, 5, 5, 5, 5));
    for market in world.towns[0].economy.markets.values_mut() {
        market.price_index = MAX_PRICE_INDEX;
    }
    world.towns[0].economy.update_consumer_price_index();

    for month in 1..=12 {
        world.month = month;
        EconomyExtension::run_month(&mut world, &stable_economy_config(), &[]);
    }

    assert!(world.towns[0].economy.price_index < MAX_PRICE_INDEX);
    assert!(
        world.towns[0]
            .economy
            .markets
            .values()
            .all(|market| market.price_index < MAX_PRICE_INDEX)
    );
}

#[test]
fn prices_find_an_interior_equilibrium_over_a_hundred_years() {
    let mut simulation = Simulation::new(5, 500, 12_345, SimulationConfig::default()).unwrap();
    simulation.run(100).unwrap();
    let economies = &simulation.world.statistics.latest().unwrap().town_economies;

    assert!(
        economies
            .iter()
            .all(|economy| economy.price_index < MAX_PRICE_INDEX)
    );
    assert!(
        economies
            .iter()
            .flat_map(|economy| &economy.goods)
            .all(|good| (MIN_PRICE_INDEX..MAX_PRICE_INDEX).contains(&good.price_index))
    );

    let annual_changes = simulation
        .world
        .statistics
        .years
        .iter()
        .flat_map(|year| &year.town_economies)
        .flat_map(|economy| &economy.goods)
        .map(|good| good.inflation_basis_points.unsigned_abs() as u64)
        .collect::<Vec<_>>();
    let average_change = annual_changes.iter().sum::<u64>() / annual_changes.len() as u64;
    assert!(
        average_change >= 250,
        "商品価格の平均年次変動が小さすぎます: {average_change}bp"
    );
    assert!(
        simulation
            .world
            .statistics
            .years
            .iter()
            .flat_map(|year| &year.town_economies)
            .flat_map(|economy| &economy.goods)
            .any(|good| good.supply_shock_basis_points < 0)
    );
    assert!(
        simulation
            .world
            .statistics
            .years
            .iter()
            .flat_map(|year| &year.town_economies)
            .flat_map(|economy| &economy.goods)
            .any(|good| good.supply_shock_basis_points > 0)
    );
}
