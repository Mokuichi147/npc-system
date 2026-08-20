use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

use crate::belief::{Belief, BeliefKind, MAX_BELIEFS};
use crate::config::SimulationConfig;
use crate::event::WorldEvent;
use crate::goal::{Goal, GoalKind};
use crate::id::{NpcId, TownId};
use crate::npc::{Attributes, Npc, Sex};
use crate::relationship::{Relationship, RelationshipKind};
use crate::statistics::Statistics;
use crate::town::{Town, TownConnection};
use crate::world::{InvariantError, World};

const INITIAL_RELATIONSHIP_MONTH: u8 = 1;
const PARTNER_BUCKET_COUNT: usize = 6;
const MAX_PARENT_CANDIDATES: usize = 12;
const INITIAL_FRIEND_OFFSETS: usize = 2;

/// 初期生成・出生・外部流入で、整合したNPCを作れなかった場合のエラー。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PopulationError {
    #[error("都市数は1以上である必要があります")]
    NoTowns,
    #[error("都市数がTownIdの上限を超えています: {0}")]
    TooManyTowns(usize),
    #[error("人口がNpcIdまたは都市収容力の上限を超えています: {0}")]
    TooManyNpcs(usize),
    #[error("存在しない都市ID: {0:?}")]
    InvalidTown(TownId),
    #[error("存在しないNPC ID: {0:?}")]
    InvalidNpc(NpcId),
    #[error("親には異なる2人のNPCが必要です")]
    SameParent,
    #[error("親NPCが死亡または世界外にいます: {0:?}")]
    InactiveParent(NpcId),
    #[error("親NPCが同じ都市に住んでいません: {0:?} / {1:?}")]
    ParentsInDifferentTowns(NpcId, NpcId),
    #[error(transparent)]
    Invariant(#[from] InvariantError),
}

/// seedから初期都市・NPC・疎な社会関係を一括生成する。
///
/// NPC IDとVec index、Town IDとVec indexは常に一致する。乱数を使う走査は
/// ID順のVecだけを対象にし、HashMapの列挙順には依存しない。
pub fn generate_initial_world(
    town_count: usize,
    population: usize,
    seed: u64,
    config: &SimulationConfig,
) -> Result<World, PopulationError> {
    validate_world_size(town_count, population)?;

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let town_populations = balanced_town_populations(town_count, population);
    let mut towns = generate_towns(&town_populations, &mut rng)?;
    connect_towns(&mut towns, &mut rng);

    let mut npcs = Vec::with_capacity(population);
    for index in 0..population {
        let id = NpcId(index as u32);
        let town = TownId((index % town_count) as u16);
        let age = sample_age(&mut rng);
        let sex = sample_sex(&mut rng);
        let attributes = sample_attributes(&mut rng);
        let beliefs = sample_initial_beliefs(
            &towns[town.0 as usize].culture,
            config.max_beliefs,
            &mut rng,
        );
        let goal = Goal::new(sample_goal(age, &mut rng), 0);
        npcs.push(Npc::with_default_name(
            id, age, sex, town, town, attributes, beliefs, goal,
        ));
    }

    let mut world = World::empty(seed);
    world.towns = towns;
    world.npcs = npcs;
    world.statistics = Statistics::new(population);
    world.rebuild_active_npcs();

    initialize_partners_and_families(&mut world, &mut rng);
    initialize_friendships(&mut world, config, &mut rng);
    align_initial_goals_with_family(&mut world);

    // 初期生成で消費した位置から、通常シミュレーションの乱数列を継続する。
    world.rng = rng;
    world.validate()?;
    Ok(world)
}

