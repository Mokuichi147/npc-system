use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

use crate::disaster::FamineEvent;
use crate::disease::DiseaseEvent;
use crate::event::{TimedWorldEvent, WorldEvent};
use crate::id::{NpcId, TownId};
use crate::npc::{Npc, NpcInvariantError};
use crate::statistics::Statistics;
use crate::town::Town;
use crate::war::WarEvent;

/// シミュレーションの全状態。
///
/// NPCはIDとVec indexを一致させ、死亡・転出後もremoveしない。
pub struct World {
    pub year: u64,
    pub month: u8,
    pub npcs: Vec<Npc>,
    pub towns: Vec<Town>,
    pub active_npcs: Vec<NpcId>,
    pub active_diseases: Vec<DiseaseEvent>,
    pub active_wars: Vec<WarEvent>,
    pub active_famines: Vec<FamineEvent>,
    pub statistics: Statistics,
    pub important_events: Vec<WorldEvent>,
    /// 現在年に発生した全イベント。年次処理の開始時にclearする。
    pub year_events: Vec<TimedWorldEvent>,
    /// タイムライン用途で年次イベントを収集するか。
    pub capture_year_events: bool,
    pub rng: ChaCha8Rng,
}

impl World {
    pub const MAX_RETAINED_EVENTS: usize = 4_096;

