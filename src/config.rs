use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// 世界で負のイベントが起きる頻度を表すプリセット。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorldDanger {
    Peaceful,
    #[default]
    Normal,
    Harsh,
}

impl WorldDanger {
    /// 通常世界を 1.0 とした負のイベント頻度の倍率。
    pub const fn multiplier(self) -> f64 {
        match self {
            Self::Peaceful => 0.05,
            Self::Normal => 1.0,
            Self::Harsh => 1.75,
        }
    }
}

impl fmt::Display for WorldDanger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Peaceful => "peaceful",
            Self::Normal => "normal",
            Self::Harsh => "harsh",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown world danger '{0}' (expected peaceful, normal, or harsh)")]
pub struct ParseWorldDangerError(pub String);

impl FromStr for WorldDanger {
    type Err = ParseWorldDangerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "peaceful" => Ok(Self::Peaceful),
            "normal" => Ok(Self::Normal),
            "harsh" => Ok(Self::Harsh),
            _ => Err(ParseWorldDangerError(value.to_owned())),
        }
    }
}

/// 両端を含む年齢帯と、その年齢帯に適用する年次確率。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgeRate {
    pub min_age: u8,
    pub max_age: u8,
    pub annual_probability: f64,
}

impl AgeRate {
    pub fn new(min_age: u8, max_age: u8, annual_probability: f64) -> Self {
        Self {
            min_age: min_age.min(max_age),
            max_age: min_age.max(max_age),
            annual_probability: clamp_probability(annual_probability),
        }
    }

    pub const fn contains(&self, age: u8) -> bool {
        self.min_age <= age && age <= self.max_age
    }
}

/// 疎な関係グラフから、長期間使われていない弱い辺を除くための設定。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationshipDecayConfig {
    pub weak_relation_max: u8,
    pub weak_forget_years: u16,
    pub friendly_relation: u8,
    pub friendly_forget_years: u16,
    pub permanent_relation_min: u8,
}

impl Default for RelationshipDecayConfig {
    fn default() -> Self {
        Self {
            weak_relation_max: 5,
            weak_forget_years: 4,
            friendly_relation: 6,
            friendly_forget_years: 8,
            permanent_relation_min: 7,
        }
    }
}

impl RelationshipDecayConfig {
    /// Family / Partner のような保護関係は `protected = true` にする。
    pub fn should_forget(
        &self,
        relation: u8,
        years_since_interaction: u16,
        protected: bool,
    ) -> bool {
        if protected || relation >= self.permanent_relation_min.min(10) {
            return false;
        }

        if relation <= self.weak_relation_max.min(10) {
            years_since_interaction >= self.weak_forget_years
        } else if relation == self.friendly_relation.min(10) {
            years_since_interaction >= self.friendly_forget_years
        } else {
            false
        }
    }
}

/// すべての確率・閾値を一元管理するシミュレーション設定。
///
/// 確率フィールドは `0.0..=1.0` を想定する。公開フィールドを編集した場合でも、
/// 確率を返すメソッドは必ずこの範囲へ clamp する。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulationConfig {
    pub world_danger: WorldDanger,

    pub birth_rates: Vec<AgeRate>,
    pub mortality_rates: Vec<AgeRate>,

    /// 1人口あたりの年次外部流入率。
    pub immigration_rate: f64,
    /// 1人口あたりの年次外部流出率。
    pub emigration_rate: f64,

    pub internal_migration_monthly_probability: f64,
    pub partnership_monthly_probability: f64,
    pub social_event_monthly_probability: f64,

    /// 世界全体で各年に少なくとも1件のイベントを開始する基礎確率。
    pub disaster_probability: f64,
    pub disease_probability: f64,
    pub war_probability: f64,

    /// 数年ごとに行う低頻度な目標再評価の年次確率。
    pub goal_reassessment_probability: f64,
    pub relationship_decay: RelationshipDecayConfig,
    pub goal_change_cooldown_years: u16,
    pub max_beliefs: usize,
    pub max_relationships_per_npc: usize,

    /// 都市間伝播を判定する際の月次基礎確率。
    pub disease_spread_monthly_probability: f64,
    /// イベントの severity=10 で用いる追加年次死亡率の上限。
    pub disaster_max_mortality: f64,
    pub disease_max_mortality: f64,
    pub war_max_mortality: f64,
    pub famine_max_mortality: f64,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self::normal()
    }
}

impl SimulationConfig {
    pub fn peaceful() -> Self {
        Self::for_danger(WorldDanger::Peaceful)
    }

    pub fn normal() -> Self {
        Self::for_danger(WorldDanger::Normal)
    }

    pub fn harsh() -> Self {
        Self::for_danger(WorldDanger::Harsh)
    }