/// 生存中かつ同じ都市にいる親2人から、新生児を生成してWorldへ追加する。
///
/// 出生可否（年齢、確率、過密補正）は呼び出し側が判定し、この関数は生成と
/// 親子参照の原子的な整合に責任を持つ。
pub fn generate_child(
    world: &mut World,
    parent_a: NpcId,
    parent_b: NpcId,
    config: &SimulationConfig,
) -> Result<NpcId, PopulationError> {
    if parent_a == parent_b {
        return Err(PopulationError::SameParent);
    }

    let (town, attributes_a, attributes_b, beliefs_a, beliefs_b) = {
        let a = world
            .npc(parent_a)
            .ok_or(PopulationError::InvalidNpc(parent_a))?;
        let b = world
            .npc(parent_b)
            .ok_or(PopulationError::InvalidNpc(parent_b))?;
        if !a.is_active() {
            return Err(PopulationError::InactiveParent(parent_a));
        }
        if !b.is_active() {
            return Err(PopulationError::InactiveParent(parent_b));
        }
        if a.town != b.town {
            return Err(PopulationError::ParentsInDifferentTowns(parent_a, parent_b));
        }
        (
            a.town,
            a.attributes,
            b.attributes,
            a.beliefs.clone(),
            b.beliefs.clone(),
        )
    };
    let culture = world
        .town(town)
        .ok_or(PopulationError::InvalidTown(town))?
        .culture
        .clone();
    let id = next_npc_id(world.npcs.len())?;

    let variation = std::array::from_fn(|_| world.rng.random_range(-2_i8..=2));
    let attributes = Attributes::average_with_variation(&attributes_a, &attributes_b, variation);
    let beliefs = sample_child_beliefs(
        &beliefs_a,
        &beliefs_b,
        &culture,
        config.max_beliefs,
        &mut world.rng,
    );
    let mut child = Npc::with_default_name(
        id,
        0,
        sample_sex(&mut world.rng),
        town,
        town,
        attributes,
        beliefs,
        Goal::new(GoalKind::LivePeacefully, world.year),
    );
    child.add_parent(parent_a);
    child.add_parent(parent_b);
    child.set_relationship(
        parent_a,
        Relationship::with_relation(
            8,
            9,
            RelationshipKind::Family,
            world.year,
            world.month.max(1),
        ),
    );
    child.set_relationship(
        parent_b,
        Relationship::with_relation(
            8,
            9,
            RelationshipKind::Family,
            world.year,
            world.month.max(1),
        ),
    );

    world.npcs.push(child);
    world.active_npcs.push(id);
    let year = world.year;
    let month = world.month.max(1);
    for parent in [parent_a, parent_b] {
        let npc = world
            .npc_mut(parent)
            .ok_or(PopulationError::InvalidNpc(parent))?;
        npc.add_child(id);
        npc.set_relationship(
            id,
            Relationship::with_relation(8, 9, RelationshipKind::Family, year, month),
        );
    }
    world.push_event(WorldEvent::Birth { npc: id });
    Ok(id)
}

/// 外部世界から指定都市へ入る新規NPCを生成する。
pub fn generate_external_immigrant(
    world: &mut World,
    destination: TownId,
    config: &SimulationConfig,
) -> Result<NpcId, PopulationError> {
    let culture = world
        .town(destination)
        .ok_or(PopulationError::InvalidTown(destination))?
        .culture
        .clone();
    let id = next_npc_id(world.npcs.len())?;
    let age = sample_migrant_age(&mut world.rng);
    let beliefs = sample_initial_beliefs(&culture, config.max_beliefs, &mut world.rng);
    let goal_options = [
        GoalKind::GainWealth,
        GoalKind::MoveToBetterTown,
        GoalKind::LivePeacefully,
        GoalKind::BecomeSkilled,
    ];
    let goal = goal_options[world.rng.random_range(0..goal_options.len())];
    let npc = Npc::with_default_name(
        id,
        age,
        sample_sex(&mut world.rng),
        destination,
        destination,
        sample_attributes(&mut world.rng),
        beliefs,
        Goal::new(goal, world.year),
    );
    world.npcs.push(npc);
    world.active_npcs.push(id);
    world.push_event(WorldEvent::ExternalImmigration {
        npc: id,
        to: destination,
    });
    Ok(id)
}

fn validate_world_size(town_count: usize, population: usize) -> Result<(), PopulationError> {
    if town_count == 0 {
        return Err(PopulationError::NoTowns);
    }
    if u16::try_from(town_count.saturating_sub(1)).is_err() {
        return Err(PopulationError::TooManyTowns(town_count));
    }
    // capacityもu32であるため、NpcIdだけでなくこちらの上限も合わせる。
    if u32::try_from(population).is_err() {
        return Err(PopulationError::TooManyNpcs(population));
    }
    Ok(())
}

fn next_npc_id(current_len: usize) -> Result<NpcId, PopulationError> {
    u32::try_from(current_len)
        .map(NpcId)
        .map_err(|_| PopulationError::TooManyNpcs(current_len.saturating_add(1)))
}

