use std::collections::BTreeMap;

use crate::config::SimulationConfig;
use crate::economy::{Good, MAX_PRICE_INDEX, MIN_PRICE_INDEX, Money, PRICE_INDEX_BASE};
use crate::extensions::SimulationExtension;
use crate::id::NpcId;
use crate::npc::NpcState;
use crate::statistics::YearStatistics;
use crate::utility::Action;
use crate::world::World;

/// npc-systemの経済拡張。コアのtickへライフサイクルフックで接続する。
pub struct EconomyExtension;

impl SimulationExtension for EconomyExtension {
    const ID: &'static str = "economy";

    fn begin_year(world: &mut World) {
        for town in &mut world.towns {
            town.economy.begin_year();
            let signature = u64::from(town.population_capacity)
                ^ (u64::from(town.jobs) << 32)
                ^ (u64::from(town.safety) << 40)
                ^ (u64::from(town.education) << 48)
                ^ (u64::from(town.wealth) << 56);
            for good in Good::ALL {
                town.economy
                    .markets
                    .entry(good)
                    .or_default()
                    .annual_supply_shock_basis_points =
                    annual_supply_shock_basis_points(world.year, town.id.0, good, signature);
            }
        }
    }

    fn before_npc_death(world: &mut World, deceased: NpcId, candidates: &[NpcId]) {
        let Some(snapshot) = world.npc(deceased) else {
            return;
        };
        let town = snapshot.town;
        let estate = snapshot.money_cents;
        let inventory = snapshot.inventory.iter().collect::<Vec<_>>();
        let heirs = candidates
            .iter()
            .copied()
            .filter(|&id| world.npc(id).is_some_and(|npc| npc.is_active()))
            .collect::<Vec<_>>();

        if let Some(npc) = world.npc_mut(deceased) {
            npc.money_cents = 0;
            for (good, quantity) in &inventory {
                npc.inventory.remove(*good, *quantity);
            }
        }
        if heirs.is_empty() {
            if let Some(town) = world.town_mut(town) {
                town.economy.treasury_cents = town.economy.treasury_cents.saturating_add(estate);
            }
            return;
        }

        let share = estate / heirs.len() as u64;
        let remainder = estate % heirs.len() as u64;
        for (index, heir) in heirs.iter().copied().enumerate() {
            if let Some(npc) = world.npc_mut(heir) {
                npc.money_cents = npc
                    .money_cents
                    .saturating_add(share + u64::from(index == 0) * remainder);
            }
        }
        if let Some(first_heir) = heirs.first().copied() {
            if let Some(npc) = world.npc_mut(first_heir) {
                for (good, quantity) in inventory {
                    npc.inventory.add(good, quantity);
                }
            }
        }
    }