    pub fn for_danger(world_danger: WorldDanger) -> Self {
        let multiplier = world_danger.multiplier();
        Self {
            world_danger,
            // パートナーのいるNPC（実際の出生判定は片方のみ）に対する年次率。
            birth_rates: vec![
                AgeRate::new(18, 24, 0.18),
                AgeRate::new(25, 34, 0.25),
                AgeRate::new(35, 39, 0.14),
                AgeRate::new(40, 45, 0.045),
            ],
            mortality_rates: vec![
                AgeRate::new(0, 0, 0.004),
                AgeRate::new(1, 14, 0.0002),
                AgeRate::new(15, 39, 0.0007),
                AgeRate::new(40, 59, 0.003),
                AgeRate::new(60, 69, 0.012),
                AgeRate::new(70, 79, 0.04),
                AgeRate::new(80, 89, 0.11),
                AgeRate::new(90, u8::MAX, 0.24),
            ],
            immigration_rate: 0.015,
            emigration_rate: 0.004,
            internal_migration_monthly_probability: 0.0025,
            partnership_monthly_probability: 0.05,
            social_event_monthly_probability: 0.25,
            // Normal で100年間あたり概ね15件、5件、1.5件。
            disaster_probability: 0.15 * multiplier,
            disease_probability: 0.05 * multiplier,
            war_probability: 0.015 * multiplier,
            goal_reassessment_probability: 0.12,
            relationship_decay: RelationshipDecayConfig::default(),
            goal_change_cooldown_years: 3,
            max_beliefs: 3,
            max_relationships_per_npc: 30,
            disease_spread_monthly_probability: 0.08,
            disaster_max_mortality: 0.035,
            disease_max_mortality: 0.06,
            war_max_mortality: 0.045,
            famine_max_mortality: 0.08,
        }
    }

    pub const fn danger_multiplier(&self) -> f64 {
        self.world_danger.multiplier()
    }

    pub fn birth_rate(&self, age: u8) -> f64 {
        rate_for_age(&self.birth_rates, age)
    }

    pub fn mortality_rate(&self, age: u8) -> f64 {
        rate_for_age(&self.mortality_rates, age)
    }

    pub fn annual_disaster_probability(&self) -> f64 {
        clamp_probability(self.disaster_probability)
    }

    pub fn annual_disease_probability(&self) -> f64 {
        clamp_probability(self.disease_probability)
    }

    pub fn annual_war_probability(&self) -> f64 {
        clamp_probability(self.war_probability)
    }

    pub fn monthly_partnership_probability(&self) -> f64 {
        clamp_probability(self.partnership_monthly_probability)
    }

    pub fn monthly_social_probability(&self) -> f64 {
        clamp_probability(self.social_event_monthly_probability)
    }

    pub fn monthly_migration_probability(&self) -> f64 {
        clamp_probability(self.internal_migration_monthly_probability)
    }

    pub fn annual_immigration_rate(&self) -> f64 {
        clamp_probability(self.immigration_rate)
    }

    pub fn annual_emigration_rate(&self) -> f64 {
        clamp_probability(self.emigration_rate)
    }

    pub fn should_forget_relationship(
        &self,
        relation: u8,
        last_interaction_year: u16,
        current_year: u16,
        protected: bool,
    ) -> bool {
        self.relationship_decay.should_forget(
            relation,
            current_year.saturating_sub(last_interaction_year),
            protected,
        )
    }
}

fn rate_for_age(rates: &[AgeRate], age: u8) -> f64 {
    rates
        .iter()
        .find(|rate| rate.contains(age))
        .map_or(0.0, |rate| clamp_probability(rate.annual_probability))
}

pub(crate) fn clamp_probability(probability: f64) -> f64 {
    if probability.is_nan() {
        0.0
    } else {
        probability.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn danger_profiles_have_ordered_event_probabilities() {
        let peaceful = SimulationConfig::peaceful();
        let normal = SimulationConfig::normal();
        let harsh = SimulationConfig::harsh();

        assert!(peaceful.disaster_probability < normal.disaster_probability);
        assert!(normal.disaster_probability < harsh.disaster_probability);
        assert!(peaceful.disease_probability < normal.disease_probability);
        assert!(normal.war_probability < harsh.war_probability);
    }

    #[test]
    fn age_rates_cover_expected_boundaries() {
        let config = SimulationConfig::default();
        assert_eq!(config.birth_rate(17), 0.0);
        assert!(config.birth_rate(18) > 0.0);
        assert!(config.birth_rate(45) > 0.0);
        assert_eq!(config.birth_rate(46), 0.0);
        assert!(config.mortality_rate(90) > config.mortality_rate(50));
    }

    #[test]
    fn relationship_decay_keeps_strong_and_protected_edges() {
        let config = SimulationConfig::default();
        assert!(config.should_forget_relationship(5, 10, 14, false));
        assert!(!config.should_forget_relationship(5, 10, 13, false));
        assert!(config.should_forget_relationship(6, 10, 18, false));
        assert!(!config.should_forget_relationship(7, 0, 100, false));
        assert!(!config.should_forget_relationship(2, 0, 100, true));
    }

    #[test]
    fn invalid_public_probabilities_are_safely_clamped() {
        let config = SimulationConfig {
            disaster_probability: 5.0,
            partnership_monthly_probability: f64::NAN,
            ..SimulationConfig::default()
        };
        assert_eq!(config.annual_disaster_probability(), 1.0);
        assert_eq!(config.monthly_partnership_probability(), 0.0);
    }

    #[test]
    fn danger_parses_and_displays_for_cli() {
        assert_eq!("harsh".parse(), Ok(WorldDanger::Harsh));
        assert_eq!(WorldDanger::Peaceful.to_string(), "peaceful");
        assert!("unknown".parse::<WorldDanger>().is_err());
    }
}
