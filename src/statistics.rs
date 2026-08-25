use crate::event::{DeathCause, WorldEvent};
use serde::{Deserialize, Serialize};
use std::fmt;

/// 1年間に発生した変化と、その年末時点の人口スナップショット。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct YearStatistics {
    pub year: u64,
    pub total_population: usize,
    pub births: usize,
    pub deaths: usize,
    pub external_immigration: usize,
    pub external_emigration: usize,
    pub internal_migrations: usize,
    pub partnerships: usize,
    pub belief_changes: usize,
    pub goal_changes: usize,
    pub disaster_deaths: usize,
    pub disease_deaths: usize,
    pub war_deaths: usize,
    pub famine_deaths: usize,
    /// World.towns と同じ安定順で格納する。
    pub town_populations: Vec<usize>,
}

impl YearStatistics {
    pub fn new(year: u64, total_population: usize, town_populations: Vec<usize>) -> Self {
        Self {
            year,
            total_population,
            town_populations,
            ..Self::default()
        }
    }

    /// イベントから直接分かるカウンタを1増やす。
    /// 原因付き死亡には `record_death` を直接使う。
    pub fn record_event(&mut self, event: &WorldEvent) {
        match event {
            WorldEvent::Birth { .. } => self.births = self.births.saturating_add(1),
            WorldEvent::Death { .. } => self.record_death(DeathCause::Natural),
            WorldEvent::Partnership { .. } => {
                self.partnerships = self.partnerships.saturating_add(1)
            }
            WorldEvent::Migration { .. } => {
                self.internal_migrations = self.internal_migrations.saturating_add(1)
            }
            WorldEvent::BeliefChanged { .. } => {
                self.belief_changes = self.belief_changes.saturating_add(1)
            }
            WorldEvent::GoalChanged { .. } => {
                self.goal_changes = self.goal_changes.saturating_add(1)
            }
            WorldEvent::ExternalImmigration { .. } => {
                self.external_immigration = self.external_immigration.saturating_add(1)
            }
            WorldEvent::ExternalEmigration { .. } => {
                self.external_emigration = self.external_emigration.saturating_add(1)
            }
            _ => {}
        }
    }

    pub fn record_death(&mut self, cause: DeathCause) {
        self.deaths = self.deaths.saturating_add(1);
        match cause {
            DeathCause::Disaster => self.disaster_deaths = self.disaster_deaths.saturating_add(1),
            DeathCause::Disease => self.disease_deaths = self.disease_deaths.saturating_add(1),
            DeathCause::War => self.war_deaths = self.war_deaths.saturating_add(1),
            DeathCause::Famine => self.famine_deaths = self.famine_deaths.saturating_add(1),
            DeathCause::Natural => {}
        }
    }

    /// 明示分類されていない死亡（通常死亡と飢饉死）の合計。
    pub fn natural_deaths(&self) -> usize {
        self.deaths.saturating_sub(
            self.disaster_deaths
                .saturating_add(self.disease_deaths)
                .saturating_add(self.war_deaths)
                .saturating_add(self.famine_deaths),
        )
    }
}

/// 年次統計を足し上げた最終サマリ。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CumulativeStatistics {
    pub initial_population: usize,
    pub final_population: usize,
    pub total_unique_npcs: usize,
    pub births: usize,
    pub deaths: usize,
    pub natural_deaths: usize,
    pub external_immigration: usize,
    pub external_emigration: usize,
    pub internal_migrations: usize,
    pub partnerships: usize,
    pub belief_changes: usize,
    pub goal_changes: usize,
    pub disaster_deaths: usize,
    pub disease_deaths: usize,
    pub war_deaths: usize,
    pub famine_deaths: usize,
    pub town_populations: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statistics {
    pub initial_population: usize,
    pub total_unique_npcs: usize,
    pub years: Vec<YearStatistics>,
    #[serde(skip, default = "retain_history_by_default")]
    retain_year_history: bool,
    #[serde(skip)]
    cumulative_before_retained: CumulativeStatistics,
    #[serde(skip)]
    years_before_retained: u64,
}