    fn run_month(world: &mut World, config: &SimulationConfig, actions: &[Option<Action>]) {
        let residents_by_town = world.residents_by_town();
        for town_index in 0..world.towns.len() {
            let residents = residents_by_town
                .get(town_index)
                .cloned()
                .unwrap_or_default();
            let (
                town_id,
                jobs,
                safety,
                education,
                wealth,
                price_index,
                productivity,
                neighbor_count,
                jobs_loss,
                safety_loss,
            ) = {
                let town = &world.towns[town_index];
                (
                    town.id,
                    town.effective_jobs(),
                    town.effective_safety(),
                    town.effective_education(),
                    town.effective_wealth(),
                    town.economy.price_index,
                    town.economy.productivity_index,
                    town.neighbors.len(),
                    town.temporary_damage.jobs_loss,
                    town.temporary_damage.safety_loss,
                )
            };
            let mut labor_force = residents
                .iter()
                .copied()
                .filter(|&id| {
                    world
                        .npc(id)
                        .is_some_and(|npc| (18..=69).contains(&npc.age))
                })
                .collect::<Vec<_>>();
            labor_force.sort_by_key(|id| {
                (
                    !matches!(
                        actions.get(id.0 as usize).copied().flatten(),
                        Some(Action::Work)
                    ),
                    *id,
                )
            });
            let employment_rate_bps = (3_500_u32 + u32::from(jobs) * 700).min(9_800);
            let employed_count = if labor_force.is_empty() {
                0
            } else {
                (labor_force.len() * employment_rate_bps as usize / 10_000)
                    .max(usize::from(jobs > 0))
                    .min(labor_force.len())
            };
            let employed = labor_force
                .iter()
                .copied()
                .take(employed_count)
                .collect::<Vec<_>>();

            world.towns[town_index].economy.labor_force = labor_force.len();
            world.towns[town_index].economy.employed = employed_count;

            for id in employed {
                let skill_bonus_bps = world.npc(id).map_or(0, |npc| {
                    (u64::from(npc.attributes.intelligence)
                        + u64::from(npc.attributes.dexterity)
                        + u64::from(npc.attributes.charisma))
                        * 100
                });
                let real_wage = config
                    .base_monthly_wage_cents
                    .saturating_mul(8_500 + skill_bonus_bps)
                    / 10_000;
                let gross_wage =
                    real_wage.saturating_mul(u64::from(price_index)) / u64::from(PRICE_INDEX_BASE);
                let net_wage = gross_wage.saturating_mul(
                    10_000_u64.saturating_sub(u64::from(config.income_tax_basis_points)),
                ) / 10_000;
                let payment = net_wage.min(world.towns[town_index].economy.treasury_cents);
                world.towns[town_index].economy.treasury_cents -= payment;
                world.towns[town_index].economy.annual_output_cents = world.towns[town_index]
                    .economy
                    .annual_output_cents
                    .saturating_add(gross_wage);
                if let Some(npc) = world.npc_mut(id) {
                    npc.money_cents = npc.money_cents.saturating_add(payment);
                }
            }

            let living_cost = world.towns[town_index]
                .economy
                .indexed_price(config.base_monthly_living_cost_cents);
            let mut support = Vec::new();
            for &recipient in &residents {
                let Some(npc) = world.npc(recipient) else {
                    continue;
                };
                if npc.money_cents >= living_cost {
                    continue;
                }
                let donor = npc
                    .partner
                    .into_iter()
                    .chain(npc.parents.iter().copied())
                    .find(|&candidate| {
                        world.npc(candidate).is_some_and(|relative| {
                            relative.is_active()
                                && relative.money_cents > living_cost.saturating_mul(3)
                        })
                    });
                if let Some(donor) = donor {
                    let available = world
                        .npc(donor)
                        .map_or(0, |npc| npc.money_cents.saturating_sub(living_cost * 2));
                    let amount = living_cost.saturating_sub(npc.money_cents).min(available);
                    if amount > 0 {
                        support.push((donor, recipient, amount));
                    }
                }
            }
            for (donor, recipient, amount) in support {
                let _ = world.transfer_money(donor, recipient, amount);
            }

            let mut demand_by_good = BTreeMap::<Good, i128>::new();
            for id in residents.iter().copied() {
                let Some(npc) = world.npc(id) else {
                    continue;
                };
                let month_key = id.0.saturating_add(u32::from(world.month));
                let desired_goods = [
                    (Good::Food, if npc.age < 16 { 3 } else { 5 }),
                    (Good::Clothing, u32::from(month_key % 3 == 0)),
                    (
                        Good::Medicine,
                        if npc.state == NpcState::Sick {
                            2
                        } else {
                            u32::from(month_key % 6 == 0)
                        },
                    ),
                    (Good::Tools, u32::from(npc.is_adult() && month_key % 6 == 1)),
                    (
                        Good::Luxury,
                        u32::from(
                            npc.is_adult()
                                && npc.money_cents > living_cost.saturating_mul(4)
                                && month_key % 4 == 2,
                        ),
                    ),
                ];

                for (good, desired_quantity) in desired_goods {
                    *demand_by_good.entry(good).or_default() += i128::from(desired_quantity);
                    if desired_quantity == 0 {
                        continue;
                    }
                    let unit_price = world.towns[town_index].economy.good_price(good);
                    let available = world.npc(id).map_or(0, |npc| npc.money_cents);
                    let affordable = available
                        .checked_div(unit_price)
                        .map_or(desired_quantity, |quantity| {
                            quantity.min(u64::from(desired_quantity)) as u32
                        });
                    if affordable == 0 {
                        continue;
                    }
                    let payment = unit_price.saturating_mul(u64::from(affordable));
                    if let Some(npc) = world.npc_mut(id) {
                        npc.money_cents -= payment;
                    }
                    let economy = &mut world.towns[town_index].economy;
                    economy.treasury_cents = economy.treasury_cents.saturating_add(payment);
                    economy.record_good_trade(good, u64::from(affordable), payment);
                }
            }

            let productivity_change = i64::from(education) - 5;
            let new_productivity =
                (i64::from(productivity) + productivity_change * 3).clamp(5_000, 20_000) as u32;
            let economy = &mut world.towns[town_index].economy;
            economy.productivity_index = new_productivity;
            for good in Good::ALL {
                let demand = demand_by_good.get(&good).copied().unwrap_or_default();
                let local_supply = match good {
                    Good::Food => {
                        employed_count as i128 * 8 * i128::from(u16::from(safety) + 10) / 15
                    }
                    Good::Clothing => employed_count as i128 * 2 / 3,
                    Good::Medicine => {
                        employed_count as i128 * i128::from(u16::from(education) + 3) / 30
                    }
                    Good::Tools => employed_count as i128 * i128::from(u16::from(jobs) + 3) / 30,
                    Good::Luxury => employed_count as i128 * i128::from(u16::from(wealth) + 2) / 40,
                };
                let trade_access_bps = (3_000_i128 + neighbor_count as i128 * 500).min(5_000);
                let import_supply = if demand == 0 {
                    0
                } else {
                    (demand * trade_access_bps + 9_999) / 10_000
                };
                let base_supply = local_supply.saturating_add(import_supply);
                let sensitivity = match good {
                    Good::Food | Good::Medicine => 125_i128,
                    Good::Clothing | Good::Tools => 100,
                    Good::Luxury => 75,
                };
                let market = economy.markets.entry(good).or_default();
                // 高価格は増産と輸入を促し、低価格では生産を縮小する。
                // 価格が供給へ反応するため、恒常的な不足でも上限へ張り付かない。
                let crisis_supply_shock = match good {
                    Good::Food => -(i32::from(jobs_loss) * 400 + i32::from(safety_loss) * 500),
                    Good::Clothing => -i32::from(jobs_loss) * 300,
                    Good::Medicine => -i32::from(jobs_loss) * 250,
                    Good::Tools => -i32::from(jobs_loss) * 500,
                    Good::Luxury => -(i32::from(jobs_loss) * 350 + i32::from(safety_loss) * 250),
                };
                let supply_shock_factor =
                    (10_000 + market.annual_supply_shock_basis_points + crisis_supply_shock)
                        .clamp(4_000, 16_000);
                let supply = if base_supply == 0 {
                    0
                } else {
                    (base_supply
                        * i128::from(new_productivity)
                        * i128::from(market.price_index)
                        * i128::from(supply_shock_factor)
                        / 1_000_000_000_000)
                        .max(1)
                };
                let monthly_change_bps = if demand == 0 {
                    // 取引のない市場は過去の危機価格を保持せず、基準価格へ戻る。
                    match market.price_index.cmp(&PRICE_INDEX_BASE) {
                        std::cmp::Ordering::Greater => -100,
                        std::cmp::Ordering::Less => 100,
                        std::cmp::Ordering::Equal => 0,
                    }
                } else {
                    let gap_bps = if supply == 0 {
                        15_000
                    } else {
                        ((demand - supply) * 10_000 / supply).clamp(-10_000, 30_000)
                    };
                    let adjustment =
                        gap_bps * i128::from(config.price_adjustment_basis_points) * sensitivity
                            / 100
                            / 10_000;
                    adjustment.clamp(-200, 300)
                };
                let price_change = i128::from(market.price_index) * monthly_change_bps / 10_000;
                market.price_index = (i128::from(market.price_index) + price_change)
                    .clamp(i128::from(MIN_PRICE_INDEX), i128::from(MAX_PRICE_INDEX))
                    as u32;
            }
            economy.update_consumer_price_index();

            debug_assert_eq!(world.towns[town_index].id, town_id);
        }
    }