fn balanced_town_populations(town_count: usize, population: usize) -> Vec<usize> {
    let base = population / town_count;
    let remainder = population % town_count;
    (0..town_count)
        .map(|index| base + usize::from(index < remainder))
        .collect()
}

fn generate_towns(
    populations: &[usize],
    rng: &mut ChaCha8Rng,
) -> Result<Vec<Town>, PopulationError> {
    let mut towns = Vec::with_capacity(populations.len());
    for (index, &population) in populations.iter().enumerate() {
        let id = TownId(index as u16);
        let capacity_factor_per_mille = rng.random_range(1_100_u64..=1_500);
        let population_u64 = population as u64;
        let capacity = if population == 0 {
            rng.random_range(20..=50)
        } else {
            population_u64
                .saturating_mul(capacity_factor_per_mille)
                .div_ceil(1_000)
                .clamp(1, u64::from(u32::MAX)) as u32
        };
        let mut town = Town::new(
            id,
            format!("Town-{:02}", index + 1),
            capacity,
            rng.random_range(3..=8),
            rng.random_range(3..=8),
            rng.random_range(3..=8),
            rng.random_range(3..=8),
            rng.random_range(3..=8),
        );
        town.culture = sample_initial_beliefs(&[], MAX_BELIEFS, rng);
        towns.push(town);
    }
    Ok(towns)
}

/// ランダム全域辺を作らず、木+n/3本以下の辺だけを追加する。
fn connect_towns(towns: &mut [Town], rng: &mut ChaCha8Rng) {
    if towns.len() < 2 {
        return;
    }

    let mut order = (0..towns.len()).collect::<Vec<_>>();
    order.shuffle(rng);
    for child_position in 1..order.len() {
        let parent_position = rng.random_range(0..child_position);
        add_undirected_connection(
            towns,
            order[child_position],
            order[parent_position],
            rng.random_range(1..=10),
        );
    }

    let extra_target = towns.len() / 3;
    let max_attempts = towns.len().saturating_mul(8);
    let mut extra_added = 0;
    for _ in 0..max_attempts {
        if extra_added >= extra_target {
            break;
        }
        let a = rng.random_range(0..towns.len());
        let b = rng.random_range(0..towns.len());
        if a == b || towns[a].connection_to(towns[b].id).is_some() {
            continue;
        }
        add_undirected_connection(towns, a, b, rng.random_range(1..=10));
        extra_added += 1;
    }
}

fn add_undirected_connection(towns: &mut [Town], a: usize, b: usize, distance: u8) {
    if a == b {
        return;
    }
    let a_id = towns[a].id;
    let b_id = towns[b].id;
    if a < b {
        let (left, right) = towns.split_at_mut(b);
        left[a].add_neighbor(TownConnection::new(b_id, distance));
        right[0].add_neighbor(TownConnection::new(a_id, distance));
    } else {
        let (left, right) = towns.split_at_mut(a);
        right[0].add_neighbor(TownConnection::new(b_id, distance));
        left[b].add_neighbor(TownConnection::new(a_id, distance));
    }
}

/// 一様分布ではなく、子供・若年・成人・中年・高齢者を持つ人口ピラミッド。
fn sample_age(rng: &mut ChaCha8Rng) -> u8 {
    match rng.random_range(0_u16..1_000) {
        0..=179 => rng.random_range(0..=14),
        180..=309 => rng.random_range(15..=24),
        310..=609 => rng.random_range(25..=44),
        610..=859 => rng.random_range(45..=64),
        860..=969 => rng.random_range(65..=79),
        _ => rng.random_range(80..=95),
    }
}

fn sample_migrant_age(rng: &mut ChaCha8Rng) -> u8 {
    match rng.random_range(0_u8..100) {
        0..=69 => rng.random_range(18..=39),
        70..=94 => rng.random_range(40..=59),
        _ => rng.random_range(60..=75),
    }
}

fn sample_sex(rng: &mut ChaCha8Rng) -> Sex {
    if rng.random_range(0..2) == 0 {
        Sex::Male
    } else {
        Sex::Female
    }
}

