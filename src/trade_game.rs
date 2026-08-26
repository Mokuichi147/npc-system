use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::economy::{Good, Inventory, Money};
use crate::id::TownId;
use crate::world::World;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerTradeSide {
    Buy,
    Sell,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerTradeReceipt {
    pub side: PlayerTradeSide,
    pub town: TownId,
    pub good: Good,
    pub quantity: u32,
    pub unit_price_cents: Money,
    pub total_cents: Money,
}

/// NPCとは独立したゲームプレイヤーの現金・商品口座。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerAccount {
    pub starting_cash_cents: Money,
    pub cash_cents: Money,
    pub inventory: Inventory,
    pub trades: Vec<PlayerTradeReceipt>,
}

impl PlayerAccount {
    pub fn new(starting_cash_cents: Money) -> Self {
        Self {
            starting_cash_cents,
            cash_cents: starting_cash_cents,
            inventory: Inventory::default(),
            trades: Vec::new(),
        }
    }

    pub fn buy(
        &mut self,
        world: &mut World,
        town_id: TownId,
        good: Good,
        quantity: u32,
    ) -> Result<PlayerTradeReceipt, PlayerTradeError> {
        if quantity == 0 {
            return Err(PlayerTradeError::ZeroQuantity);
        }
        let town = world
            .town(town_id)
            .ok_or(PlayerTradeError::InvalidTown(town_id))?;
        let unit_price_cents = town.economy.good_price(good);
        let total_cents = unit_price_cents
            .checked_mul(Money::from(quantity))
            .ok_or(PlayerTradeError::AmountOverflow)?;
        if self.cash_cents < total_cents {
            return Err(PlayerTradeError::InsufficientCash {
                required: total_cents,
                available: self.cash_cents,
            });
        }
        if self
            .inventory
            .quantity(good)
            .checked_add(quantity)
            .is_none()
            || town
                .economy
                .treasury_cents
                .checked_add(total_cents)
                .is_none()
        {
            return Err(PlayerTradeError::AmountOverflow);
        }

        self.cash_cents -= total_cents;
        self.inventory.add(good, quantity);
        let town = world
            .town_mut(town_id)
            .expect("town existence was validated");
        town.economy.treasury_cents += total_cents;
        town.economy
            .record_good_trade(good, Money::from(quantity), total_cents);
        let receipt = PlayerTradeReceipt {
            side: PlayerTradeSide::Buy,
            town: town_id,
            good,
            quantity,
            unit_price_cents,
            total_cents,
        };
        self.trades.push(receipt.clone());
        Ok(receipt)
    }

    pub fn sell(
        &mut self,
        world: &mut World,
        town_id: TownId,
        good: Good,
        quantity: u32,
    ) -> Result<PlayerTradeReceipt, PlayerTradeError> {
        if quantity == 0 {
            return Err(PlayerTradeError::ZeroQuantity);
        }
        let town = world
            .town(town_id)
            .ok_or(PlayerTradeError::InvalidTown(town_id))?;
        let available = self.inventory.quantity(good);
        if available < quantity {
            return Err(PlayerTradeError::InsufficientInventory {
                good,
                required: quantity,
                available,
            });
        }
        let unit_price_cents = town.economy.good_price(good);
        let total_cents = unit_price_cents
            .checked_mul(Money::from(quantity))
            .ok_or(PlayerTradeError::AmountOverflow)?;
        if town.economy.treasury_cents < total_cents {
            return Err(PlayerTradeError::InsufficientMarketFunds {
                town: town_id,
                required: total_cents,
                available: town.economy.treasury_cents,
            });
        }
        if self.cash_cents.checked_add(total_cents).is_none() {
            return Err(PlayerTradeError::AmountOverflow);
        }

        let removed = self.inventory.remove(good, quantity);
        debug_assert!(removed);
        self.cash_cents += total_cents;
        let town = world
            .town_mut(town_id)
            .expect("town existence was validated");
        town.economy.treasury_cents -= total_cents;
        town.economy
            .record_good_trade(good, Money::from(quantity), total_cents);
        let receipt = PlayerTradeReceipt {
            side: PlayerTradeSide::Sell,
            town: town_id,
            good,
            quantity,
            unit_price_cents,
            total_cents,
        };
        self.trades.push(receipt.clone());
        Ok(receipt)
    }

    pub fn inventory_market_value(
        &self,
        world: &World,
        town_id: TownId,
    ) -> Result<Money, PlayerTradeError> {
        let town = world
            .town(town_id)
            .ok_or(PlayerTradeError::InvalidTown(town_id))?;
        Ok(self.inventory.iter().fold(0, |value, (good, quantity)| {
            value.saturating_add(
                town.economy
                    .good_price(good)
                    .saturating_mul(Money::from(quantity)),
            )
        }))
    }

    pub fn total_value(&self, world: &World, town_id: TownId) -> Result<Money, PlayerTradeError> {
        Ok(self
            .cash_cents
            .saturating_add(self.inventory_market_value(world, town_id)?))
    }

    /// 最終成績用に全商品を現在の市場価格で決済する。都市財政は変更しない。
    pub fn settle_at_market_value(
        &mut self,
        world: &World,
        town_id: TownId,
    ) -> Result<Money, PlayerTradeError> {
        let final_cash = self.total_value(world, town_id)?;
        let inventory = self.inventory.iter().collect::<Vec<_>>();
        for (good, quantity) in inventory {
            self.inventory.remove(good, quantity);
        }
        self.cash_cents = final_cash;
        Ok(final_cash)
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PlayerTradeError {
    #[error("存在しない都市IDです: {0:?}")]
    InvalidTown(TownId),
    #[error("数量は1以上である必要があります")]
    ZeroQuantity,
    #[error("現金が不足しています: 必要 {required}、所持 {available}")]
    InsufficientCash { required: Money, available: Money },
    #[error("{good:?} の保有数が不足しています: 必要 {required}、保有 {available}")]
    InsufficientInventory {
        good: Good,
        required: u32,
        available: u32,
    },
    #[error("都市 {town:?} の市場資金が不足しています: 必要 {required}、市場 {available}")]
    InsufficientMarketFunds {
        town: TownId,
        required: Money,
        available: Money,
    },
    #[error("取引金額が上限を超えました")]
    AmountOverflow,
}
