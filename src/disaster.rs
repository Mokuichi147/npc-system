use crate::config::{SimulationConfig, clamp_probability};
use crate::id::TownId;
use crate::town::TownDamage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NaturalDisaster {
    Earthquake,
    Flood,
    Fire,
    Storm,
}

impl NaturalDisaster {
    pub const ALL: [Self; 4] = [Self::Earthquake, Self::Flood, Self::Fire, Self::Storm];
}

/// 1都市で発生した自然災害。severity は常に `1..=10`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NaturalDisasterEvent {
    pub kind: NaturalDisaster,
    pub town: TownId,
    pub severity: u8,
    pub remaining_months: u16,
}

impl NaturalDisasterEvent {
    pub fn new(kind: NaturalDisaster, town: TownId, severity: u8) -> Self {
        let severity = severity.clamp(1, 10);
        Self {
            kind,
            town,
            severity,
            remaining_months: 6 + u16::from(severity) * 2,
        }
    }

    pub fn with_duration(
        kind: NaturalDisaster,
        town: TownId,
        severity: u8,
        remaining_months: u16,
    ) -> Self {
        let mut event = Self::new(kind, town, severity);
        event.remaining_months = remaining_months.max(1);
        event
    }

    pub fn mortality_probability(&self, config: &SimulationConfig) -> f64 {
        let severity = f64::from(self.severity) / 10.0;
        clamp_probability(config.disaster_max_mortality * severity.powf(1.5))
    }

    pub fn town_damage(&self, population_capacity: u32) -> TownDamage {
        let severity = u32::from(self.severity);
        let capacity_loss = ((u64::from(population_capacity) * u64::from(severity) * 3) / 100)
            .min(u64::from(u32::MAX)) as u32;
        TownDamage::new(
            self.severity.div_ceil(3).min(4),
            self.severity.div_ceil(2).min(5),
            capacity_loss,
            self.remaining_months,
        )
    }

    /// 1か月進め、まだ継続中なら true。
    pub fn progress_month(&mut self) -> bool {
        self.remaining_months = self.remaining_months.saturating_sub(1);
        self.remaining_months > 0
    }
}

/// 飢饉を独立乱数ではなく複合条件から発生させるための入力。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FamineConditions {
    pub disaster_severity: u8,
    pub war_severity: u8,
    pub occupancy: f64,
    pub effective_jobs: u8,
    pub effective_safety: u8,
}

impl FamineConditions {
    /// 災害、戦争、過密、都市機能低下を合成した `0.0..=10.0` の危険度。
    pub fn risk_score(&self) -> f64 {
        let disaster = f64::from(self.disaster_severity.min(10)) * 0.25;
        let war = f64::from(self.war_severity.min(10)) * 0.35;
        let occupancy = if self.occupancy.is_finite() {
            ((self.occupancy.max(0.0) - 0.90) * 5.0).clamp(0.0, 4.0)
        } else {
            0.0
        };
        let deterioration = (f64::from(10 - self.effective_jobs.min(10))
            + f64::from(10 - self.effective_safety.min(10)))
            * 0.15;
        (disaster + war + occupancy + deterioration).clamp(0.0, 10.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamineEvent {
    pub town: TownId,
    pub severity: u8,
    pub remaining_months: u16,
}

impl FamineEvent {
    pub const START_THRESHOLD: f64 = 6.0;

    pub fn new(town: TownId, severity: u8, remaining_months: u16) -> Self {
        Self {
            town,
            severity: severity.clamp(1, 10),
            remaining_months: remaining_months.max(1),
        }
    }

    pub fn from_conditions(town: TownId, conditions: FamineConditions) -> Option<Self> {
        let risk = conditions.risk_score();
        if risk < Self::START_THRESHOLD {
            return None;
        }
        let severity = risk.round().clamp(1.0, 10.0) as u8;
        Some(Self::new(town, severity, 6 + u16::from(severity) * 3))
    }

    pub fn mortality_probability(&self, config: &SimulationConfig) -> f64 {
        let severity = f64::from(self.severity) / 10.0;
        clamp_probability(config.famine_max_mortality * severity.powf(1.35))
    }

    pub fn birth_rate_multiplier(&self) -> f64 {
        (1.0 - f64::from(self.severity) * 0.07).clamp(0.25, 1.0)
    }

    pub fn emigration_multiplier(&self) -> f64 {
        1.0 + f64::from(self.severity) * 0.18
    }

    pub fn town_damage(&self) -> TownDamage {
        TownDamage::new(
            self.severity.div_ceil(3).min(4),
            self.severity.div_ceil(5).min(2),
            0,
            self.remaining_months,
        )
    }

    pub fn progress_month(&mut self) -> bool {
        self.remaining_months = self.remaining_months.saturating_sub(1);
        self.remaining_months > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disaster_severity_is_clamped_and_damage_scales() {
        let mild = NaturalDisasterEvent::new(NaturalDisaster::Flood, TownId(1), 1);
        let severe = NaturalDisasterEvent::new(NaturalDisaster::Flood, TownId(1), 99);
        assert_eq!(severe.severity, 10);
        assert!(severe.town_damage(1_000).capacity_loss > mild.town_damage(1_000).capacity_loss);
        assert!(
            severe.mortality_probability(&SimulationConfig::default())
                > mild.mortality_probability(&SimulationConfig::default())
        );
    }

    #[test]
    fn famine_needs_compound_bad_conditions() {
        let healthy = FamineConditions {
            disaster_severity: 0,
            war_severity: 0,
            occupancy: 0.7,
            effective_jobs: 8,
            effective_safety: 8,
        };
        let crisis = FamineConditions {
            disaster_severity: 8,
            war_severity: 8,
            occupancy: 1.4,
            effective_jobs: 2,
            effective_safety: 2,
        };
        assert!(FamineEvent::from_conditions(TownId(0), healthy).is_none());
        assert!(FamineEvent::from_conditions(TownId(0), crisis).is_some());
    }

    #[test]
    fn event_duration_expires_without_underflow() {
        let mut event = NaturalDisasterEvent::with_duration(NaturalDisaster::Fire, TownId(0), 5, 1);
        assert!(!event.progress_month());
        assert!(!event.progress_month());
    }
}