fn sample_attributes(rng: &mut ChaCha8Rng) -> Attributes {
    Attributes::new(
        rng.random_range(2..=9),
        rng.random_range(2..=9),
        rng.random_range(2..=9),
        rng.random_range(2..=9),
        rng.random_range(2..=9),
    )
}

fn sample_goal(age: u8, rng: &mut ChaCha8Rng) -> GoalKind {
    let choices: &[GoalKind] = match age {
        0..=14 => &[
            GoalKind::LivePeacefully,
            GoalKind::BecomeSkilled,
            GoalKind::SeekKnowledge,
        ],
        15..=29 => &[
            GoalKind::FindPartner,
            GoalKind::BecomeSkilled,
            GoalKind::GainWealth,
            GoalKind::SeekKnowledge,
            GoalKind::MoveToBetterTown,
        ],
        30..=54 => &[
            GoalKind::ProtectFamily,
            GoalKind::RaiseChildren,
            GoalKind::GainWealth,
            GoalKind::GainStatus,
            GoalKind::ProtectTown,
        ],
        55..=69 => &[
            GoalKind::ProtectFamily,
            GoalKind::ProtectTown,
            GoalKind::GainStatus,
            GoalKind::LivePeacefully,
        ],
        _ => &[
            GoalKind::Survive,
            GoalKind::ProtectFamily,
            GoalKind::LivePeacefully,
        ],
    };
    choices[rng.random_range(0..choices.len())]
}

fn desired_belief_count(max_beliefs: usize, rng: &mut ChaCha8Rng) -> usize {
    let maximum = max_beliefs.min(MAX_BELIEFS).min(BeliefKind::ALL.len());
    match maximum {
        0 => 0,
        1 => 1,
        _ => rng.random_range(2..=maximum),
    }
}

fn sample_initial_beliefs(
    culture: &[Belief],
    max_beliefs: usize,
    rng: &mut ChaCha8Rng,
) -> Vec<Belief> {
    let desired = desired_belief_count(max_beliefs, rng);
    let mut result = Vec::with_capacity(desired);
    let mut attempts = 0;
    while result.len() < desired && attempts < desired.saturating_mul(12).max(1) {
        attempts += 1;
        let from_culture = !culture.is_empty() && rng.random_range(0..100) < 55;
        let belief = if from_culture {
            let source = culture[rng.random_range(0..culture.len())];
            Belief::new(
                source.kind,
                vary_strength(source.strength, rng.random_range(-1..=1)),
            )
        } else {
            Belief::new(
                BeliefKind::ALL[rng.random_range(0..BeliefKind::ALL.len())],
                rng.random_range(4..=9),
            )
        };
        if !result
            .iter()
            .any(|existing: &Belief| existing.kind == belief.kind)
        {
            result.push(belief);
        }
    }
    fill_missing_beliefs(&mut result, desired, rng);
    result.sort_by_key(|belief| belief.kind);
    result
}

fn sample_child_beliefs(
    parent_a: &[Belief],
    parent_b: &[Belief],
    culture: &[Belief],
    max_beliefs: usize,
    rng: &mut ChaCha8Rng,
) -> Vec<Belief> {
    let desired = desired_belief_count(max_beliefs, rng);
    let mut result = Vec::with_capacity(desired);
    let mut attempts = 0;
    while result.len() < desired && attempts < desired.saturating_mul(16).max(1) {
        attempts += 1;
        let source = match rng.random_range(0..100) {
            0..=39 => parent_a,
            40..=79 => parent_b,
            _ => culture,
        };
        let source = if source.is_empty() {
            if !parent_a.is_empty() {
                parent_a
            } else if !parent_b.is_empty() {
                parent_b
            } else {
                culture
            }
        } else {
            source
        };
        if source.is_empty() {
            break;
        }
        let inherited = source[rng.random_range(0..source.len())];
        let belief = Belief::new(
            inherited.kind,
            vary_strength(inherited.strength, rng.random_range(-2..=2)),
        );
        if !result
            .iter()
            .any(|existing: &Belief| existing.kind == belief.kind)
        {
            result.push(belief);
        }
    }
    fill_missing_beliefs(&mut result, desired, rng);
    result.sort_by_key(|belief| belief.kind);
    result
}

