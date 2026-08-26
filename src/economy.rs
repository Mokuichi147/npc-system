use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::id::{NpcId, TownId};

/// 通貨は浮動小数点の丸め誤差を避けるため、最小単位（1/100通貨）で保持する。
pub type Money = u64;

pub const PRICE_INDEX_BASE: u32 = 10_000;
pub const MIN_PRICE_INDEX: u32 = 2_500;
pub const MAX_PRICE_INDEX: u32 = 40_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Good {
    Food,
    Clothing,
    Medicine,
    Tools,
    Luxury,
}

impl Good {
    pub const ALL: [Self; 5] = [
        Self::Food,
        Self::Clothing,
        Self::Medicine,
        Self::Tools,
        Self::Luxury,
    ];

    pub const fn base_price_cents(self) -> Money {
        match self {
            Self::Food => 1_200,
            Self::Clothing => 4_000,
            Self::Medicine => 3_000,
            Self::Tools => 6_000,
            Self::Luxury => 12_000,
        }
    }

    /// 総合物価指数を作る際の消費ウェイト（合計10000）。
    pub const fn consumer_weight(self) -> u32 {
        match self {
            Self::Food => 5_000,
            Self::Clothing => 1_500,
            Self::Medicine => 1_500,
            Self::Tools => 1_500,
            Self::Luxury => 500,
        }
    }

    pub const fn ordinal(self) -> u64 {
        match self {
            Self::Food => 0,
            Self::Clothing => 1,
            Self::Medicine => 2,
            Self::Tools => 3,
            Self::Luxury => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommodityMarket {
    pub price_index: u32,
    pub previous_year_price_index: u32,
    pub annual_quantity: u64,
    pub annual_trade_volume_cents: Money,
    /// 100bp = 1%。負値は不作・供給障害、正値は豊作・技術改善を表す。
    #[serde(default)]
    pub annual_supply_shock_basis_points: i32,
}

impl CommodityMarket {
    fn new() -> Self {
        Self {
            price_index: PRICE_INDEX_BASE,
            previous_year_price_index: PRICE_INDEX_BASE,
            annual_quantity: 0,
            annual_trade_volume_cents: 0,
            annual_supply_shock_basis_points: 0,
        }
    }

    pub fn inflation_basis_points(&self) -> i32 {
        price_change_basis_points(self.price_index, self.previous_year_price_index)
    }
}

impl Default for CommodityMarket {
    fn default() -> Self {
        Self::new()
    }
}

fn default_markets() -> BTreeMap<Good, CommodityMarket> {
    Good::ALL
        .into_iter()
        .map(|good| (good, CommodityMarket::new()))
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    goods: BTreeMap<Good, u32>,
}

impl Inventory {
    pub fn quantity(&self, good: Good) -> u32 {
        self.goods.get(&good).copied().unwrap_or_default()
    }

    pub fn add(&mut self, good: Good, quantity: u32) {
        if quantity == 0 {
            return;
        }
        let entry = self.goods.entry(good).or_default();
        *entry = entry.saturating_add(quantity);
    }

    pub fn remove(&mut self, good: Good, quantity: u32) -> bool {
        if quantity == 0 {
            return true;
        }
        let Some(current) = self.goods.get_mut(&good) else {
            return false;
        };
        if *current < quantity {
            return false;
        }
        *current -= quantity;
        if *current == 0 {
            self.goods.remove(&good);
        }
        true
    }

    pub fn iter(&self) -> impl Iterator<Item = (Good, u32)> + '_ {
        self.goods.iter().map(|(&good, &quantity)| (good, quantity))
    }
}

/// 都市が保持する市場・財政状態。年次フローは年初にリセットされる。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TownEconomy {
    pub treasury_cents: Money,
    /// 10000を基準とする消費者物価指数。
    pub price_index: u32,
    pub previous_year_price_index: u32,
    /// 10000を基準とする生産性。
    pub productivity_index: u32,
    pub annual_output_cents: Money,
    pub annual_trade_volume_cents: Money,
    pub annual_transactions: u64,
    pub annual_transfers: u64,
    pub labor_force: usize,
    pub employed: usize,
    #[serde(default = "default_markets")]
    pub markets: BTreeMap<Good, CommodityMarket>,
}

impl TownEconomy {
    pub fn new(wealth: u8, population_capacity: u32) -> Self {
        let wealth = wealth.min(10);
        Self {
            treasury_cents: Money::from(population_capacity)
                .saturating_mul(30_000 + Money::from(wealth) * 5_000),
            price_index: PRICE_INDEX_BASE,
            previous_year_price_index: PRICE_INDEX_BASE,
            productivity_index: 7_500 + u32::from(wealth) * 500,
            annual_output_cents: 0,
            annual_trade_volume_cents: 0,
            annual_transactions: 0,
            annual_transfers: 0,
            labor_force: 0,
            employed: 0,
            markets: default_markets(),
        }
    }

