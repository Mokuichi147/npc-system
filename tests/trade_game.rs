#![cfg(feature = "economy-extension")]

use npc_system::economy::Good;
use npc_system::id::TownId;
use npc_system::trade_game::{PlayerAccount, PlayerTradeError, PlayerTradeSide};
use npc_system::{Simulation, SimulationConfig};

fn simulation() -> Simulation {
    Simulation::new(1, 20, 12345, SimulationConfig::default()).expect("simulation should build")
}

#[test]
fn player_can_buy_and_sell_at_the_current_market_price() {
    let mut simulation = simulation();
    let town_id = TownId(0);
    let unit_price = simulation
        .world
        .town(town_id)
        .expect("town should exist")
        .economy
        .good_price(Good::Food);
    let initial_treasury = simulation
        .world
        .town(town_id)
        .expect("town should exist")
        .economy
        .treasury_cents;
    let mut player = PlayerAccount::new(unit_price * 10);

    let purchase = player
        .buy(&mut simulation.world, town_id, Good::Food, 4)
        .expect("purchase should succeed");
    assert_eq!(purchase.side, PlayerTradeSide::Buy);
    assert_eq!(purchase.unit_price_cents, unit_price);
    assert_eq!(purchase.total_cents, unit_price * 4);
    assert_eq!(player.cash_cents, unit_price * 6);
    assert_eq!(player.inventory.quantity(Good::Food), 4);
    assert_eq!(
        simulation
            .world
            .town(town_id)
            .expect("town should exist")
            .economy
            .treasury_cents,
        initial_treasury + unit_price * 4
    );

    let sale = player
        .sell(&mut simulation.world, town_id, Good::Food, 3)
        .expect("sale should succeed");
    assert_eq!(sale.side, PlayerTradeSide::Sell);
    assert_eq!(player.cash_cents, unit_price * 9);
    assert_eq!(player.inventory.quantity(Good::Food), 1);
    assert_eq!(player.trades, vec![purchase, sale]);
}

#[test]
fn rejected_trades_do_not_change_player_or_city_assets() {
    let mut simulation = simulation();
    let town_id = TownId(0);
    let initial_treasury = simulation
        .world
        .town(town_id)
        .expect("town should exist")
        .economy
        .treasury_cents;
    let mut player = PlayerAccount::new(1);
    let initial_player = player.clone();

    let buy_error = player
        .buy(&mut simulation.world, town_id, Good::Luxury, 1)
        .expect_err("purchase should be rejected");
    assert!(matches!(
        buy_error,
        PlayerTradeError::InsufficientCash { .. }
    ));
    assert_eq!(player, initial_player);
    assert_eq!(
        simulation
            .world
            .town(town_id)
            .expect("town should exist")
            .economy
            .treasury_cents,
        initial_treasury
    );

    let sell_error = player
        .sell(&mut simulation.world, town_id, Good::Food, 1)
        .expect_err("sale should be rejected");
    assert!(matches!(
        sell_error,
        PlayerTradeError::InsufficientInventory { .. }
    ));
    assert_eq!(player, initial_player);
}

#[test]
fn final_settlement_converts_inventory_to_market_value() {
    let mut simulation = simulation();
    let town_id = TownId(0);
    let mut player = PlayerAccount::new(100_000);
    player
        .buy(&mut simulation.world, town_id, Good::Tools, 2)
        .expect("purchase should succeed");
    player
        .buy(&mut simulation.world, town_id, Good::Medicine, 3)
        .expect("purchase should succeed");
    let expected = player
        .total_value(&simulation.world, town_id)
        .expect("valuation should succeed");

    let final_cash = player
        .settle_at_market_value(&simulation.world, town_id)
        .expect("settlement should succeed");

    assert_eq!(final_cash, expected);
    assert_eq!(player.cash_cents, expected);
    assert!(
        Good::ALL
            .into_iter()
            .all(|good| player.inventory.quantity(good) == 0)
    );
}