fn fill_missing_beliefs(beliefs: &mut Vec<Belief>, desired: usize, rng: &mut ChaCha8Rng) {
    let mut kinds = BeliefKind::ALL;
    kinds.shuffle(rng);
    for kind in kinds {
        if beliefs.len() >= desired {
            break;
        }
        if !beliefs.iter().any(|belief| belief.kind == kind) {
            beliefs.push(Belief::new(kind, rng.random_range(4..=8)));
        }
    }
}

fn vary_strength(strength: u8, variation: i8) -> u8 {
    (i16::from(strength) + i16::from(variation)).clamp(0, 10) as u8
}

fn initialize_partners_and_families(world: &mut World, rng: &mut ChaCha8Rng) {
    let residents = world.residents_by_town();
    let mut couples_by_town = vec![Vec::<(NpcId, NpcId)>::new(); world.towns.len()];

    for (town_index, town_residents) in residents.iter().enumerate() {
        let mut male_buckets = vec![Vec::<NpcId>::new(); PARTNER_BUCKET_COUNT];
        let mut female_buckets = vec![Vec::<NpcId>::new(); PARTNER_BUCKET_COUNT];
        for &id in town_residents {
            let Some(npc) = world.npc(id) else {
                continue;
            };
            if !(18..=77).contains(&npc.age) {
                continue;
            }
            let bucket = usize::from((npc.age - 18) / 10).min(PARTNER_BUCKET_COUNT - 1);
            match npc.sex {
                Sex::Male => male_buckets[bucket].push(id),
                Sex::Female => female_buckets[bucket].push(id),
            }
        }

        for bucket in 0..PARTNER_BUCKET_COUNT {
            male_buckets[bucket].shuffle(rng);
            female_buckets[bucket].shuffle(rng);
            for (&male, &female) in male_buckets[bucket].iter().zip(&female_buckets[bucket]) {
                let affinity = rng.random_range(7..=10);
                if link_partners(world, male, female, affinity) {
                    couples_by_town[town_index].push((male.min(female), male.max(female)));
                }
            }
        }
        couples_by_town[town_index].shuffle(rng);
    }

    for (town_index, town_residents) in residents.iter().enumerate() {
        let couples = &couples_by_town[town_index];
        if couples.is_empty() {
            continue;
        }
        for &child_id in town_residents {
            let Some(child_age) = world.npc(child_id).map(|npc| npc.age) else {
                continue;
            };
            if child_age >= 18 {
                continue;
            }
            let start = rng.random_range(0..couples.len());
            let candidate_count = couples.len().min(MAX_PARENT_CANDIDATES);
            for offset in 0..candidate_count {
                let (parent_a, parent_b) = couples[(start + offset) % couples.len()];
                if parents_can_adopt_initial_child(world, parent_a, parent_b, child_age) {
                    link_parent_child(world, parent_a, child_id);
                    link_parent_child(world, parent_b, child_id);
                    break;
                }
            }
        }
    }
}

fn link_partners(world: &mut World, a: NpcId, b: NpcId, affinity: u8) -> bool {
    let year = world.year;
    let Some((left, right)) = world.two_npcs_mut(a, b) else {
        return false;
    };
    if left.partner.is_some() || right.partner.is_some() {
        return false;
    }
    left.set_partner(b);
    right.set_partner(a);
    let relationship = Relationship::with_relation(
        affinity,
        10,
        RelationshipKind::Partner,
        year,
        INITIAL_RELATIONSHIP_MONTH,
    );
    left.set_relationship(b, relationship);
    right.set_relationship(a, relationship);
    true
}

fn parents_can_adopt_initial_child(
    world: &World,
    parent_a: NpcId,
    parent_b: NpcId,
    child_age: u8,
) -> bool {
    let Some(a) = world.npc(parent_a) else {
        return false;
    };
    let Some(b) = world.npc(parent_b) else {
        return false;
    };
    let minimum_parent_age = child_age.saturating_add(18);
    let maximum_parent_age = child_age.saturating_add(50);
    (minimum_parent_age..=maximum_parent_age).contains(&a.age)
        && (minimum_parent_age..=maximum_parent_age).contains(&b.age)
        && a.children.len() < 4
        && b.children.len() < 4
}