const fn retain_history_by_default() -> bool {
    true
}

impl Default for Statistics {
    fn default() -> Self {
        Self {
            initial_population: 0,
            total_unique_npcs: 0,
            years: Vec::new(),
            retain_year_history: true,
            cumulative_before_retained: CumulativeStatistics::default(),
            years_before_retained: 0,
        }
    }
}

impl Statistics {
    pub fn new(initial_population: usize) -> Self {
        Self {
            initial_population,
            total_unique_npcs: initial_population,
            years: Vec::new(),
            retain_year_history: true,
            cumulative_before_retained: CumulativeStatistics::default(),
            years_before_retained: 0,
        }
    }

    /// 無期限運転向けに年次履歴を最新1件だけへ制限する。
    /// 累積カウンタは別途保持するため、サマリーとwarningは全期間を対象にできる。
    pub fn retain_only_latest_year(&mut self) {
        if self.retain_year_history {
            if self.years.len() > 1 {
                let latest = self.years.pop().expect("length was checked");
                for year in self.years.drain(..) {
                    add_to_cumulative(&mut self.cumulative_before_retained, &year);
                    self.years_before_retained = self.years_before_retained.saturating_add(1);
                }
                self.years.push(latest);
            }
            self.retain_year_history = false;
        }
    }

    /// 年次スナップショットを追加し、延べ固有NPC数も更新する。
    pub fn push(&mut self, year: YearStatistics) {
        self.total_unique_npcs = self
            .total_unique_npcs
            .saturating_add(year.births)
            .saturating_add(year.external_immigration);
        if !self.retain_year_history {
            if let Some(previous) = self.years.pop() {
                add_to_cumulative(&mut self.cumulative_before_retained, &previous);
                self.years_before_retained = self.years_before_retained.saturating_add(1);
            }
        }
        self.years.push(year);
    }

    pub fn push_year(&mut self, year: YearStatistics) {
        self.push(year);
    }

    pub fn latest(&self) -> Option<&YearStatistics> {
        self.years.last()
    }

    pub fn len(&self) -> usize {
        usize::try_from(self.total_years()).unwrap_or(usize::MAX)
    }