    pub fn begin_year(&mut self) {
        self.previous_year_price_index = self.price_index.max(1);
        self.annual_output_cents = 0;
        self.annual_trade_volume_cents = 0;
        self.annual_transactions = 0;
        self.annual_transfers = 0;
        self.labor_force = 0;
        self.employed = 0;
        for market in self.markets.values_mut() {
            market.previous_year_price_index = market.price_index.max(1);
            market.annual_quantity = 0;
            market.annual_trade_volume_cents = 0;
        }
    }

    pub fn indexed_price(&self, base_cents: Money) -> Money {
        base_cents
            .saturating_mul(Money::from(self.price_index))
            .div_ceil(Money::from(PRICE_INDEX_BASE))
    }

    pub fn inflation_basis_points(&self) -> i32 {
        price_change_basis_points(self.price_index, self.previous_year_price_index)
    }

    pub fn good_price_index(&self, good: Good) -> u32 {
        self.markets
            .get(&good)
            .map_or(PRICE_INDEX_BASE, |market| market.price_index)
    }

    pub fn good_price(&self, good: Good) -> Money {
        good.base_price_cents()
            .saturating_mul(Money::from(self.good_price_index(good)))
            .div_ceil(Money::from(PRICE_INDEX_BASE))
    }

    pub fn record_good_trade(&mut self, good: Good, quantity: u64, volume_cents: Money) {
        let market = self.markets.entry(good).or_default();
        market.annual_quantity = market.annual_quantity.saturating_add(quantity);
        market.annual_trade_volume_cents = market
            .annual_trade_volume_cents
            .saturating_add(volume_cents);
        self.annual_trade_volume_cents =
            self.annual_trade_volume_cents.saturating_add(volume_cents);
        self.annual_transactions = self.annual_transactions.saturating_add(1);
    }

    pub fn update_consumer_price_index(&mut self) {
        let weighted = Good::ALL
            .into_iter()
            .map(|good| u64::from(self.good_price_index(good)) * u64::from(good.consumer_weight()))
            .sum::<u64>();
        self.price_index = (weighted / 10_000)
            .clamp(u64::from(MIN_PRICE_INDEX), u64::from(MAX_PRICE_INDEX))
            as u32;
    }

