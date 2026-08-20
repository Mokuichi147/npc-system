use crate::config::{SimulationConfig, clamp_probability};
use crate::id::TownId;
use crate::town::TownDamage;
use serde::{Deserialize, Serialize};

/// 国家・軍隊を個別モデル化しない、都市群に対する抽象的な戦争。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarEvent {
    pub towns: Vec<TownId>,
    pub severity: u8,
    pub remaining_years: u8,
}

impl WarEvent {
    pub fn new(towns: impl IntoIterator<Item = TownId>, severity: u8, remaining_years: u8) -> Self {
        let mut towns: Vec<_> = towns.into_iter().collect();
        towns.sort_unstable();
        towns.dedup();
        Self {
            towns,
            severity: severity.clamp(1, 10),
            remaining_years: remaining_years.max(1),
        }
    }

    pub fn includes(&self, town: TownId) -> bool {
        self.towns.binary_search(&town).is_ok()
    }

    pub fn additional_mortality(&self, config: &SimulationConfig) -> f64 {
        let severity = f64::from(self.severity.min(10)) / 10.0;
        clamp_probability(config.war_max_mortality * severity.powf(1.35))
    }

    pub fn birth_rate_multiplier(&self) -> f64 {
        (1.0 - f64::from(self.severity.min(10)) * 0.07).clamp(0.25, 1.0)
    }

    /// 平時の流出確率に掛ける倍率。
    pub fn emigration_multiplier(&self) -> f64 {
        1.0 + f64::from(self.severity.min(10)) * 0.20
    }

    /// 年間で避難候補にする人口比率。
    pub fn refugee_fraction(&self) -> f64 {
        (f64::from(self.severity.min(10)) * 0.012).clamp(0.0, 0.12)
    }

    pub fn town_damage(&self) -> TownDamage {
        let months = u16::from(self.remaining_years.max(1)).saturating_mul(12);
        TownDamage::new(
            self.severity.min(10).div_ceil(3),
            self.severity.min(10).div_ceil(2),
            0,
            months,
        )
    }

    /// 1年進め、まだ継続中なら true。
    pub fn progress_year(&mut self) -> bool {
        self.remaining_years = self.remaining_years.saturating_sub(1);
        self.remaining_years > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn towns_are_canonical_and_searchable() {
        let war = WarEvent::new([TownId(3), TownId(1), TownId(3)], 5, 2);
        assert_eq!(war.towns, vec![TownId(1), TownId(3)]);
        assert!(war.includes(TownId(3)));
        assert!(!war.includes(TownId(2)));
    }

    #[test]
    fn severe_war_has_stronger_effects() {
        let mild = WarEvent::new([TownId(0), TownId(1)], 1, 1);
        let severe = WarEvent::new([TownId(0), TownId(1)], 10, 1);
        let config = SimulationConfig::default();
        assert!(severe.additional_mortality(&config) > mild.additional_mortality(&config));
        assert!(severe.birth_rate_multiplier() < mild.birth_rate_multiplier());
        assert!(severe.refugee_fraction() > mild.refugee_fraction());
    }

    #[test]
    fn war_duration_does_not_underflow() {
        let mut war = WarEvent::new([TownId(0)], 5, 1);
        assert!(!war.progress_year());
        assert!(!war.progress_year());
    }
}
