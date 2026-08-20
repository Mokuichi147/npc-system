use std::collections::BTreeMap;

use crate::id::{NpcId, TownId};
use crate::world::World;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MigrationChoice {
    pub destination: TownId,
    pub score: f32,
    pub distance: u8,
}

/// 隣接都市と2-hop都市だけを、最短距離付きで返す。
pub fn candidate_towns(world: &World, origin: TownId) -> Vec<(TownId, u8)> {
    let Some(origin_town) = world.town(origin) else {
        return Vec::new();
    };
    let mut candidates = BTreeMap::<TownId, u8>::new();
    for connection in &origin_town.neighbors {
        if connection.destination != origin {
            candidates
                .entry(connection.destination)
                .and_modify(|distance| *distance = (*distance).min(connection.distance))
                .or_insert(connection.distance);
        }
        if let Some(neighbor) = world.town(connection.destination) {
            for second in &neighbor.neighbors {
                if second.destination == origin {
                    continue;
                }
                let distance = connection.distance.saturating_add(second.distance).min(10);
                candidates
                    .entry(second.destination)
                    .and_modify(|current| *current = (*current).min(distance))
                    .or_insert(distance);
            }
        }
    }
    candidates.into_iter().collect()
}

pub fn best_destination(
    world: &World,
    npc_id: NpcId,
    populations: &[usize],
) -> Option<MigrationChoice> {
    let npc = world.npc(npc_id)?;
    let origin_population = populations
        .get(npc.town.0 as usize)
        .copied()
        .unwrap_or_default();
    let origin_score = world.town(npc.town)?.attractiveness(origin_population) as f32;

    candidate_towns(world, npc.town)
        .into_iter()
        .filter_map(|(destination, distance)| {
            let town = world.town(destination)?;
            let population = populations
                .get(destination.0 as usize)
                .copied()
                .unwrap_or_default();
            let known_people = npc
                .relationships
                .keys()
                .filter(|&&other_id| {
                    world.npc(other_id).is_some_and(|other| {
                        other.alive && other.in_world && other.town == destination
                    })
                })
                .count()
                .min(3) as f32;
            let culture_bonus = npc
                .beliefs
                .iter()
                .filter(|belief| town.culture.iter().any(|item| item.kind == belief.kind))
                .count() as f32
                * 0.35;
            let score = town.attractiveness(population) as f32 - origin_score
                + known_people * 0.6
                + culture_bonus
                - distance as f32 * 0.35
                - 0.75;
            Some(MigrationChoice {
                destination,
                score,
                distance,
            })
        })
        .max_by(|a, b| {
            a.score
                .total_cmp(&b.score)
                .then_with(|| b.destination.cmp(&a.destination))
        })
}

/// パートナーと同居する未成年の子供を含む移住単位を作る。
pub fn household_members(world: &World, leader_id: NpcId) -> Vec<NpcId> {
    let Some(leader) = world.npc(leader_id) else {
        return Vec::new();
    };
    if !leader.alive || !leader.in_world {
        return Vec::new();
    }
    let origin = leader.town;
    let mut members = vec![leader_id];
    if let Some(partner_id) = leader.partner {
        if world
            .npc(partner_id)
            .is_some_and(|partner| partner.alive && partner.in_world && partner.town == origin)
        {
            members.push(partner_id);
        }
    }

    let adults = members.clone();
    for adult_id in adults {
        if let Some(adult) = world.npc(adult_id) {
            for &child_id in &adult.children {
                if world.npc(child_id).is_some_and(|child| {
                    child.alive && child.in_world && child.age < 18 && child.town == origin
                }) {
                    members.push(child_id);
                }
            }
        }
    }
    members.sort_unstable();
    members.dedup();
    members
}

/// 世帯を即座に移動し、実際に移動したNPCを返す。
pub fn move_household(world: &mut World, leader: NpcId, destination: TownId) -> Vec<NpcId> {
    if world.town(destination).is_none() {
        return Vec::new();
    }
    let members = household_members(world, leader);
    let mut moved = Vec::with_capacity(members.len());
    for id in members {
        if let Some(npc) = world.npc_mut(id) {
            if npc.town != destination && npc.alive && npc.in_world {
                npc.town = destination;
                moved.push(id);
            }
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_world_has_no_candidates() {
        let world = World::empty(1);
        assert!(candidate_towns(&world, TownId(0)).is_empty());
    }
}