    pub fn unemployment_basis_points(&self) -> u32 {
        if self.labor_force == 0 {
            return 0;
        }
        let unemployed = self.labor_force.saturating_sub(self.employed);
        ((unemployed as u128 * 10_000) / self.labor_force as u128) as u32
    }
}

fn price_change_basis_points(current: u32, previous: u32) -> i32 {
    let previous = i64::from(previous.max(1));
    let change = i64::from(current) - previous;
    (change.saturating_mul(10_000) / previous).clamp(i64::from(i32::MIN), i64::from(i32::MAX))
        as i32
}

impl Default for TownEconomy {
    fn default() -> Self {
        Self::new(5, 1)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionReceipt {
    pub buyer: NpcId,
    pub seller: NpcId,
    pub good: Good,
    pub quantity: u32,
    pub total_cents: Money,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferReceipt {
    pub from: NpcId,
    pub to: NpcId,
    pub amount_cents: Money,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TownEconomicStatistics {
    pub town: TownId,
    pub gross_product_cents: Money,
    pub trade_volume_cents: Money,
    pub transactions: u64,
    pub transfers: u64,
    pub resident_wealth_cents: Money,
    pub treasury_cents: Money,
    /// GDP・住民資産・都市財政を合わせた比較用指標。
    pub economic_power_cents: Money,
    pub price_index: u32,
    /// 100bp = 1%。負値はデフレを表す。
    pub inflation_basis_points: i32,
    pub labor_force: usize,
    pub employed: usize,
    pub unemployment_basis_points: u32,
    /// 0（完全平等）..=10000（最大格差）。
    pub gini_basis_points: u32,
    pub goods: Vec<GoodPriceStatistics>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoodPriceStatistics {
    pub good: Good,
    pub unit_price_cents: Money,
    pub price_index: u32,
    pub inflation_basis_points: i32,
    pub annual_quantity: u64,
    pub annual_trade_volume_cents: Money,
    #[serde(default)]
    pub supply_shock_basis_points: i32,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum EconomyError {
    #[error("存在しないNPC ID: {0:?}")]
    InvalidNpc(NpcId),
    #[error("取引できないNPCです: {0:?}")]
    InactiveNpc(NpcId),
    #[error("自分自身とは取引できません: {0:?}")]
    SameNpc(NpcId),
    #[error("数量または金額は1以上である必要があります")]
    ZeroAmount,
    #[error("NPC {npc:?} の残高不足: 必要 {required}、残高 {available}")]
    InsufficientFunds {
        npc: NpcId,
        required: Money,
        available: Money,
    },
    #[error("NPC {npc:?} の在庫不足: {good:?} が必要 {required}、在庫 {available}")]
    InsufficientInventory {
        npc: NpcId,
        good: Good,
        required: u32,
        available: u32,
    },
    #[error("取引金額が上限を超えました")]
    AmountOverflow,
}

/// 整数演算によるGini係数。入力順に依存しない。
pub fn gini_basis_points(values: &[Money]) -> u32 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let total = sorted.iter().map(|&value| value as u128).sum::<u128>();
    if total == 0 {
        return 0;
    }
    let n = sorted.len() as u128;
    let weighted = sorted
        .iter()
        .enumerate()
        .map(|(index, &value)| (index as u128 + 1) * value as u128)
        .sum::<u128>();
    let numerator = weighted.saturating_mul(2);
    let adjustment = n.saturating_add(1).saturating_mul(total);
    if numerator <= adjustment {
        return 0;
    }
    (((numerator - adjustment) * 10_000) / (n * total)).min(10_000) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_adds_and_removes_without_underflow() {
        let mut inventory = Inventory::default();
        inventory.add(Good::Food, 3);
        assert!(!inventory.remove(Good::Food, 4));
        assert!(inventory.remove(Good::Food, 2));
        assert_eq!(inventory.quantity(Good::Food), 1);
    }

    #[test]
    fn gini_distinguishes_equal_and_unequal_distributions() {
        assert_eq!(gini_basis_points(&[100, 100, 100]), 0);
        assert!(gini_basis_points(&[0, 0, 300]) > 6_000);
    }

    #[test]
    fn inflation_supports_positive_and_negative_changes() {
        let mut economy = TownEconomy {
            previous_year_price_index: 10_000,
            price_index: 10_500,
            ..TownEconomy::default()
        };
        assert_eq!(economy.inflation_basis_points(), 500);
        economy.price_index = 9_500;
        assert_eq!(economy.inflation_basis_points(), -500);
    }

    #[test]
    fn consumer_price_index_is_weighted_from_independent_good_prices() {
        let mut economy = TownEconomy::default();
        economy.markets.get_mut(&Good::Food).unwrap().price_index = 12_000;
        economy.markets.get_mut(&Good::Luxury).unwrap().price_index = 8_000;
        economy.update_consumer_price_index();
        assert_eq!(economy.price_index, 10_900);
        assert_eq!(economy.good_price(Good::Food), 1_440);
        assert_eq!(economy.good_price(Good::Luxury), 9_600);
    }
}