    pub fn is_empty(&self) -> bool {
        self.total_years() == 0
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &YearStatistics> {
        self.years.iter()
    }

    pub fn total_years(&self) -> u64 {
        self.years_before_retained
            .saturating_add(self.years.len() as u64)
    }

    pub fn cumulative(&self) -> CumulativeStatistics {
        let mut result = self.cumulative_before_retained.clone();
        result.initial_population = self.initial_population;
        result.final_population = self
            .latest()
            .map_or(self.initial_population, |year| year.total_population);
        result.total_unique_npcs = self.total_unique_npcs;
        result.town_populations = self
            .latest()
            .map_or_else(Vec::new, |year| year.town_populations.clone());
        for year in &self.years {
            add_to_cumulative(&mut result, year);
        }
        result.natural_deaths = result.deaths.saturating_sub(
            result
                .disaster_deaths
                .saturating_add(result.disease_deaths)
                .saturating_add(result.war_deaths)
                .saturating_add(result.famine_deaths),
        );
        result
    }

    pub fn detect_warnings(&self, health: SimulationHealthMetrics) -> Vec<SimulationWarning> {
        let cumulative = self.cumulative();
        let mut warnings = Vec::new();

        let extreme_fraction = finite_or_zero(health.extreme_relationship_fraction);
        if extreme_fraction >= 0.80 {
            warnings.push(SimulationWarning::RelationshipPolarization {
                fraction: extreme_fraction,
            });
        }

        let average_relationships = finite_or_zero(health.average_active_relationships);
        if average_relationships > 50.0 {
            warnings.push(SimulationWarning::DenseRelationshipGraph {
                average: average_relationships,
            });
        }

        if cumulative.town_populations.len() > 1 && cumulative.final_population > 0 {
            let largest = cumulative
                .town_populations
                .iter()
                .copied()
                .max()
                .unwrap_or(0);
            let share = largest as f64 / cumulative.final_population as f64;
            if share > 0.40 {
                warnings.push(SimulationWarning::TownConcentration { share });
            }
        }

        if !self.is_empty()
            && cumulative.town_populations.len() > 1
            && cumulative.internal_migrations == 0
        {
            warnings.push(SimulationWarning::NoMigration);
        }
        if !self.is_empty() && cumulative.goal_changes == 0 {
            warnings.push(SimulationWarning::NoGoalChanges);
        }

        let simulated_years = self.total_years();
        if simulated_years > 0 && cumulative.total_unique_npcs > 0 {
            let per_npc_year = cumulative.goal_changes as f64
                / (cumulative.total_unique_npcs as f64 * simulated_years as f64);
            if per_npc_year > 1.0 {
                warnings.push(SimulationWarning::FrequentGoalChanges {
                    average_per_npc_year: per_npc_year,
                });
            }
        }

        if cumulative.initial_population > 0 {
            let population_factor =
                cumulative.final_population as f64 / cumulative.initial_population as f64;
            if population_factor > 5.0 {
                warnings.push(SimulationWarning::PopulationExplosion {
                    factor: population_factor,
                });
            }
            let decline = 1.0 - population_factor;
            if decline > 0.90 {
                warnings.push(SimulationWarning::PopulationCollapse { decline });
            }
        }

        warnings
    }
}

fn add_to_cumulative(result: &mut CumulativeStatistics, year: &YearStatistics) {
    result.births = result.births.saturating_add(year.births);
    result.deaths = result.deaths.saturating_add(year.deaths);
    result.external_immigration = result
        .external_immigration
        .saturating_add(year.external_immigration);
    result.external_emigration = result
        .external_emigration
        .saturating_add(year.external_emigration);
    result.internal_migrations = result
        .internal_migrations
        .saturating_add(year.internal_migrations);
    result.partnerships = result.partnerships.saturating_add(year.partnerships);
    result.belief_changes = result.belief_changes.saturating_add(year.belief_changes);
    result.goal_changes = result.goal_changes.saturating_add(year.goal_changes);
    result.disaster_deaths = result.disaster_deaths.saturating_add(year.disaster_deaths);
    result.disease_deaths = result.disease_deaths.saturating_add(year.disease_deaths);
    result.war_deaths = result.war_deaths.saturating_add(year.war_deaths);
    result.famine_deaths = result.famine_deaths.saturating_add(year.famine_deaths);
}

/// 年次統計からは導けない関係グラフの健全性指標。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SimulationHealthMetrics {
    pub average_active_relationships: f64,
    pub average_strong_relationships: f64,
    /// relation が 0 または 10 の関係の比率 (`0.0..=1.0`)。
    pub extreme_relationship_fraction: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SimulationWarning {
    RelationshipPolarization { fraction: f64 },
    DenseRelationshipGraph { average: f64 },
    TownConcentration { share: f64 },
    NoMigration,
    NoGoalChanges,
    FrequentGoalChanges { average_per_npc_year: f64 },
    PopulationExplosion { factor: f64 },
    PopulationCollapse { decline: f64 },
}

impl fmt::Display for SimulationWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RelationshipPolarization { fraction } => write!(
                f,
                "WARN: {:.0}% of relationships have relation 0 or 10",
                fraction * 100.0
            ),
            Self::DenseRelationshipGraph { average } => write!(
                f,
                "WARN: average active relationships is {:.1} (> 50)",
                average
            ),
            Self::TownConcentration { share } => write!(
                f,
                "WARN: one town owns {:.0}% of total population",
                share * 100.0
            ),
            Self::NoMigration => f.write_str("WARN: no migration occurred"),
            Self::NoGoalChanges => f.write_str("WARN: no goal changes occurred"),
            Self::FrequentGoalChanges {
                average_per_npc_year,
            } => write!(
                f,
                "WARN: average goal changes is {:.2}/year/NPC",
                average_per_npc_year
            ),
            Self::PopulationExplosion { factor } => {
                write!(f, "WARN: population increased {:.1}x (> 5x)", factor)
            }
            Self::PopulationCollapse { decline } => write!(
                f,
                "WARN: population decreased {:.0}% (> 90%)",
                decline * 100.0
            ),
        }
    }
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{NpcId, TownId};

    #[test]
    fn records_events_and_death_causes() {
        let mut year = YearStatistics::new(1, 100, vec![60, 40]);
        year.record_event(&WorldEvent::Birth { npc: NpcId(1) });
        year.record_event(&WorldEvent::Migration {
            npc: NpcId(1),
            from: TownId(0),
            to: TownId(1),
        });
        year.record_death(DeathCause::Disease);
        year.record_death(DeathCause::Famine);
        year.record_death(DeathCause::Natural);
        assert_eq!(year.births, 1);
        assert_eq!(year.internal_migrations, 1);
        assert_eq!(year.deaths, 3);
        assert_eq!(year.disease_deaths, 1);
        assert_eq!(year.famine_deaths, 1);
        assert_eq!(year.natural_deaths(), 1);
    }

    #[test]
    fn cumulative_statistics_sum_years_and_track_unique_npcs() {
        let mut statistics = Statistics::new(100);
        let mut first = YearStatistics::new(1, 105, vec![105]);
        first.births = 10;
        first.deaths = 5;
        first.external_immigration = 2;
        statistics.push(first);
        let mut second = YearStatistics::new(2, 106, vec![106]);
        second.births = 4;
        second.deaths = 3;
        statistics.push(second);

        let cumulative = statistics.cumulative();
        assert_eq!(cumulative.births, 14);
        assert_eq!(cumulative.deaths, 8);
        assert_eq!(cumulative.final_population, 106);
        assert_eq!(cumulative.total_unique_npcs, 116);
    }

    #[test]
    fn continuous_mode_retains_only_latest_year_and_keeps_totals() {
        let mut statistics = Statistics::new(100);
        statistics.retain_only_latest_year();
        for year_number in 1..=3 {
            let mut year = YearStatistics::new(year_number, 100 + year_number as usize, vec![]);
            year.births = year_number as usize;
            statistics.push(year);
        }

        assert_eq!(statistics.years.len(), 1);
        assert_eq!(statistics.latest().map(|year| year.year), Some(3));
        assert_eq!(statistics.total_years(), 3);
        assert_eq!(statistics.cumulative().births, 6);
    }

    #[test]
    fn warnings_are_generated_but_do_not_fail_statistics() {
        let mut statistics = Statistics::new(100);
        statistics.push(YearStatistics::new(100, 5, vec![4, 1]));
        let warnings = statistics.detect_warnings(SimulationHealthMetrics {
            average_active_relationships: 55.0,
            average_strong_relationships: 2.0,
            extreme_relationship_fraction: 0.8,
        });
        assert!(
            warnings
                .iter()
                .any(|warning| matches!(warning, SimulationWarning::DenseRelationshipGraph { .. }))
        );
        assert!(
            warnings.iter().any(|warning| matches!(
                warning,
                SimulationWarning::RelationshipPolarization { .. }
            ))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| matches!(warning, SimulationWarning::PopulationCollapse { .. }))
        );
        assert!(
            warnings
                .iter()
                .all(|warning| warning.to_string().starts_with("WARN:"))
        );
    }

    #[test]
    fn yearly_json_round_trip_is_reproducible() {
        let year = YearStatistics::new(42, 123, vec![50, 73]);
        let json = serde_json::to_string(&year).unwrap();
        assert_eq!(serde_json::from_str::<YearStatistics>(&json).unwrap(), year);
    }
}