    pub fn empty(seed: u64) -> Self {
        Self {
            year: 0,
            month: 0,
            npcs: Vec::new(),
            towns: Vec::new(),
            active_npcs: Vec::new(),
            active_diseases: Vec::new(),
            active_wars: Vec::new(),
            active_famines: Vec::new(),
            statistics: Statistics::default(),
            important_events: Vec::new(),
            year_events: Vec::new(),
            capture_year_events: false,
            rng: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    pub fn npc(&self, id: NpcId) -> Option<&Npc> {
        self.npcs.get(id.0 as usize).filter(|npc| npc.id == id)
    }

    pub fn npc_mut(&mut self, id: NpcId) -> Option<&mut Npc> {
        self.npcs.get_mut(id.0 as usize).filter(|npc| npc.id == id)
    }

    pub fn two_npcs_mut(&mut self, a: NpcId, b: NpcId) -> Option<(&mut Npc, &mut Npc)> {
        if a == b {
            return None;
        }
        let ai = a.0 as usize;
        let bi = b.0 as usize;
        if ai >= self.npcs.len() || bi >= self.npcs.len() {
            return None;
        }
        if ai < bi {
            let (left, right) = self.npcs.split_at_mut(bi);
            let first = &mut left[ai];
            let second = &mut right[0];
            (first.id == a && second.id == b).then_some((first, second))
        } else {
            let (left, right) = self.npcs.split_at_mut(ai);
            let second = &mut left[bi];
            let first = &mut right[0];
            (first.id == a && second.id == b).then_some((first, second))
        }
    }

    pub fn town(&self, id: TownId) -> Option<&Town> {
        self.towns.get(id.0 as usize).filter(|town| town.id == id)
    }

    pub fn town_mut(&mut self, id: TownId) -> Option<&mut Town> {
        self.towns
            .get_mut(id.0 as usize)
            .filter(|town| town.id == id)
    }

    pub fn active_population(&self) -> usize {
        self.active_npcs.len()
    }

    pub fn total_unique_npcs(&self) -> usize {
        self.npcs.len()
    }

    pub fn town_populations(&self) -> Vec<usize> {
        let mut populations = vec![0; self.towns.len()];
        for &id in &self.active_npcs {
            if let Some(npc) = self.npc(id) {
                if let Some(value) = populations.get_mut(npc.town.0 as usize) {
                    *value += 1;
                }
            }
        }
        populations
    }

    pub fn residents_by_town(&self) -> Vec<Vec<NpcId>> {
        let mut residents = vec![Vec::new(); self.towns.len()];
        for &id in &self.active_npcs {
            if let Some(npc) = self.npc(id) {
                if let Some(bucket) = residents.get_mut(npc.town.0 as usize) {
                    bucket.push(id);
                }
            }
        }
        residents
    }

    pub fn rebuild_active_npcs(&mut self) {
        self.active_npcs.clear();
        self.active_npcs.extend(
            self.npcs
                .iter()
                .filter(|npc| npc.alive && npc.in_world)
                .map(|npc| npc.id),
        );
    }

    pub fn push_event(&mut self, event: WorldEvent) {
        if self.capture_year_events {
            self.year_events
                .push(TimedWorldEvent::new(self.year, self.month, event.clone()));
        }
        if self.important_events.len() == Self::MAX_RETAINED_EVENTS {
            let remove = Self::MAX_RETAINED_EVENTS / 4;
            self.important_events.drain(..remove);
        }
        self.important_events.push(event);
    }

    pub fn validate(&self) -> Result<(), InvariantError> {
        for (index, town) in self.towns.iter().enumerate() {
            if town.id.0 as usize != index {
                return Err(InvariantError::InvalidTownId(town.id));
            }
            if town.population_capacity == 0
                || [
                    town.jobs,
                    town.safety,
                    town.education,
                    town.freedom,
                    town.wealth,
                ]
                .into_iter()
                .any(|value| value > 10)
            {
                return Err(InvariantError::InvalidTownState(town.id));
            }
            for connection in &town.neighbors {
                if connection.destination == town.id
                    || !(1..=10).contains(&connection.distance)
                    || self.town(connection.destination).is_none()
                {
                    return Err(InvariantError::InvalidTownConnection(
                        town.id,
                        connection.destination,
                    ));
                }
            }
        }
        let mut active = vec![false; self.npcs.len()];
        for &id in &self.active_npcs {
            let index = id.0 as usize;
            let Some(npc) = self.npcs.get(index) else {
                return Err(InvariantError::InvalidNpcId(id));
            };
            if npc.id != id || !npc.alive || !npc.in_world {
                return Err(InvariantError::InvalidActiveNpc(id));
            }
            if std::mem::replace(&mut active[index], true) {
                return Err(InvariantError::DuplicateActiveNpc(id));
            }
        }

        for (index, npc) in self.npcs.iter().enumerate() {
            if npc.id.0 as usize != index {
                return Err(InvariantError::InvalidNpcId(npc.id));
            }
            if npc.alive && npc.in_world && !active[index] {
                return Err(InvariantError::MissingActiveNpc(npc.id));
            }
            if npc.in_world && self.town(npc.town).is_none() {
                return Err(InvariantError::InvalidTown(npc.id, npc.town));
            }
            if !npc.values_in_range() {
                return Err(InvariantError::ValueOutOfRange(npc.id));
            }
            npc.validate()
                .map_err(|error| InvariantError::InvalidNpcState(npc.id, error))?;
            if npc.relationships.contains_key(&npc.id) {
                return Err(InvariantError::SelfRelationship(npc.id));
            }
            for &other_id in npc.relationships.keys() {
                if self.npc(other_id).is_none() {
                    return Err(InvariantError::InvalidRelationshipTarget(npc.id, other_id));
                }
            }
            if let Some(partner_id) = npc.partner {
                let Some(partner) = self.npc(partner_id) else {
                    return Err(InvariantError::InvalidNpcId(partner_id));
                };
                if !partner.alive || partner.partner != Some(npc.id) {
                    return Err(InvariantError::AsymmetricPartner(npc.id, partner_id));
                }
                if npc.parents.contains(&partner_id)
                    || npc.children.contains(&partner_id)
                    || partner.parents.contains(&npc.id)
                    || partner.children.contains(&npc.id)
                    || npc
                        .parents
                        .iter()
                        .any(|parent| partner.parents.contains(parent))
                {
                    return Err(InvariantError::RelatedPartners(npc.id, partner_id));
                }
            }
            for &parent_id in &npc.parents {
                let Some(parent) = self.npc(parent_id) else {
                    return Err(InvariantError::InvalidNpcId(parent_id));
                };
                if !parent.children.contains(&npc.id) {
                    return Err(InvariantError::BrokenParentChild(parent_id, npc.id));
                }
            }
            for &child_id in &npc.children {
                let Some(child) = self.npc(child_id) else {
                    return Err(InvariantError::InvalidNpcId(child_id));
                };
                if !child.parents.contains(&npc.id) {
                    return Err(InvariantError::BrokenParentChild(npc.id, child_id));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InvariantError {
    #[error("存在しないNPC ID: {0:?}")]
    InvalidNpcId(NpcId),
    #[error("active_npcsの状態が不正: {0:?}")]
    InvalidActiveNpc(NpcId),
    #[error("active_npcsに重複: {0:?}")]
    DuplicateActiveNpc(NpcId),
    #[error("生存中のNPCがactive_npcsにない: {0:?}")]
    MissingActiveNpc(NpcId),
    #[error("NPC {0:?} の都市ID {1:?} が不正")]
    InvalidTown(NpcId, TownId),
    #[error("都市IDとVec indexが一致しない: {0:?}")]
    InvalidTownId(TownId),
    #[error("都市 {0:?} の状態値が不正")]
    InvalidTownState(TownId),
    #[error("都市接続が不正: {0:?} -> {1:?}")]
    InvalidTownConnection(TownId, TownId),
    #[error("NPC {0:?} の0..=10値が範囲外")]
    ValueOutOfRange(NpcId),
    #[error("NPC {0:?} 自体の状態が不正: {1:?}")]
    InvalidNpcState(NpcId, NpcInvariantError),
    #[error("NPC {0:?} が自分自身への関係を保持")]
    SelfRelationship(NpcId),
    #[error("NPC {0:?} の関係先 {1:?} が存在しない")]
    InvalidRelationshipTarget(NpcId, NpcId),
    #[error("パートナー関係が非対称: {0:?} / {1:?}")]
    AsymmetricPartner(NpcId, NpcId),
    #[error("近親NPC間のパートナー関係: {0:?} / {1:?}")]
    RelatedPartners(NpcId, NpcId),
    #[error("親子関係が非対称: parent={0:?}, child={1:?}")]
    BrokenParentChild(NpcId, NpcId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushed_events_keep_the_current_year_and_month() {
        let mut world = World::empty(1);
        world.capture_year_events = true;
        world.year = 12;
        world.month = 7;
        world.push_event(WorldEvent::DiseaseOutbreak);

        assert_eq!(world.important_events, vec![WorldEvent::DiseaseOutbreak]);
        assert_eq!(world.year_events.len(), 1);
        assert_eq!(world.year_events[0].year, 12);
        assert_eq!(world.year_events[0].month, 7);
        assert_eq!(world.year_events[0].event, WorldEvent::DiseaseOutbreak);
    }
}
