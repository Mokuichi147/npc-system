use crate::belief::Belief;
use crate::id::TownId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// 都市間の疎なグラフを構成する辺。距離は `1..=10`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TownConnection {
    pub destination: TownId,
    pub distance: u8,
}

impl TownConnection {
    pub fn new(destination: TownId, distance: u8) -> Self {
        Self {
            destination,
            distance: distance.clamp(1, 10),
        }
    }
}

/// 災害・疫病・戦争などによる一時的な都市機能の低下。
///
/// 基礎パラメータそのものは変更せず、`effective_*` でこの値を差し引く。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TownDamage {
    pub jobs_loss: u8,
    pub safety_loss: u8,
    pub capacity_loss: u32,
    pub remaining_months: u16,
}

impl TownDamage {
    pub fn new(jobs_loss: u8, safety_loss: u8, capacity_loss: u32, remaining_months: u16) -> Self {
        Self {
            jobs_loss: jobs_loss.min(10),
            safety_loss: safety_loss.min(10),
            capacity_loss,
            remaining_months,
        }
    }

    pub const fn is_active(&self) -> bool {
        self.remaining_months > 0
            && (self.jobs_loss > 0 || self.safety_loss > 0 || self.capacity_loss > 0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Town {
    pub id: TownId,
    pub name: String,
    pub population_capacity: u32,
    pub jobs: u8,
    pub safety: u8,
    pub education: u8,
    pub freedom: u8,
    pub wealth: u8,
    pub culture: Vec<Belief>,
    pub neighbors: Vec<TownConnection>,
    #[serde(default)]
    pub temporary_damage: TownDamage,
}

impl Town {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: TownId,
        name: impl Into<String>,
        population_capacity: u32,
        jobs: u8,
        safety: u8,
        education: u8,
        freedom: u8,
        wealth: u8,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            population_capacity: population_capacity.max(1),
            jobs: jobs.min(10),
            safety: safety.min(10),
            education: education.min(10),
            freedom: freedom.min(10),
            wealth: wealth.min(10),
            culture: Vec::new(),
            neighbors: Vec::new(),
            temporary_damage: TownDamage::default(),
        }
    }

    /// 公開フィールドを直接編集した後に不変条件を戻す。
    pub fn normalize(&mut self) {
        self.population_capacity = self.population_capacity.max(1);
        self.jobs = self.jobs.min(10);
        self.safety = self.safety.min(10);
        self.education = self.education.min(10);
        self.freedom = self.freedom.min(10);
        self.wealth = self.wealth.min(10);
        self.temporary_damage.jobs_loss = self.temporary_damage.jobs_loss.min(self.jobs);
        self.temporary_damage.safety_loss = self.temporary_damage.safety_loss.min(self.safety);
        self.temporary_damage.capacity_loss = self
            .temporary_damage
            .capacity_loss
            .min(self.population_capacity.saturating_sub(1));
        self.neighbors
            .retain(|connection| connection.destination != self.id);
        for connection in &mut self.neighbors {
            connection.distance = connection.distance.clamp(1, 10);
        }
        self.neighbors
            .sort_by_key(|connection| connection.destination);
        self.neighbors
            .dedup_by_key(|connection| connection.destination);
    }

    pub fn effective_jobs(&self) -> u8 {
        self.jobs
            .min(10)
            .saturating_sub(self.temporary_damage.jobs_loss)
    }

    pub fn effective_safety(&self) -> u8 {
        self.safety
            .min(10)
            .saturating_sub(self.temporary_damage.safety_loss)
    }

    pub fn effective_education(&self) -> u8 {
        self.education.min(10)
    }

    pub fn effective_freedom(&self) -> u8 {
        self.freedom.min(10)
    }

    pub fn effective_wealth(&self) -> u8 {
        self.wealth.min(10)
    }

    pub fn effective_capacity(&self) -> u32 {
        self.population_capacity
            .max(1)
            .saturating_sub(self.temporary_damage.capacity_loss)
            .max(1)
    }

    pub fn effective_population_capacity(&self) -> u32 {
        self.effective_capacity()
    }

    /// 現在の（一時ダメージ反映後の）収容力に対する人口比。
    pub fn occupancy(&self, population: usize) -> f64 {
        population as f64 / f64::from(self.effective_capacity())
    }

    /// 都市の基礎品質から過密・極端な過疎の負のフィードバックを差し引く。
    /// 戻り値は `0.0..=10.0`。
    pub fn attractiveness(&self, population: usize) -> f64 {
        let quality = (f64::from(self.effective_jobs()) * 1.25
            + f64::from(self.effective_safety()) * 1.40
            + f64::from(self.effective_education()) * 0.85
            + f64::from(self.effective_freedom()) * 0.60
            + f64::from(self.effective_wealth()) * 0.90)
            / 5.0;

        let occupancy = self.occupancy(population);
        let occupancy_penalty = if occupancy < 0.15 {
            // 人口ゼロでも最大1点だけにし、過疎都市への再流入余地を残す。
            (0.15 - occupancy) / 0.15
        } else if occupancy < 0.70 {
            0.0
        } else if occupancy < 1.0 {
            (occupancy - 0.70) / 0.30 * 2.0
        } else if occupancy < 1.20 {
            2.0 + (occupancy - 1.0) / 0.20 * 3.0
        } else {
            (5.0 + (occupancy - 1.20) * 5.0).min(9.0)
        };

        (quality - occupancy_penalty).clamp(0.0, 10.0)
    }

    /// 同じ接続先がある場合は短い距離を採用する。
    pub fn add_neighbor(&mut self, connection: TownConnection) {
        if connection.destination == self.id {
            return;
        }
        let connection = TownConnection::new(connection.destination, connection.distance);
        if let Some(existing) = self
            .neighbors
            .iter_mut()
            .find(|existing| existing.destination == connection.destination)
        {
            existing.distance = existing.distance.min(connection.distance);
        } else {
            self.neighbors.push(connection);
            self.neighbors.sort_by_key(|edge| edge.destination);
        }
    }

    pub fn connection_to(&self, destination: TownId) -> Option<&TownConnection> {
        self.neighbors
            .iter()
            .find(|connection| connection.destination == destination)
    }

    /// 全都市を候補化せず、直接接続と2-hopの都市だけを返す。
    pub fn reachable_within_two_hops(&self, towns: &[Town]) -> Vec<TownId> {
        let mut destinations = BTreeSet::new();
        for connection in &self.neighbors {
            destinations.insert(connection.destination);
            if let Some(neighbor) = towns.iter().find(|town| town.id == connection.destination) {
                for second_hop in &neighbor.neighbors {
                    if second_hop.destination != self.id {
                        destinations.insert(second_hop.destination);
                    }
                }
            }
        }
        destinations.into_iter().collect()
    }

    pub fn apply_damage(&mut self, damage: TownDamage) {
        if !damage.is_active() {
            return;
        }
        self.temporary_damage.jobs_loss = self
            .temporary_damage
            .jobs_loss
            .saturating_add(damage.jobs_loss)
            .min(self.jobs.min(10));
        self.temporary_damage.safety_loss = self
            .temporary_damage
            .safety_loss
            .saturating_add(damage.safety_loss)
            .min(self.safety.min(10));
        self.temporary_damage.capacity_loss = self
            .temporary_damage
            .capacity_loss
            .saturating_add(damage.capacity_loss)
            .min(self.population_capacity.max(1).saturating_sub(1));
        self.temporary_damage.remaining_months = self
            .temporary_damage
            .remaining_months
            .max(damage.remaining_months);
    }

    /// 1か月分回復させる。戻り値は回復後もダメージが残っているか。
    pub fn recover(&mut self) -> bool {
        if !self.temporary_damage.is_active() {
            self.temporary_damage = TownDamage::default();
            return false;
        }
        self.temporary_damage.remaining_months =
            self.temporary_damage.remaining_months.saturating_sub(1);
        if self.temporary_damage.remaining_months == 0 {
            self.temporary_damage = TownDamage::default();
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn town(id: u16) -> Town {
        Town::new(TownId(id), format!("Town {id}"), 100, 8, 8, 6, 6, 7)
    }

    #[test]
    fn constructor_clamps_town_values() {
        let town = Town::new(TownId(0), "A", 0, 99, 99, 99, 99, 99);
        assert_eq!(town.population_capacity, 1);
        assert_eq!(town.jobs, 10);
        assert_eq!(town.wealth, 10);
    }

    #[test]
    fn damage_is_temporary_and_recovers() {
        let mut town = town(0);
        town.apply_damage(TownDamage::new(4, 3, 40, 4));
        assert_eq!(town.effective_jobs(), 4);
        assert_eq!(town.effective_safety(), 5);
        assert_eq!(town.effective_capacity(), 60);

        for _ in 0..4 {
            town.recover();
        }
        assert_eq!(town.effective_jobs(), 8);
        assert_eq!(town.effective_safety(), 8);
        assert_eq!(town.effective_capacity(), 100);
    }

    #[test]
    fn overcrowding_reduces_attractiveness() {
        let town = town(0);
        assert!(town.attractiveness(60) > town.attractiveness(100));
        assert!(town.attractiveness(100) > town.attractiveness(150));
        assert!(town.occupancy(150) > 1.0);
    }

    #[test]
    fn graph_candidates_are_limited_to_two_hops() {
        let mut a = town(0);
        let mut b = town(1);
        let mut c = town(2);
        let d = town(3);
        a.add_neighbor(TownConnection::new(TownId(1), 3));
        b.add_neighbor(TownConnection::new(TownId(2), 3));
        c.add_neighbor(TownConnection::new(TownId(3), 3));

        let candidates = a.reachable_within_two_hops(&[a.clone(), b, c, d]);
        assert_eq!(candidates, vec![TownId(1), TownId(2)]);
    }

    #[test]
    fn duplicate_connection_keeps_shorter_distance() {
        let mut town = town(0);
        town.add_neighbor(TownConnection::new(TownId(1), 8));
        town.add_neighbor(TownConnection::new(TownId(1), 2));
        assert_eq!(town.neighbors.len(), 1);
        assert_eq!(town.neighbors[0].distance, 2);
    }
}
