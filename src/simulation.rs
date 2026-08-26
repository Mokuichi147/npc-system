use std::collections::{BTreeMap, HashSet};

use rand::Rng;
use serde::Serialize;
use thiserror::Error;

use crate::belief::{Belief, BeliefKind};
use crate::config::SimulationConfig;
use crate::disaster::{FamineConditions, FamineEvent, NaturalDisaster, NaturalDisasterEvent};
use crate::event::{DeathCause, WorldEvent};
#[cfg(feature = "economy-extension")]
use crate::extensions::SimulationExtension;
#[cfg(feature = "economy-extension")]
use crate::extensions::economy::EconomyExtension;
use crate::goal::GoalKind;
use crate::id::{NpcId, TownId};
use crate::migration::{candidate_towns, household_members, move_household};
use crate::npc::{Npc, NpcState, Sex};
use crate::population::{
    PopulationError, generate_child, generate_external_immigrant, generate_initial_world,
};
use crate::relationship::{Relationship, RelationshipKind};
use crate::statistics::{SimulationHealthMetrics, YearStatistics};
use crate::utility::{
    Action, MoveCandidate, Situation, choose_action, sorted_relationship_candidates,
};
use crate::war::WarEvent;
use crate::world::{InvariantError, World};

#[derive(Debug, Error)]
pub enum SimulationError {
    #[error(transparent)]
    Population(#[from] PopulationError),
    #[error(transparent)]
    Invariant(#[from] InvariantError),
    #[error("{0}")]
    InvalidConfiguration(String),
    #[error("year {year} population conservation failed: expected {expected}, actual {actual}")]
    PopulationConservation {
        year: u64,
        expected: i128,
        actual: usize,
    },
    #[error("the simulation year cannot exceed {0}")]
    YearOverflow(u64),
}

/// CLIとテストから扱うシミュレーション実行器。
pub struct Simulation {
    pub world: World,
    pub config: SimulationConfig,
    pub seed: u64,
}

impl Simulation {
    pub fn new(
        towns: usize,
        population: usize,
        seed: u64,
        config: SimulationConfig,
    ) -> Result<Self, SimulationError> {
        if towns == 0 {
            return Err(SimulationError::InvalidConfiguration(
                "towns must be at least 1".to_owned(),
            ));
        }
        if towns > usize::from(u16::MAX) + 1 {
            return Err(SimulationError::InvalidConfiguration(format!(
                "towns must be at most {}",
                usize::from(u16::MAX) + 1
            )));
        }
        if population > u32::MAX as usize {
            return Err(SimulationError::InvalidConfiguration(
                "population exceeds the NpcId range".to_owned(),
            ));
        }
        let world = generate_initial_world(towns, population, seed, &config)?;
        world.validate()?;
        Ok(Self {
            world,
            config,
            seed,
        })
    }

    pub fn run(&mut self, years: u64) -> Result<(), SimulationError> {
        for _ in 0..years {
            self.run_year()?;
        }
        Ok(())
    }

    /// 1年を進め、確定した年次統計を返す。
    pub fn run_year(&mut self) -> Result<&YearStatistics, SimulationError> {
        self.world.year_events.clear();
        let previous_population = self.world.active_population();
        self.world.year = self
            .world
            .year
            .checked_add(1)
            .ok_or(SimulationError::YearOverflow(u64::MAX))?;
        self.world.month = 0;
        let year = self.world.year;
        let mut statistics = YearStatistics::new(year, 0, Vec::new());
        #[cfg(feature = "economy-extension")]
        EconomyExtension::begin_year(&mut self.world);

        self.age_active_npcs();

        let disaster_severity = self.trigger_natural_disaster(&mut statistics);
        self.maybe_start_disease(&mut statistics);
        self.maybe_start_war(&mut statistics);
        self.maybe_start_famines(&disaster_severity, &mut statistics);

        let mut deaths = BTreeMap::new();
        self.queue_disaster_deaths(&disaster_severity, &mut deaths);
        self.queue_war_deaths(&mut deaths);
        self.queue_natural_deaths(&mut deaths);
        self.commit_deaths(deaths, &mut statistics);

        self.process_births(&mut statistics)?;
        self.process_external_emigration(&mut statistics);
        self.process_external_immigration(&mut statistics)?;

        for month in 1..=12 {
            self.world.month = month;
            self.progress_diseases_and_famines(&mut statistics);
            let actions = self.choose_utility_actions();
            #[cfg(feature = "economy-extension")]
            EconomyExtension::run_month(&mut self.world, &self.config, &actions);
            self.process_social_events(&actions, &mut statistics);
            self.process_partnerships(&actions, &mut statistics);
            self.process_internal_migration(&actions, &mut statistics);
            self.apply_utility_outcomes(&actions);
            for town in &mut self.world.towns {
                town.recover();
            }
            self.world.rebuild_active_npcs();
            debug_assert!(self.world.validate().is_ok());
        }

        self.progress_wars();
        self.reassess_goals(&mut statistics);
        self.forget_relationships();
        self.world.rebuild_active_npcs();

        statistics.total_population = self.world.active_population();
        statistics.town_populations = self.world.town_populations();
        #[cfg(feature = "economy-extension")]
        EconomyExtension::finish_year(&self.world, &mut statistics);
        let expected = previous_population as i128
            + statistics.births as i128
            + statistics.external_immigration as i128
            - statistics.deaths as i128
            - statistics.external_emigration as i128;
        if expected != statistics.total_population as i128 {
            return Err(SimulationError::PopulationConservation {
                year,
                expected,
                actual: statistics.total_population,
            });
        }
        self.world.validate()?;
        self.world.statistics.push(statistics);
        Ok(self
            .world
            .statistics
            .latest()
            .expect("a statistic was just pushed"))
    }

    pub fn health_metrics(&self) -> SimulationHealthMetrics {
        if self.world.active_npcs.is_empty() {
            return SimulationHealthMetrics::default();
        }
        let mut active = vec![false; self.world.npcs.len()];
        for &id in &self.world.active_npcs {
            if let Some(value) = active.get_mut(id.0 as usize) {
                *value = true;
            }
        }

        let mut relationships = 0usize;
        let mut strong = 0usize;
        let mut extreme = 0usize;
        for &id in &self.world.active_npcs {
            let Some(npc) = self.world.npc(id).cloned() else {
                continue;
            };
            for (&other_id, relationship) in &npc.relationships {
                if active.get(other_id.0 as usize).copied().unwrap_or(false) {
                    relationships += 1;
                    strong += usize::from(relationship.is_strong());
                    extreme += usize::from(matches!(relationship.relation, 0 | 10));
                }
            }
        }
        let population = self.world.active_population() as f64;
        SimulationHealthMetrics {
            average_active_relationships: relationships as f64 / population,
            average_strong_relationships: strong as f64 / population,
            extreme_relationship_fraction: if relationships == 0 {
                0.0
            } else {
                extreme as f64 / relationships as f64
            },
        }
    }

    fn age_active_npcs(&mut self) {
        let ids = self.world.active_npcs.clone();
        for id in ids {
            if let Some(npc) = self.world.npc_mut(id) {
                npc.age_one_year();
            }
        }
    }

    fn trigger_natural_disaster(&mut self, statistics: &mut YearStatistics) -> Vec<u8> {
        let mut severities = vec![0u8; self.world.towns.len()];
        if self.world.towns.is_empty()
            || !self
                .world
                .rng
                .random_bool(self.config.annual_disaster_probability())
        {
            return severities;
        }

        let town_index = self.world.rng.random_range(0..self.world.towns.len());
        let severity = self.world.rng.random_range(1..=10);
        let kind = NaturalDisaster::ALL[self.world.rng.random_range(0..NaturalDisaster::ALL.len())];
        let town_id = self.world.towns[town_index].id;
        let event = NaturalDisasterEvent::new(kind, town_id, severity);
        let capacity = self.world.towns[town_index].population_capacity;
        self.world.towns[town_index].apply_damage(event.town_damage(capacity));
        severities[town_index] = severity;
        self.world
            .push_event(WorldEvent::NaturalDisaster { town: town_id });

        let affected = self.world.residents_by_town()[town_index].clone();
        for id in affected {
            self.change_goal(id, GoalKind::Survive, true, statistics);
            self.change_belief(id, BeliefKind::ProtectHometown, 1, statistics);
        }
        severities
    }

    fn queue_disaster_deaths(
        &mut self,
        severities: &[u8],
        deaths: &mut BTreeMap<NpcId, DeathCause>,
    ) {
        let ids = self.world.active_npcs.clone();
        for id in ids {
            let Some(npc) = self.world.npc(id) else {
                continue;
            };
            let severity = severities
                .get(npc.town.0 as usize)
                .copied()
                .unwrap_or_default();
            if severity == 0 {
                continue;
            }
            let event = NaturalDisasterEvent::new(NaturalDisaster::Storm, npc.town, severity);
            if self
                .world
                .rng
                .random_bool(event.mortality_probability(&self.config))
            {
                deaths.insert(id, DeathCause::Disaster);
            }
        }
    }

    fn maybe_start_disease(&mut self, statistics: &mut YearStatistics) {
        if self.world.towns.is_empty()
            || !self
                .world
                .rng
                .random_bool(self.config.annual_disease_probability())
        {
            return;
        }
        let populations = self.world.town_populations();
        let mut weights = Vec::with_capacity(self.world.towns.len());
        let mut total = 0usize;
        for (town, population) in self.world.towns.iter().zip(populations) {
            let weight = (population + 1)
                .saturating_mul((town.occupancy(population) * 10.0).max(1.0) as usize);
            total = total.saturating_add(weight);
            weights.push(weight);
        }
        let mut draw = self.world.rng.random_range(0..total.max(1));
        let mut index = 0usize;
        for (candidate, weight) in weights.into_iter().enumerate() {
            if draw < weight {
                index = candidate;
                break;
            }
            draw = draw.saturating_sub(weight);
        }
        let town = self.world.towns[index].id;
        let severity = self.world.rng.random_range(2..=10);
        let months = self.world.rng.random_range(6..=24);
        let disease = crate::disease::DiseaseEvent::new(severity, town, months);
        self.world.towns[index].apply_damage(disease.town_damage());
        self.world.active_diseases.push(disease);
        self.world.push_event(WorldEvent::DiseaseOutbreak);
        let affected = self.world.residents_by_town()[index].clone();
        for id in affected {
            self.change_goal(id, GoalKind::Survive, true, statistics);
            self.change_belief(id, BeliefKind::HelpOthers, 1, statistics);
        }
    }

    fn maybe_start_war(&mut self, statistics: &mut YearStatistics) {
        if self.world.towns.len() < 2
            || !self
                .world
                .rng
                .random_bool(self.config.annual_war_probability())
        {
            return;
        }
        let first_index = self.world.rng.random_range(0..self.world.towns.len());
        let first = self.world.towns[first_index].id;
        let second = if self.world.towns[first_index].neighbors.is_empty() {
            self.world.towns[(first_index + 1) % self.world.towns.len()].id
        } else {
            let edge_index = self
                .world
                .rng
                .random_range(0..self.world.towns[first_index].neighbors.len());
            self.world.towns[first_index].neighbors[edge_index].destination
        };
        let severity = self.world.rng.random_range(2..=10);
        let years = self.world.rng.random_range(1..=3);
        let war = WarEvent::new([first, second], severity, years);
        let damage = war.town_damage();
        for &town_id in &war.towns {
            if let Some(town) = self.world.town_mut(town_id) {
                town.apply_damage(damage);
            }
        }
        let affected = self
            .world
            .active_npcs
            .iter()
            .copied()
            .filter(|&id| self.world.npc(id).is_some_and(|npc| war.includes(npc.town)))
            .collect::<Vec<_>>();
        self.world.active_wars.push(war);
        self.world.push_event(WorldEvent::WarStarted);
        for id in affected {
            self.change_goal(id, GoalKind::ProtectTown, true, statistics);
            self.change_belief(id, BeliefKind::ProtectHometown, 2, statistics);
            if let Some(npc) = self.world.npc_mut(id) {
                npc.state = NpcState::Evacuating;
            }
        }
    }

    fn queue_war_deaths(&mut self, deaths: &mut BTreeMap<NpcId, DeathCause>) {
        let ids = self.world.active_npcs.clone();
        for id in ids {
            let Some(npc) = self.world.npc(id) else {
                continue;
            };
            let probability = self
                .world
                .active_wars
                .iter()
                .filter(|war| war.includes(npc.town))
                .map(|war| war.additional_mortality(&self.config))
                .fold(0.0_f64, f64::max);
            if probability > 0.0 && self.world.rng.random_bool(probability) {
                deaths.insert(id, DeathCause::War);
            }
        }
    }

    fn queue_natural_deaths(&mut self, deaths: &mut BTreeMap<NpcId, DeathCause>) {
        let ids = self.world.active_npcs.clone();
        for id in ids {
            let Some(npc) = self.world.npc(id) else {
                continue;
            };
            let probability = self.config.mortality_rate(npc.age);
            if probability > 0.0 && self.world.rng.random_bool(probability) {
                deaths.entry(id).or_insert(DeathCause::Natural);
            }
        }
    }

    fn maybe_start_famines(&mut self, disaster_severity: &[u8], statistics: &mut YearStatistics) {
        let populations = self.world.town_populations();
        for index in 0..self.world.towns.len() {
            let town_id = self.world.towns[index].id;
            if self
                .world
                .active_famines
                .iter()
                .any(|famine| famine.town == town_id)
            {
                continue;
            }
            let war_severity = self
                .world
                .active_wars
                .iter()
                .filter(|war| war.includes(town_id))
                .map(|war| war.severity)
                .max()
                .unwrap_or_default();
            let town = &self.world.towns[index];
            let conditions = FamineConditions {
                disaster_severity: disaster_severity.get(index).copied().unwrap_or_default(),
                war_severity,
                occupancy: town.occupancy(populations.get(index).copied().unwrap_or_default()),
                effective_jobs: town.effective_jobs(),
                effective_safety: town.effective_safety(),
            };
            if let Some(famine) = FamineEvent::from_conditions(town_id, conditions) {
                let damage = famine.town_damage();
                self.world.towns[index].apply_damage(damage);
                self.world.active_famines.push(famine);
                self.world
                    .push_event(WorldEvent::FamineStarted { town: town_id });
                let affected = self.world.residents_by_town()[index].clone();
                for id in affected {
                    self.change_goal(id, GoalKind::Survive, true, statistics);
                    self.change_belief(id, BeliefKind::ProtectFamily, 1, statistics);
                }
            }
        }
    }

    fn commit_deaths(
        &mut self,
        deaths: BTreeMap<NpcId, DeathCause>,
        statistics: &mut YearStatistics,
    ) {
        for (id, cause) in deaths {
            let Some(snapshot) = self.world.npc(id) else {
                continue;
            };
            if !snapshot.is_active() {
                continue;
            }
            let partner = snapshot.partner;
            let mut affected_family = snapshot.parents.clone();
            affected_family.extend(snapshot.children.iter().copied());
            affected_family.extend(
                snapshot
                    .relationships
                    .iter()
                    .filter_map(|(&other, relationship)| relationship.is_strong().then_some(other)),
            );
            if let Some(partner_id) = partner {
                affected_family.push(partner_id);
            }
            affected_family.sort_unstable();
            affected_family.dedup();

            #[cfg(feature = "economy-extension")]
            EconomyExtension::before_npc_death(&mut self.world, id, &affected_family);

            if let Some(npc) = self.world.npc_mut(id) {
                npc.mark_dead();
            }
            if let Some(partner_id) = partner {
                if let Some(partner_npc) = self.world.npc_mut(partner_id) {
                    partner_npc.clear_partner_if(id);
                    if let Some(relationship) = partner_npc.relationship_mut(id) {
                        relationship.kind = RelationshipKind::Family;
                    }
                }
            }
            statistics.record_death(cause);
            self.world.push_event(WorldEvent::Death { npc: id });
            for relative in affected_family {
                if self.world.npc(relative).is_some_and(|npc| npc.is_active()) {
                    self.change_goal(relative, GoalKind::ProtectFamily, true, statistics);
                    self.change_belief(relative, BeliefKind::ProtectFamily, 1, statistics);
                }
            }
        }
        self.world.rebuild_active_npcs();
    }

    /// 現金と商品を近親者へ均等相続させる。相続人がいなければ都市財政へ戻す。
    fn process_births(&mut self, statistics: &mut YearStatistics) -> Result<(), PopulationError> {
        let mut populations = self.world.town_populations();
        let candidates = self.world.active_npcs.clone();
        for mother_id in candidates {
            let Some(mother) = self.world.npc(mother_id) else {
                continue;
            };
            if mother.sex != Sex::Female || !(18..=45).contains(&mother.age) {
                continue;
            }
            let Some(partner_id) = mother.partner else {
                continue;
            };
            let Some(partner) = self.world.npc(partner_id) else {
                continue;
            };
            if !partner.is_active()
                || partner.town != mother.town
                || !(18..=75).contains(&partner.age)
            {
                continue;
            }
            let town_id = mother.town;
            let age = mother.age;
            let town_index = town_id.0 as usize;
            let occupancy = self
                .world
                .town(town_id)
                .map(|town| {
                    town.occupancy(populations.get(town_index).copied().unwrap_or_default())
                })
                .unwrap_or(1.0);
            let capacity_multiplier = if occupancy <= 0.7 {
                1.0
            } else if occupancy <= 1.0 {
                1.0 - (occupancy - 0.7) * 1.4
            } else {
                (0.58 - (occupancy - 1.0) * 1.4).max(0.08)
            };
            let crisis_multiplier = self.birth_multiplier(town_id);
            let probability =
                (self.config.birth_rate(age) * capacity_multiplier * crisis_multiplier)
                    .clamp(0.0, 1.0);
            if !self.world.rng.random_bool(probability) {
                continue;
            }
            let child_id = generate_child(&mut self.world, mother_id, partner_id, &self.config)?;
            if let Some(population) = populations.get_mut(town_index) {
                *population += 1;
            }
            let event = WorldEvent::Birth { npc: child_id };
            statistics.record_event(&event);
            self.change_goal(mother_id, GoalKind::RaiseChildren, true, statistics);
            self.change_goal(partner_id, GoalKind::RaiseChildren, true, statistics);
        }
        Ok(())
    }

    fn birth_multiplier(&self, town: TownId) -> f64 {
        let disease = self
            .world
            .active_diseases
            .iter()
            .filter(|disease| disease.is_infected(town))
            .map(|disease| disease.birth_rate_multiplier())
            .fold(1.0_f64, f64::min);
        let war = self
            .world
            .active_wars
            .iter()
            .filter(|war| war.includes(town))
            .map(WarEvent::birth_rate_multiplier)
            .fold(1.0_f64, f64::min);
        let famine = self
            .world
            .active_famines
            .iter()
            .filter(|famine| famine.town == town)
            .map(FamineEvent::birth_rate_multiplier)
            .fold(1.0_f64, f64::min);
        disease.min(war).min(famine)
    }

    fn process_external_emigration(&mut self, statistics: &mut YearStatistics) {
        let populations = self.world.town_populations();
        let ids = self.world.active_npcs.clone();
        let mut scheduled = vec![false; self.world.npcs.len()];
        for id in ids {
            if scheduled.get(id.0 as usize).copied().unwrap_or(true) {
                continue;
            }
            let Some(npc) = self.world.npc(id).cloned() else {
                continue;
            };
            if !npc.is_adult() {
                continue;
            }
            let town = npc.town;
            let occupancy = self
                .world
                .town(town)
                .map(|value| {
                    value.occupancy(
                        populations
                            .get(town.0 as usize)
                            .copied()
                            .unwrap_or_default(),
                    )
                })
                .unwrap_or(1.0);
            let pressure = if occupancy > 1.0 {
                1.0 + (occupancy - 1.0).min(1.0) * 2.0
            } else {
                1.0
            };
            let crisis = self.emigration_multiplier(town);
            let probability =
                (self.config.annual_emigration_rate() * pressure * crisis).clamp(0.0, 0.5);
            if !self.world.rng.random_bool(probability) {
                continue;
            }
            for member in household_members(&self.world, id) {
                if let Some(flag) = scheduled.get_mut(member.0 as usize) {
                    *flag = true;
                }
                let Some(member_npc) = self.world.npc(member) else {
                    continue;
                };
                let from = member_npc.town;
                if let Some(member_npc) = self.world.npc_mut(member) {
                    if member_npc.leave_world() {
                        let event = WorldEvent::ExternalEmigration { npc: member, from };
                        statistics.record_event(&event);
                        self.world.push_event(event);
                    }
                }
            }
        }
        self.world.rebuild_active_npcs();
    }

    fn process_external_immigration(
        &mut self,
        statistics: &mut YearStatistics,
    ) -> Result<(), PopulationError> {
        if self.world.towns.is_empty() {
            return Ok(());
        }
        let population = self.world.active_population();
        let expected = population as f64 * self.config.annual_immigration_rate();
        let mut count = expected.floor() as usize;
        if self.world.rng.random_bool(expected.fract()) {
            count += 1;
        }
        if population == 0 {
            count = count.max(2);
        }
        let mut populations = self.world.town_populations();
        for _ in 0..count {
            let start = self.world.rng.random_range(0..self.world.towns.len());
            let mut best = start;
            let mut best_score = f64::NEG_INFINITY;
            for offset in 0..self.world.towns.len().min(5) {
                let index = (start + offset) % self.world.towns.len();
                let occupancy = self.world.towns[index].occupancy(populations[index]);
                let sparse_bonus = (1.0 - occupancy.clamp(0.0, 1.0)) * 6.0;
                let score = self.world.towns[index].attractiveness(populations[index])
                    + sparse_bonus
                    + self.world.rng.random_range(0.0..1.0);
                if score > best_score {
                    best = index;
                    best_score = score;
                }
            }
            let town = self.world.towns[best].id;
            let id = generate_external_immigrant(&mut self.world, town, &self.config)?;
            populations[best] += 1;
            let event = WorldEvent::ExternalImmigration { npc: id, to: town };
            statistics.record_event(&event);
        }
        Ok(())
    }

    fn emigration_multiplier(&self, town: TownId) -> f64 {
        let war = self
            .world
            .active_wars
            .iter()
            .filter(|war| war.includes(town))
            .map(WarEvent::emigration_multiplier)
            .fold(1.0_f64, f64::max);
        let famine = self
            .world
            .active_famines
            .iter()
            .filter(|famine| famine.town == town)
            .map(FamineEvent::emigration_multiplier)
            .fold(1.0_f64, f64::max);
        war.max(famine)
    }

    fn progress_diseases_and_famines(&mut self, statistics: &mut YearStatistics) {
        self.spread_diseases(statistics);
        let ids = self.world.active_npcs.clone();
        let mut deaths = BTreeMap::new();
        for id in ids {
            let Some(npc) = self.world.npc(id) else {
                continue;
            };
            let disease_probability = self
                .world
                .active_diseases
                .iter()
                .filter(|disease| disease.is_infected(npc.town))
                .map(|disease| disease.additional_mortality(npc.age, &self.config) / 12.0)
                .fold(0.0_f64, f64::max);
            let famine_probability = self
                .world
                .active_famines
                .iter()
                .filter(|famine| famine.town == npc.town)
                .map(|famine| famine.mortality_probability(&self.config) / 12.0)
                .fold(0.0_f64, f64::max);
            if disease_probability > 0.0 && self.world.rng.random_bool(disease_probability) {
                deaths.insert(id, DeathCause::Disease);
            } else if famine_probability > 0.0 && self.world.rng.random_bool(famine_probability) {
                deaths.insert(id, DeathCause::Famine);
            }
        }
        self.commit_deaths(deaths, statistics);

        self.world
            .active_diseases
            .retain_mut(|disease| disease.progress_month());
        let ended_famines = self
            .world
            .active_famines
            .iter_mut()
            .filter_map(|famine| (!famine.progress_month()).then_some(famine.town))
            .collect::<Vec<_>>();
        self.world
            .active_famines
            .retain(|famine| famine.remaining_months > 0);
        for town in ended_famines {
            self.world.push_event(WorldEvent::FamineEnded { town });
        }
    }

    fn spread_diseases(&mut self, statistics: &mut YearStatistics) {
        if self.world.active_diseases.is_empty() {
            return;
        }
        let populations = self.world.town_populations();
        for disease_index in 0..self.world.active_diseases.len() {
            let disease = self.world.active_diseases[disease_index].clone();
            let mut newly_infected = Vec::new();
            for source in disease.infected_towns_sorted() {
                let source_population = populations
                    .get(source.0 as usize)
                    .copied()
                    .unwrap_or_default();
                if source_population == 0 {
                    continue;
                }
                let Some(source_town) = self.world.town(source) else {
                    continue;
                };
                let source_occupancy = source_town.occupancy(source_population);
                let connections = source_town.neighbors.clone();
                for connection in connections {
                    if disease.is_infected(connection.destination)
                        || newly_infected.contains(&connection.destination)
                    {
                        continue;
                    }
                    let Some(destination) = self.world.town(connection.destination) else {
                        continue;
                    };
                    let destination_population = populations
                        .get(connection.destination.0 as usize)
                        .copied()
                        .unwrap_or_default();
                    if destination_population == 0 {
                        continue;
                    }
                    let destination_occupancy = destination.occupancy(destination_population);
                    let probability = disease.spread_probability(
                        source_occupancy,
                        destination_occupancy,
                        connection.distance,
                        &self.config,
                    );
                    if self.world.rng.random_bool(probability) {
                        newly_infected.push(connection.destination);
                    }
                }
            }
            newly_infected.sort_unstable();
            newly_infected.dedup();
            let damage = disease.town_damage();
            for town_id in newly_infected {
                self.world.active_diseases[disease_index].infect(town_id);
                if let Some(town) = self.world.town_mut(town_id) {
                    town.apply_damage(damage);
                }
                let affected = self.world.residents_by_town()[town_id.0 as usize].clone();
                for id in affected {
                    self.change_goal(id, GoalKind::Survive, true, statistics);
                    self.change_belief(id, BeliefKind::HelpOthers, 1, statistics);
                }
            }
        }
    }

    fn process_social_events(
        &mut self,
        actions: &[Option<Action>],
        statistics: &mut YearStatistics,
    ) {
        let residents = self.world.residents_by_town();
        let ids = self.world.active_npcs.clone();
        for id in ids {
            let selected_target = match actions.get(id.0 as usize).copied().flatten() {
                Some(Action::Socialize) => None,
                Some(Action::HelpPerson(target)) => Some(target),
                _ => continue,
            };
            if !self
                .world
                .rng
                .random_bool(self.config.monthly_social_probability())
            {
                continue;
            }
            let Some(npc) = self.world.npc(id).cloned() else {
                continue;
            };
            let town_index = npc.town.0 as usize;
            let Some(local) = residents.get(town_index) else {
                continue;
            };
            if local.len() < 2 {
                continue;
            }
            let mut target = selected_target
                .filter(|target| local.binary_search(target).is_ok())
                .unwrap_or_else(|| local[self.world.rng.random_range(0..local.len())]);
            if target == id {
                let position = local
                    .iter()
                    .position(|candidate| *candidate == id)
                    .unwrap_or(0);
                target = local[(position + 1) % local.len()];
            }
            if target == id {
                continue;
            }
            let affinity = self.calculate_affinity(id, target);
            let roll = self.world.rng.random_range(0..100u8);
            let delta = if roll < affinity.saturating_add(3) {
                1
            } else if roll > 96u8.saturating_sub(10u8.saturating_sub(affinity)) {
                -1
            } else {
                0
            };
            let year = self.world.year;
            let month = self.world.month;
            let Some((a, b)) = self.world.two_npcs_mut(id, target) else {
                continue;
            };
            let mut changed = false;
            for (npc, other) in [(a, target), (b, id)] {
                let relationship = npc
                    .ensure_relationship(other, affinity, year, month)
                    .expect("other NPC differs");
                relationship.record_interaction(year, month);
                if delta != 0 {
                    changed |= relationship.adjust_relation(delta) != 0;
                    update_relationship_kind(relationship);
                }
            }
            if changed {
                let event = WorldEvent::RelationshipChanged { a: id, b: target };
                statistics.record_event(&event);
                self.world.push_event(event);
            }
            self.enforce_relationship_limit(id);
            self.enforce_relationship_limit(target);
        }
    }

    fn calculate_affinity(&self, a: NpcId, b: NpcId) -> u8 {
        let (Some(a), Some(b)) = (self.world.npc(a), self.world.npc(b)) else {
            return 5;
        };
        let shared_beliefs = a
            .beliefs
            .iter()
            .filter(|belief| b.beliefs.iter().any(|other| other.kind == belief.kind))
            .count() as i16;
        let charisma_difference =
            (i16::from(a.attributes.charisma) - i16::from(b.attributes.charisma)).abs();
        (5 + shared_beliefs - charisma_difference / 4).clamp(0, 10) as u8
    }

    fn enforce_relationship_limit(&mut self, id: NpcId) {
        let limit = self.config.max_relationships_per_npc.max(1);
        let Some(npc) = self.world.npc(id) else {
            return;
        };
        if npc.relationships.len() <= limit {
            return;
        }
        let mut candidates = npc
            .relationships
            .iter()
            .filter(|(_, relationship)| !relationship.is_permanent())
            .map(|(&other, relationship)| {
                (
                    relationship.relation,
                    relationship.last_interaction_year,
                    relationship.last_interaction_month,
                    other,
                )
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable();
        let remove_count = npc.relationships.len().saturating_sub(limit);
        let remove = candidates
            .into_iter()
            .take(remove_count)
            .map(|(_, _, _, other)| other)
            .collect::<Vec<_>>();
        for other in remove {
            if let Some(npc) = self.world.npc_mut(id) {
                npc.remove_relationship(other);
            }
            if let Some(other_npc) = self.world.npc_mut(other) {
                let permanent = other_npc
                    .relationship(id)
                    .is_some_and(Relationship::is_permanent);
                if !permanent {
                    other_npc.remove_relationship(id);
                }
            }
        }
    }

    fn process_partnerships(
        &mut self,
        actions: &[Option<Action>],
        statistics: &mut YearStatistics,
    ) {
        let residents = self.world.residents_by_town();
        let ids = self.world.active_npcs.clone();
        let mut paired = HashSet::new();
        for id in ids {
            if !matches!(
                actions.get(id.0 as usize).copied().flatten(),
                Some(Action::SeekPartner)
            ) || paired.contains(&id)
                || !self
                    .world
                    .rng
                    .random_bool(self.config.monthly_partnership_probability())
            {
                continue;
            }
            let Some(npc) = self.world.npc(id).cloned() else {
                continue;
            };
            if npc.partner.is_some() || !(18..=65).contains(&npc.age) {
                continue;
            }
            let town = npc.town;
            let Some(local) = residents.get(town.0 as usize) else {
                continue;
            };
            let mut best: Option<(i16, NpcId)> = None;
            let start = self.world.rng.random_range(0..local.len().max(1));
            for offset in 0..local.len().min(12) {
                let candidate_id = local[(start + offset) % local.len()];
                if candidate_id == id || paired.contains(&candidate_id) {
                    continue;
                }
                if !matches!(
                    actions.get(candidate_id.0 as usize).copied().flatten(),
                    Some(Action::SeekPartner)
                ) {
                    continue;
                }
                let Some(candidate) = self.world.npc(candidate_id) else {
                    continue;
                };
                if candidate.partner.is_some()
                    || candidate.sex == npc.sex
                    || !(18..=65).contains(&candidate.age)
                    || npc.age.abs_diff(candidate.age) > 20
                    || are_close_relatives(&npc, candidate)
                {
                    continue;
                }
                let existing = npc
                    .relationship(candidate_id)
                    .map_or(5, |relationship| relationship.relation);
                let affinity = npc
                    .relationship(candidate_id)
                    .map_or_else(|| self.calculate_affinity(id, candidate_id), |r| r.affinity);
                let shared = npc
                    .beliefs
                    .iter()
                    .filter(|belief| {
                        candidate
                            .beliefs
                            .iter()
                            .any(|other| other.kind == belief.kind)
                    })
                    .count() as i16;
                let goal_bonus = i16::from(matches!(npc.goal.kind, GoalKind::FindPartner))
                    + i16::from(matches!(candidate.goal.kind, GoalKind::FindPartner));
                let score = i16::from(existing) + i16::from(affinity) + shared + goal_bonus
                    - i16::from(npc.age.abs_diff(candidate.age) / 5);
                if best.is_none_or(|current| {
                    (score, std::cmp::Reverse(candidate_id))
                        > (current.0, std::cmp::Reverse(current.1))
                }) {
                    best = Some((score, candidate_id));
                }
            }
            let Some((score, partner_id)) = best else {
                continue;
            };
            if score < 10
                || !self
                    .world
                    .rng
                    .random_bool(((score - 8) as f64 * 0.08).clamp(0.08, 0.75))
            {
                continue;
            }
            let affinity = self.calculate_affinity(id, partner_id);
            let relationship = Relationship::with_relation(
                affinity,
                9,
                RelationshipKind::Partner,
                self.world.year,
                self.world.month,
            );
            if let Some((a, b)) = self.world.two_npcs_mut(id, partner_id) {
                if a.partner.is_some() || b.partner.is_some() {
                    continue;
                }
                a.set_partner(partner_id);
                b.set_partner(id);
                a.set_relationship(partner_id, relationship);
                b.set_relationship(id, relationship);
            }
            paired.insert(id);
            paired.insert(partner_id);
            let event = WorldEvent::Partnership {
                a: id,
                b: partner_id,
            };
            statistics.record_event(&event);
            self.world.push_event(event);
            self.change_goal(id, GoalKind::RaiseChildren, true, statistics);
            self.change_goal(partner_id, GoalKind::RaiseChildren, true, statistics);
        }
    }

    fn process_internal_migration(
        &mut self,
        actions: &[Option<Action>],
        statistics: &mut YearStatistics,
    ) {
        if self.world.towns.len() < 2 {
            return;
        }
        let ids = self.world.active_npcs.clone();
        let mut scheduled = vec![false; self.world.npcs.len()];
        for id in ids {
            if scheduled.get(id.0 as usize).copied().unwrap_or(true) {
                continue;
            }
            let Some(npc) = self.world.npc(id).cloned() else {
                continue;
            };
            if !npc.is_adult() {
                continue;
            }
            let (destination, fleeing) = match actions.get(id.0 as usize).copied().flatten() {
                Some(Action::MoveTown(destination)) => (destination, false),
                Some(Action::FleeTown(destination)) => (destination, true),
                _ => continue,
            };
            let disease_multiplier = self
                .world
                .active_diseases
                .iter()
                .filter(|disease| disease.is_infected(npc.town))
                .map(|disease| disease.mobility_multiplier())
                .fold(1.0_f64, f64::min);
            let intent_multiplier = if fleeing { 80.0 } else { 8.0 };
            let probability = (self.config.monthly_migration_probability()
                * intent_multiplier
                * disease_multiplier)
                .clamp(0.0, if fleeing { 0.75 } else { 0.25 });
            if !self.world.rng.random_bool(probability) {
                continue;
            }
            if destination == npc.town || self.world.town(destination).is_none() {
                continue;
            }
            let from = npc.town;
            let members = household_members(&self.world, id);
            for member in &members {
                if let Some(flag) = scheduled.get_mut(member.0 as usize) {
                    *flag = true;
                }
            }
            let moved = move_household(&mut self.world, id, destination);
            if !moved.is_empty() {
                self.spread_disease_via_migration(from, destination, statistics);
            }
            for member in moved {
                let event = WorldEvent::Migration {
                    npc: member,
                    from,
                    to: destination,
                };
                statistics.record_event(&event);
                self.world.push_event(event);
            }
            self.change_goal(id, self.suggest_goal(id), true, statistics);
        }
    }

    fn spread_disease_via_migration(
        &mut self,
        origin: TownId,
        destination: TownId,
        statistics: &mut YearStatistics,
    ) {
        let candidates = self
            .world
            .active_diseases
            .iter()
            .enumerate()
            .filter(|(_, disease)| disease.is_infected(origin) && !disease.is_infected(destination))
            .map(|(index, disease)| {
                (
                    index,
                    (f64::from(disease.severity) * 0.025).clamp(0.0, 0.35),
                    disease.town_damage(),
                )
            })
            .collect::<Vec<_>>();
        for (index, probability, damage) in candidates {
            if !self.world.rng.random_bool(probability) {
                continue;
            }
            self.world.active_diseases[index].infect(destination);
            if let Some(town) = self.world.town_mut(destination) {
                town.apply_damage(damage);
            }
            let affected = self
                .world
                .residents_by_town()
                .get(destination.0 as usize)
                .cloned()
                .unwrap_or_default();
            for id in affected {
                self.change_goal(id, GoalKind::Survive, true, statistics);
                self.change_belief(id, BeliefKind::HelpOthers, 1, statistics);
            }
        }
    }

    /// 月初スナップショットから全NPCのIntentを先に確定する。
    fn choose_utility_actions(&self) -> Vec<Option<Action>> {
        let mut actions = vec![None; self.world.npcs.len()];
        let populations = self.world.town_populations();
        let residents = self.world.residents_by_town();
        for &id in &self.world.active_npcs {
            let Some(npc) = self.world.npc(id).cloned() else {
                continue;
            };
            let Some(town) = self.world.town(npc.town) else {
                continue;
            };
            let danger = 10u8.saturating_sub(town.effective_safety());
            let at_war = self
                .world
                .active_wars
                .iter()
                .any(|war| war.includes(npc.town));
            let disease_outbreak = self
                .world
                .active_diseases
                .iter()
                .any(|disease| disease.is_infected(npc.town));
            let mut people = sorted_relationship_candidates(&npc)
                .into_iter()
                .filter(|person| {
                    self.world
                        .npc(person.npc)
                        .is_some_and(|other| other.is_active() && other.town == npc.town)
                })
                .take(4)
                .collect::<Vec<_>>();
            let mut seen_people = people.iter().map(|person| person.npc).collect::<Vec<_>>();
            if let Some(local) = residents.get(npc.town.0 as usize) {
                let position = local.binary_search(&id).unwrap_or(0);
                for offset in 1..=local.len().saturating_sub(1).min(4) {
                    let other_id = local[(position + offset) % local.len()];
                    if seen_people.contains(&other_id) {
                        continue;
                    }
                    let person = npc.relationship(other_id).map_or_else(
                        || crate::utility::PersonCandidate::new(other_id, 5).as_outsider(),
                        |relationship| {
                            crate::utility::PersonCandidate::from_relationship(
                                other_id,
                                relationship,
                            )
                        },
                    );
                    people.push(person);
                    seen_people.push(other_id);
                }
            }
            for person in &mut people {
                let Some(other) = self.world.npc(person.npc) else {
                    continue;
                };
                person.partner_eligible = eligible_partners(&npc, other);
                if person.is_family && danger >= 5 {
                    person.needs_help = true;
                    person.danger = danger;
                }
            }
            let needs_move_candidates = danger >= 5 || npc.goal.kind == GoalKind::MoveToBetterTown;
            let move_candidates = if needs_move_candidates {
                candidate_towns(&self.world, npc.town)
                    .into_iter()
                    .take(8)
                    .filter_map(|(town_id, distance)| {
                        let destination = self.world.town(town_id)?;
                        let attractiveness = destination.attractiveness(
                            populations
                                .get(town_id.0 as usize)
                                .copied()
                                .unwrap_or_default(),
                        ) as f32;
                        let known_people =
                            npc.relationships
                                .keys()
                                .filter(|&&other_id| {
                                    self.world.npc(other_id).is_some_and(|other| {
                                        other.is_active() && other.town == town_id
                                    })
                                })
                                .count()
                                .min(u16::MAX as usize) as u16;
                        let compatibility = npc
                            .beliefs
                            .iter()
                            .filter(|belief| {
                                destination
                                    .culture
                                    .iter()
                                    .any(|item| item.kind == belief.kind)
                            })
                            .count() as f32
                            * 0.35;
                        Some(
                            MoveCandidate::new(town_id, attractiveness, distance)
                                .with_known_people(known_people)
                                .with_belief_compatibility(compatibility)
                                .safe_for_fleeing(destination.effective_safety() >= 5),
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let family_needs_care = people
                .iter()
                .any(|person| person.is_family && person.needs_help);
            let situation = Situation {
                danger,
                tiredness: ((u32::from(self.world.month) + id.0) % 11) as u8,
                at_war,
                natural_disaster: town.temporary_damage.safety_loss >= 3,
                disease_outbreak,
                has_work: town.effective_jobs() > 0,
                socializing_allowed: !disease_outbreak,
                family_needs_care,
                can_move: self.world.towns.len() > 1,
                can_defend_town: npc.attributes.physical >= 3,
                people,
                move_candidates,
            };
            actions[id.0 as usize] = choose_action(&npc, &situation);
        }
        actions
    }

    fn apply_utility_outcomes(&mut self, actions: &[Option<Action>]) {
        let ids = self.world.active_npcs.clone();
        for id in ids {
            let action = actions.get(id.0 as usize).copied().flatten();
            let Some(snapshot) = self.world.npc(id) else {
                continue;
            };
            let danger = self
                .world
                .town(snapshot.town)
                .map_or(0, |town| 10u8.saturating_sub(town.effective_safety()));
            let disease_outbreak = self
                .world
                .active_diseases
                .iter()
                .any(|disease| disease.is_infected(snapshot.town));
            if let Some(npc) = self.world.npc_mut(id) {
                npc.state = if matches!(action, Some(Action::FleeTown(_))) || danger >= 8 {
                    NpcState::Evacuating
                } else if disease_outbreak {
                    NpcState::Sick
                } else {
                    NpcState::Normal
                };
                let progress = match action {
                    Some(Action::Work)
                        if matches!(
                            npc.goal.kind,
                            GoalKind::BecomeSkilled
                                | GoalKind::GainWealth
                                | GoalKind::GainStatus
                                | GoalKind::SeekKnowledge
                        ) =>
                    {
                        0.002
                    }
                    Some(Action::SeekPartner) if npc.goal.kind == GoalKind::FindPartner => 0.002,
                    Some(Action::CareForFamily | Action::HelpPerson(_))
                        if matches!(
                            npc.goal.kind,
                            GoalKind::ProtectFamily | GoalKind::RaiseChildren
                        ) =>
                    {
                        0.003
                    }
                    Some(Action::DefendTown) if npc.goal.kind == GoalKind::ProtectTown => 0.003,
                    _ => 0.0,
                };
                npc.goal.advance(progress);
            }
        }
    }

    fn progress_wars(&mut self) {
        let mut ended = 0usize;
        self.world.active_wars.retain_mut(|war| {
            let active = war.progress_year();
            ended += usize::from(!active);
            active
        });
        for _ in 0..ended {
            self.world.push_event(WorldEvent::WarEnded);
        }
    }

    fn reassess_goals(&mut self, statistics: &mut YearStatistics) {
        let ids = self.world.active_npcs.clone();
        for id in ids {
            let Some(npc) = self.world.npc(id) else {
                continue;
            };
            let complete = npc.goal.is_complete();
            let due = complete
                || self
                    .world
                    .rng
                    .random_bool(self.config.goal_reassessment_probability.clamp(0.0, 1.0));
            if due {
                let goal = self.suggest_goal(id);
                self.change_goal(id, goal, complete, statistics);
            }
        }
    }

    fn suggest_goal(&self, id: NpcId) -> GoalKind {
        let Some(npc) = self.world.npc(id) else {
            return GoalKind::Survive;
        };
        let danger = self
            .world
            .town(npc.town)
            .map_or(0, |town| 10u8.saturating_sub(town.effective_safety()));
        #[cfg(feature = "economy-extension")]
        let economic_hardship = self.world.town(npc.town).is_some_and(|town| {
            npc.money_cents
                < town
                    .economy
                    .indexed_price(self.config.base_monthly_living_cost_cents)
                    .saturating_mul(2)
        });
        #[cfg(not(feature = "economy-extension"))]
        let economic_hardship = false;
        if danger >= 6 {
            GoalKind::Survive
        } else if npc.age < 18 {
            GoalKind::BecomeSkilled
        } else if npc.partner.is_none() && npc.age <= 60 {
            GoalKind::FindPartner
        } else if !npc.children.is_empty()
            && npc.children.iter().any(|&child| {
                self.world
                    .npc(child)
                    .is_some_and(|child| child.is_active() && child.age < 18)
            })
        {
            GoalKind::RaiseChildren
        } else if economic_hardship {
            GoalKind::GainWealth
        } else if npc.belief_strength(BeliefKind::ValueKnowledge) >= 7 {
            GoalKind::SeekKnowledge
        } else if npc.belief_strength(BeliefKind::ValueWealth) >= 7 {
            GoalKind::GainWealth
        } else if npc.age >= 65 {
            GoalKind::LivePeacefully
        } else {
            GoalKind::GainStatus
        }
    }

    fn change_goal(
        &mut self,
        id: NpcId,
        new_goal: GoalKind,
        major: bool,
        statistics: &mut YearStatistics,
    ) {
        let year = self.world.year;
        let cooldown = self.config.goal_change_cooldown_years;
        let Some(npc) = self.world.npc(id) else {
            return;
        };
        let old = npc.goal.kind;
        if let Some(npc) = self.world.npc_mut(id) {
            if npc.change_goal(new_goal, year, cooldown, major) {
                let event = WorldEvent::GoalChanged {
                    npc: id,
                    old,
                    new: new_goal,
                };
                statistics.record_event(&event);
                self.world.push_event(event);
            }
        }
    }

    fn change_belief(
        &mut self,
        id: NpcId,
        kind: BeliefKind,
        delta: i8,
        statistics: &mut YearStatistics,
    ) {
        let Some(npc) = self.world.npc_mut(id) else {
            return;
        };
        let changed = if let Some(actual) = npc.adjust_belief(kind, delta) {
            actual != 0
        } else if npc.beliefs.len() < self.config.max_beliefs.min(3) {
            npc.add_belief(Belief::new(kind, 5))
        } else {
            false
        };
        if changed {
            let event = WorldEvent::BeliefChanged {
                npc: id,
                belief: kind,
            };
            statistics.record_event(&event);
            self.world.push_event(event);
        }
    }

    fn forget_relationships(&mut self) {
        let year = self.world.year;
        let ids = self.world.active_npcs.clone();
        let mut edges = Vec::new();
        for id in ids {
            let Some(npc) = self.world.npc(id) else {
                continue;
            };
            let mut forgotten = npc
                .relationships
                .iter()
                .filter_map(|(&other, relationship)| {
                    self.config
                        .should_forget_relationship(
                            relationship.relation,
                            relationship.last_interaction_year,
                            year,
                            relationship.is_permanent(),
                        )
                        .then_some(other)
                })
                .collect::<Vec<_>>();
            forgotten.sort_unstable();
            if let Some(npc) = self.world.npc_mut(id) {
                for other in forgotten {
                    npc.remove_relationship(other);
                    edges.push((id, other));
                }
            }
        }
        edges.sort_unstable();
        for (a, b) in edges {
            if let Some(other) = self.world.npc_mut(b) {
                let permanent = other
                    .relationship(a)
                    .is_some_and(Relationship::is_permanent);
                if !permanent {
                    other.remove_relationship(a);
                }
            }
        }
    }
}

fn update_relationship_kind(relationship: &mut Relationship) {
    if relationship.is_permanent() {
        return;
    }
    relationship.kind = match relationship.relation {
        0..=1 => RelationshipKind::Enemy,
        2..=3 => RelationshipKind::Rival,
        8 => RelationshipKind::Friend,
        9..=10 => RelationshipKind::CloseFriend,
        _ => RelationshipKind::Acquaintance,
    };
}

fn eligible_partners(a: &Npc, b: &Npc) -> bool {
    a.id != b.id
        && a.is_active()
        && b.is_active()
        && a.town == b.town
        && a.partner.is_none()
        && b.partner.is_none()
        && a.sex != b.sex
        && (18..=65).contains(&a.age)
        && (18..=65).contains(&b.age)
        && a.age.abs_diff(b.age) <= 20
        && !are_close_relatives(a, b)
}

fn are_close_relatives(a: &Npc, b: &Npc) -> bool {
    a.parents.contains(&b.id)
        || a.children.contains(&b.id)
        || b.parents.contains(&a.id)
        || b.children.contains(&a.id)
        || a.parents.iter().any(|parent| b.parents.contains(parent))
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SimulationResult<'a> {
    pub seed: u64,
    pub config: &'a SimulationConfig,
    pub statistics: &'a [YearStatistics],
    pub health: SimulationHealthMetrics,
}

impl Simulation {
    pub fn result(&self) -> SimulationResult<'_> {
        SimulationResult {
            seed: self.seed,
            config: &self.config,
            statistics: &self.world.statistics.years,
            health: self.health_metrics(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulation_can_advance_beyond_the_previous_u16_year_limit() {
        let mut simulation = Simulation::new(1, 20, 7, SimulationConfig::normal()).unwrap();
        simulation.world.year = u64::from(u16::MAX);

        simulation.run_year().unwrap();

        assert_eq!(simulation.world.year, u64::from(u16::MAX) + 1);
    }

    #[test]
    fn run_year_replaces_the_year_event_buffer() {
        let mut simulation = Simulation::new(1, 20, 7, SimulationConfig::normal()).unwrap();
        simulation.world.capture_year_events = true;
        simulation.world.push_event(WorldEvent::DiseaseOutbreak);
        assert_eq!(simulation.world.year_events[0].year, 0);

        simulation.run_year().unwrap();

        assert!(
            simulation
                .world
                .year_events
                .iter()
                .all(|timed| timed.year == 1)
        );
        assert!(
            simulation
                .world
                .important_events
                .contains(&WorldEvent::DiseaseOutbreak)
        );
    }

    #[test]
    fn utility_snapshot_contains_real_social_and_partner_intents() {
        let simulation = Simulation::new(1, 200, 123, SimulationConfig::normal()).unwrap();
        let actions = simulation.choose_utility_actions();
        let eligible = simulation
            .world
            .active_npcs
            .iter()
            .filter(|&&id| {
                let npc = simulation.world.npc(id).unwrap();
                simulation
                    .world
                    .active_npcs
                    .iter()
                    .any(|&other| eligible_partners(npc, simulation.world.npc(other).unwrap()))
            })
            .count();
        let find_partner_goals = simulation
            .world
            .active_npcs
            .iter()
            .filter(|&&id| simulation.world.npc(id).unwrap().goal.kind == GoalKind::FindPartner)
            .count();
        assert!(
            actions
                .iter()
                .flatten()
                .any(|action| matches!(action, Action::Socialize | Action::HelpPerson(_)))
        );
        assert!(
            actions
                .iter()
                .flatten()
                .any(|action| matches!(action, Action::SeekPartner)),
            "独身成人のUtilityにSeekPartner intentが必要: eligible={eligible}, goals={find_partner_goals}, actions={actions:?}"
        );
    }
}
