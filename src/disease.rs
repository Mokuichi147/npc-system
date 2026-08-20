use crate::config::{SimulationConfig, clamp_probability};
use crate::id::TownId;
use crate::town::TownDamage;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// 複数都市へ伝播し得る感染症イベント。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiseaseEvent {
    pub severity: u8,
    pub infected_towns: HashSet<TownId>,
    pub remaining_months: u8,
}

impl DiseaseEvent {
    pub fn new(severity: u8, initial_town: TownId, remaining_months: u8) -> Self {
        Self {
            severity: severity.clamp(1, 10),
            infected_towns: HashSet::from([initial_town]),
            remaining_months: remaining_months.max(1),
        }
    }

    pub fn from_infected_towns(
        severity: u8,
        infected_towns: impl IntoIterator<Item = TownId>,
        remaining_months: u8,
    ) -> Self {
        Self {
            severity: severity.clamp(1, 10),
            infected_towns: infected_towns.into_iter().collect(),
            remaining_months: remaining_months.max(1),
        }
    }

    /// 新しく感染した都市なら true。
    pub fn infect(&mut self, town: TownId) -> bool {
        self.infected_towns.insert(town)
    }

    pub fn is_infected(&self, town: TownId) -> bool {
        self.infected_towns.contains(&town)
    }

    /// HashSetの反復順に乱数消費順を依存させないための安定順ビュー。
    pub fn infected_towns_sorted(&self) -> Vec<TownId> {
        let mut towns: Vec<_> = self.infected_towns.iter().copied().collect();
        towns.sort_unstable();
        towns
    }

    /// 接続1辺を越えて伝播する月次確率。
    pub fn spread_probability(
        &self,
        source_occupancy: f64,
        destination_occupancy: f64,
        distance: u8,
        config: &SimulationConfig,
    ) -> f64 {
        if self.remaining_months == 0 {
            return 0.0;
        }
        let density = safe_occupancy(source_occupancy)
            .max(safe_occupancy(destination_occupancy))
            .clamp(0.25, 1.75);
        let severity = 0.5 + f64::from(self.severity.min(10)) / 10.0;
        let distance_factor = 1.0 / f64::from(distance.clamp(1, 10)).sqrt();
        clamp_probability(
            config.disease_spread_monthly_probability * density * severity * distance_factor,
        )
    }

    /// 疫病による年次追加死亡率。年齢による脆弱性を含む。
    pub fn additional_mortality(&self, age: u8, config: &SimulationConfig) -> f64 {
        let age_factor = match age {
            0..=4 => 1.20,
            5..=49 => 0.65,
            50..=69 => 1.00,
            _ => 1.50,
        };
        let severity = f64::from(self.severity.min(10)) / 10.0;
        clamp_probability(config.disease_max_mortality * severity.powf(1.4) * age_factor)
    }

    pub fn mobility_multiplier(&self) -> f64 {
        (1.0 - f64::from(self.severity.min(10)) * 0.06).clamp(0.35, 1.0)
    }

    pub fn birth_rate_multiplier(&self) -> f64 {
        (1.0 - f64::from(self.severity.min(10)) * 0.08).clamp(0.20, 1.0)
    }

    pub fn job_penalty(&self) -> u8 {
        self.severity.min(10).div_ceil(3)
    }

    pub fn town_damage(&self) -> TownDamage {
        TownDamage::new(self.job_penalty(), 0, 0, u16::from(self.remaining_months))
    }

    /// 1か月進め、まだ継続中なら true。
    pub fn progress_month(&mut self) -> bool {
        self.remaining_months = self.remaining_months.saturating_sub(1);
        self.remaining_months > 0
    }
}

fn safe_occupancy(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disease_spreads_without_duplicate_towns() {
        let mut disease = DiseaseEvent::new(5, TownId(2), 12);
        assert!(!disease.infect(TownId(2)));
        assert!(disease.infect(TownId(1)));
        assert_eq!(disease.infected_towns_sorted(), vec![TownId(1), TownId(2)]);
    }

    #[test]
    fn density_and_distance_affect_spread() {
        let disease = DiseaseEvent::new(8, TownId(0), 12);
        let config = SimulationConfig::default();
        let crowded_near = disease.spread_probability(1.4, 1.2, 1, &config);
        let sparse_far = disease.spread_probability(0.4, 0.5, 10, &config);
        assert!(crowded_near > sparse_far);
    }

    #[test]
    fn old_people_have_more_additional_mortality() {
        let disease = DiseaseEvent::new(7, TownId(0), 12);
        let config = SimulationConfig::default();
        assert!(
            disease.additional_mortality(80, &config) > disease.additional_mortality(30, &config)
        );
    }

    #[test]
    fn disease_expires_safely() {
        let mut disease = DiseaseEvent::new(3, TownId(0), 1);
        assert!(!disease.progress_month());
        assert!(!disease.progress_month());
        assert_eq!(
            disease.spread_probability(1.0, 1.0, 1, &SimulationConfig::default()),
            0.0
        );
    }
}