    fn finish_year(world: &World, statistics: &mut YearStatistics) {
        statistics.town_economies = world.town_economic_statistics();
        statistics.gross_product_cents = statistics
            .town_economies
            .iter()
            .map(|economy| economy.gross_product_cents)
            .fold(0, Money::saturating_add);
        statistics.trade_volume_cents = statistics
            .town_economies
            .iter()
            .map(|economy| economy.trade_volume_cents)
            .fold(0, Money::saturating_add);
        statistics.economic_transactions = statistics
            .town_economies
            .iter()
            .map(|economy| economy.transactions)
            .fold(0, u64::saturating_add);
        statistics.money_transfers = statistics
            .town_economies
            .iter()
            .map(|economy| economy.transfers)
            .fold(0, u64::saturating_add);
    }
}

/// 年単位で持続する商品別供給ショック。Worldの乱数列を消費せず再現可能にする。
fn annual_supply_shock_basis_points(year: u64, town: u16, good: Good, town_signature: u64) -> i32 {
    let amplitude = match good {
        Good::Food => 2_000_u64,
        Good::Clothing => 1_200,
        Good::Medicine => 1_600,
        Good::Tools => 1_400,
        Good::Luxury => 2_200,
    };
    let key = town_signature
        ^ year.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ u64::from(town).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ good.ordinal().wrapping_mul(0x94D0_49BB_1331_11EB);
    let mixed = mix_u64(key);
    let span = amplitude * 2 + 1;
    (mixed % span) as i32 - amplitude as i32
}

fn mix_u64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}