fn link_parent_child(world: &mut World, parent: NpcId, child: NpcId) {
    let year = world.year;
    if let Some(parent_npc) = world.npc_mut(parent) {
        parent_npc.add_child(child);
        parent_npc.set_relationship(
            child,
            Relationship::with_relation(
                8,
                9,
                RelationshipKind::Family,
                year,
                INITIAL_RELATIONSHIP_MONTH,
            ),
        );
    }
    if let Some(child_npc) = world.npc_mut(child) {
        child_npc.add_parent(parent);
        child_npc.set_relationship(
            parent,
            Relationship::with_relation(
                8,
                9,
                RelationshipKind::Family,
                year,
                INITIAL_RELATIONSHIP_MONTH,
            ),
        );
    }
}

fn initialize_friendships(world: &mut World, config: &SimulationConfig, rng: &mut ChaCha8Rng) {
    let residents = world.residents_by_town();
    for mut order in residents {
        if order.len() < 2 {
            continue;
        }
        order.shuffle(rng);
        let max_offset = INITIAL_FRIEND_OFFSETS.min(order.len() - 1);
        for offset in 1..=max_offset {
            for index in 0..order.len() {
                let a = order[index];
                let b = order[(index + offset) % order.len()];
                if !can_add_friendship(world, a, b, config.max_relationships_per_npc) {
                    continue;
                }
                let affinity = rng.random_range(5..=10);
                link_symmetric_relationship(
                    world,
                    a,
                    b,
                    Relationship::with_relation(
                        affinity,
                        rng.random_range(7..=9),
                        RelationshipKind::Friend,
                        0,
                        INITIAL_RELATIONSHIP_MONTH,
                    ),
                );
            }
        }
    }
}

fn can_add_friendship(world: &World, a: NpcId, b: NpcId, limit: usize) -> bool {
    if a == b || limit == 0 {
        return false;
    }
    let Some(left) = world.npc(a) else {
        return false;
    };
    let Some(right) = world.npc(b) else {
        return false;
    };
    left.relationship(b).is_none()
        && right.relationship(a).is_none()
        && left.relationship_count() < limit
        && right.relationship_count() < limit
}

fn link_symmetric_relationship(
    world: &mut World,
    a: NpcId,
    b: NpcId,
    relationship: Relationship,
) -> bool {
    let Some((left, right)) = world.two_npcs_mut(a, b) else {
        return false;
    };
    left.set_relationship(b, relationship);
    right.set_relationship(a, relationship);
    true
}

fn align_initial_goals_with_family(world: &mut World) {
    for npc in &mut world.npcs {
        if !npc.children.is_empty() && npc.age < 60 {
            npc.goal = Goal::new(GoalKind::RaiseChildren, 0);
        } else if npc.partner.is_some() && npc.goal.kind == GoalKind::FindPartner {
            npc.goal = Goal::new(GoalKind::ProtectFamily, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn rejects_world_without_towns() {
        assert!(matches!(
            generate_initial_world(0, 100, 1, &SimulationConfig::default()),
            Err(PopulationError::NoTowns)
        ));
    }

    #[test]
    fn initial_generation_is_reproducible_and_valid() {
        let config = SimulationConfig::default();
        let mut first = generate_initial_world(8, 400, 42, &config).unwrap();
        let mut second = generate_initial_world(8, 400, 42, &config).unwrap();

        assert_eq!(first.towns, second.towns);
        assert_eq!(first.npcs, second.npcs);
        assert_eq!(first.active_npcs, second.active_npcs);
        assert!(first.validate().is_ok());
        assert_eq!(first.active_population(), 400);
        assert_eq!(first.statistics, Statistics::new(400));

        // 初期生成後にWorldへ戻したRNG位置も同一であることを確認する。
        let first_id = generate_external_immigrant(&mut first, TownId(0), &config).unwrap();
        let second_id = generate_external_immigrant(&mut second, TownId(0), &config).unwrap();
        assert_eq!(first_id, second_id);
        assert_eq!(first.npc(first_id), second.npc(second_id));
        assert_eq!(first.important_events, second.important_events);
    }

    #[test]
    fn town_graph_is_connected_and_sparse() {
        let world = generate_initial_world(20, 200, 7, &SimulationConfig::default()).unwrap();
        let mut visited = BTreeSet::from([TownId(0)]);
        let mut frontier = vec![TownId(0)];
        while let Some(id) = frontier.pop() {
            for edge in &world.town(id).unwrap().neighbors {
                if visited.insert(edge.destination) {
                    frontier.push(edge.destination);
                }
            }
        }
        let undirected_edges: usize = world
            .towns
            .iter()
            .map(|town| town.neighbors.len())
            .sum::<usize>()
            / 2;
        assert_eq!(visited.len(), world.towns.len());
        assert!(undirected_edges <= world.towns.len() - 1 + world.towns.len() / 3);
        assert!(world.towns.iter().all(|town| {
            town.neighbors.iter().all(|edge| {
                world
                    .town(edge.destination)
                    .and_then(|other| other.connection_to(town.id))
                    .is_some_and(|reverse| reverse.distance == edge.distance)
            })
        }));
    }

    #[test]
    fn sampled_ages_include_all_life_stages_with_natural_weights() {
        let mut rng = ChaCha8Rng::seed_from_u64(99);
        let mut groups = [0_usize; 5];
        for _ in 0..10_000 {
            match sample_age(&mut rng) {
                0..=14 => groups[0] += 1,
                15..=24 => groups[1] += 1,
                25..=44 => groups[2] += 1,
                45..=64 => groups[3] += 1,
                _ => groups[4] += 1,
            }
        }
        assert!(groups.iter().all(|&count| count > 500));
        assert!(groups[2] > groups[0]);
        assert!(groups[3] > groups[1]);
    }

    #[test]
    fn initial_social_graph_contains_consistent_sparse_families_and_friends() {
        let config = SimulationConfig::default();
        let world = generate_initial_world(5, 1_000, 123, &config).unwrap();
        let partnered = world
            .npcs
            .iter()
            .filter(|npc| npc.partner.is_some())
            .count();
        let children_with_parents = world
            .npcs
            .iter()
            .filter(|npc| !npc.parents.is_empty())
            .count();
        let friend_edges = world
            .npcs
            .iter()
            .flat_map(|npc| npc.relationships.values())
            .filter(|relationship| relationship.kind == RelationshipKind::Friend)
            .count();
        assert!(partnered > 0);
        assert!(children_with_parents > 0);
        assert!(friend_edges > 0);
        assert!(world.validate().is_ok());
        assert!(world.npcs.iter().all(|npc| {
            npc.relationships.iter().all(|(&other_id, relationship)| {
                world
                    .npc(other_id)
                    .and_then(|other| other.relationship(npc.id))
                    .is_some_and(|reverse| reverse == relationship)
            })
        }));
        assert!(world.npcs.iter().all(|npc| {
            npc.relationship_count()
                <= config.max_relationships_per_npc.max(
                    npc.parents.len() + npc.children.len() + usize::from(npc.partner.is_some()),
                )
        }));
    }

    #[test]
    fn child_generation_inherits_values_and_updates_both_parents() {
        let config = SimulationConfig::default();
        let mut world = generate_initial_world(1, 300, 777, &config).unwrap();
        let parent_a = world
            .npcs
            .iter()
            .find(|npc| npc.partner.is_some())
            .map(|npc| npc.id)
            .unwrap();
        let parent_b = world.npc(parent_a).unwrap().partner.unwrap();
        let child_id = generate_child(&mut world, parent_a, parent_b, &config).unwrap();
        let child = world.npc(child_id).unwrap();

        assert_eq!(child.age, 0);
        assert_eq!(child.parents, vec![parent_a, parent_b]);
        assert!(child.attributes.values_in_range());
        assert!(world.npc(parent_a).unwrap().children.contains(&child_id));
        assert!(world.npc(parent_b).unwrap().children.contains(&child_id));
        assert!(
            matches!(world.important_events.last(), Some(WorldEvent::Birth { npc }) if *npc == child_id)
        );
        assert!(world.validate().is_ok());
    }

    #[test]
    fn external_immigrant_is_active_and_logged() {
        let config = SimulationConfig::default();
        let mut world = generate_initial_world(2, 20, 8, &config).unwrap();
        let before = world.active_population();
        let id = generate_external_immigrant(&mut world, TownId(1), &config).unwrap();
        let npc = world.npc(id).unwrap();
        assert!(npc.is_active());
        assert_eq!(npc.town, TownId(1));
        assert_eq!(world.active_population(), before + 1);
        assert!(
            matches!(world.important_events.last(), Some(WorldEvent::ExternalImmigration { npc, to }) if *npc == id && *to == TownId(1))
        );
        assert!(world.validate().is_ok());
    }
}
