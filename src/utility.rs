use serde::{Deserialize, Deserializer, Serialize};

use crate::belief::BeliefKind;
use crate::goal::GoalKind;
use crate::id::{NpcId, TownId};
use crate::npc::{Npc, NpcState};
use crate::relationship::{Relationship, RelationshipKind};

const MAX_SITUATION_VALUE: u8 = 10;
const MIN_DISTANCE: u8 = 1;
const MAX_DISTANCE: u8 = 10;
const EMERGENCY_DANGER: u8 = 7;

/// Utility AIが選べる、意図的に少数へ絞った行動集合。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Work,
    Rest,
    Socialize,
    HelpPerson(NpcId),
    AvoidPerson(NpcId),
    SeekPartner,
    CareForFamily,
    MoveTown(TownId),
    FleeTown(TownId),
    DefendTown,
}

/// 人物を対象にする行動の、明示的で小さな候補情報。
///
/// 候補は全NPC総当たりではなく、同じ都市の知人などを呼び出し側で限定する。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct PersonCandidate {
    pub npc: NpcId,
    #[serde(deserialize_with = "deserialize_situation_value")]
    pub relation: u8,
    #[serde(deserialize_with = "deserialize_situation_value")]
    pub danger: u8,
    pub is_family: bool,
    pub is_outsider: bool,
    pub needs_help: bool,
    pub is_threat: bool,
    pub reachable: bool,
    pub partner_eligible: bool,
}

impl PersonCandidate {
    pub const fn new(npc: NpcId, relation: u8) -> Self {
        Self {
            npc,
            relation: clamp_situation_value(relation),
            danger: 0,
            is_family: false,
            is_outsider: false,
            needs_help: false,
            is_threat: false,
            reachable: true,
            partner_eligible: false,
        }
    }

    pub const fn from_relationship(npc: NpcId, relationship: &Relationship) -> Self {
        Self {
            npc,
            relation: clamp_situation_value(relationship.relation),
            danger: 0,
            is_family: matches!(
                relationship.kind,
                RelationshipKind::Family | RelationshipKind::Partner
            ),
            is_outsider: false,
            needs_help: false,
            is_threat: matches!(
                relationship.kind,
                RelationshipKind::Rival | RelationshipKind::Enemy
            ),
            reachable: true,
            partner_eligible: false,
        }
    }

    pub const fn needing_help(mut self, danger: u8) -> Self {
        self.needs_help = true;
        self.danger = clamp_situation_value(danger);
        self
    }

    pub const fn threatening(mut self, danger: u8) -> Self {
        self.is_threat = true;
        self.danger = clamp_situation_value(danger);
        self
    }

    pub const fn as_family(mut self) -> Self {
        self.is_family = true;
        self
    }

    pub const fn as_outsider(mut self) -> Self {
        self.is_outsider = true;
        self
    }

    pub const fn eligible_partner(mut self) -> Self {
        self.partner_eligible = true;
        self
    }

    pub fn normalize(&mut self) {
        self.relation = clamp_situation_value(self.relation);
        self.danger = clamp_situation_value(self.danger);
    }

    pub const fn values_in_range(&self) -> bool {
        self.relation <= MAX_SITUATION_VALUE && self.danger <= MAX_SITUATION_VALUE
    }
}

impl Default for PersonCandidate {
    fn default() -> Self {
        Self::new(NpcId::default(), 5)
    }
}

/// 移住・避難の候補都市。呼び出し側で隣接都市や2-hop程度に限定する。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct MoveCandidate {
    pub town: TownId,
    #[serde(deserialize_with = "deserialize_attractiveness")]
    pub attractiveness: f32,
    #[serde(deserialize_with = "deserialize_distance")]
    pub distance: u8,
    pub known_people: u16,
    #[serde(deserialize_with = "deserialize_compatibility")]
    pub belief_compatibility: f32,
    #[serde(deserialize_with = "deserialize_nonnegative")]
    pub family_separation_penalty: f32,
    #[serde(deserialize_with = "deserialize_nonnegative")]
    pub moving_cost: f32,
    pub safe_for_fleeing: bool,
}

impl MoveCandidate {
    pub fn new(town: TownId, attractiveness: f32, distance: u8) -> Self {
        Self {
            town,
            attractiveness: clamp_attractiveness(attractiveness),
            distance: clamp_distance(distance),
            known_people: 0,
            belief_compatibility: 0.0,
            family_separation_penalty: 0.0,
            moving_cost: 0.0,
            safe_for_fleeing: false,
        }
    }

    pub const fn with_known_people(mut self, known_people: u16) -> Self {
        self.known_people = known_people;
        self
    }

    pub fn with_belief_compatibility(mut self, compatibility: f32) -> Self {
        self.belief_compatibility = clamp_compatibility(compatibility);
        self
    }

    pub fn with_costs(mut self, family_separation_penalty: f32, moving_cost: f32) -> Self {
        self.family_separation_penalty = clamp_nonnegative(family_separation_penalty);
        self.moving_cost = clamp_nonnegative(moving_cost);
        self
    }

    pub const fn safe_for_fleeing(mut self, safe: bool) -> Self {
        self.safe_for_fleeing = safe;
        self
    }

    pub fn normalize(&mut self) {
        self.attractiveness = clamp_attractiveness(self.attractiveness);
        self.distance = clamp_distance(self.distance);
        self.belief_compatibility = clamp_compatibility(self.belief_compatibility);
        self.family_separation_penalty = clamp_nonnegative(self.family_separation_penalty);
        self.moving_cost = clamp_nonnegative(self.moving_cost);
    }

    pub fn values_in_range(&self) -> bool {
        self.attractiveness.is_finite()
            && (0.0..=10.0).contains(&self.attractiveness)
            && (MIN_DISTANCE..=MAX_DISTANCE).contains(&self.distance)
            && self.belief_compatibility.is_finite()
            && (-10.0..=10.0).contains(&self.belief_compatibility)
            && self.family_separation_penalty.is_finite()
            && self.family_separation_penalty >= 0.0
            && self.moving_cost.is_finite()
            && self.moving_cost >= 0.0
    }
}

impl Default for MoveCandidate {
    fn default() -> Self {
        Self::new(TownId::default(), 5.0, 1)
    }
}

/// 1回の意思決定へ渡す、世界全体から切り離した状況スナップショット。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Situation {
    #[serde(deserialize_with = "deserialize_situation_value")]
    pub danger: u8,
    #[serde(deserialize_with = "deserialize_situation_value")]
    pub tiredness: u8,
    pub at_war: bool,
    pub natural_disaster: bool,
    pub disease_outbreak: bool,
    pub has_work: bool,
    pub socializing_allowed: bool,
    pub family_needs_care: bool,
    pub can_move: bool,
    pub can_defend_town: bool,
    pub people: Vec<PersonCandidate>,
    pub move_candidates: Vec<MoveCandidate>,
}

impl Situation {
    pub const fn is_emergency(&self) -> bool {
        self.danger >= EMERGENCY_DANGER || self.at_war || self.natural_disaster
    }

    pub fn normalize(&mut self) {
        self.danger = clamp_situation_value(self.danger);
        self.tiredness = clamp_situation_value(self.tiredness);
        for person in &mut self.people {
            person.normalize();
        }
        for destination in &mut self.move_candidates {
            destination.normalize();
        }
    }

    pub fn values_in_range(&self) -> bool {
        self.danger <= MAX_SITUATION_VALUE
            && self.tiredness <= MAX_SITUATION_VALUE
            && self.people.iter().all(PersonCandidate::values_in_range)
            && self
                .move_candidates
                .iter()
                .all(MoveCandidate::values_in_range)
    }

    pub fn available_actions(&self, npc: &Npc) -> Vec<Action> {
        available_actions(npc, self)
    }

    pub fn choose_action(&self, npc: &Npc) -> Option<Action> {
        choose_action(npc, self)
    }
}

impl Default for Situation {
    fn default() -> Self {
        Self {
            danger: 0,
            tiredness: 0,
            at_war: false,
            natural_disaster: false,
            disease_outbreak: false,
            has_work: true,
            socializing_allowed: true,
            family_needs_care: false,
            can_move: true,
            can_defend_town: true,
            people: Vec::new(),
            move_candidates: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ScoredAction {
    pub action: Action,
    pub score: f32,
}

/// NPCの既存関係をID昇順の候補へ変換する。
/// `HashMap`のランダムな走査順をUtilityの同点決着へ持ち込まないための補助。
pub fn sorted_relationship_candidates(npc: &Npc) -> Vec<PersonCandidate> {
    let mut relationships = npc.relationships.iter().collect::<Vec<_>>();
    relationships.sort_unstable_by_key(|(id, _)| **id);
    relationships
        .into_iter()
        .map(|(id, relationship)| PersonCandidate::from_relationship(*id, relationship))
        .collect()
}

/// 先に実行可能性だけを評価し、固定カテゴリ順・候補入力順で行動を返す。
pub fn available_actions(npc: &Npc, situation: &Situation) -> Vec<Action> {
    if !npc.is_active() {
        return Vec::new();
    }

    let emergency = situation.is_emergency();
    let mut actions =
        Vec::with_capacity(5 + situation.people.len() * 2 + situation.move_candidates.len() * 2);

    if situation.has_work && !emergency {
        actions.push(Action::Work);
    }
    actions.push(Action::Rest);
    if situation.socializing_allowed && !emergency && !situation.disease_outbreak {
        actions.push(Action::Socialize);
    }

    let mut help_targets = Vec::new();
    for person in &situation.people {
        if person.npc != npc.id
            && person.reachable
            && person.needs_help
            && !help_targets.contains(&person.npc)
        {
            help_targets.push(person.npc);
            actions.push(Action::HelpPerson(person.npc));
        }
    }

    let mut avoid_targets = Vec::new();
    for person in &situation.people {
        if person.npc != npc.id
            && person.reachable
            && (person.is_threat || clamp_situation_value(person.relation) <= 4)
            && !avoid_targets.contains(&person.npc)
        {
            avoid_targets.push(person.npc);
            actions.push(Action::AvoidPerson(person.npc));
        }
    }

    let has_partner_candidate = situation
        .people
        .iter()
        .any(|person| person.npc != npc.id && person.reachable && person.partner_eligible);
    if npc.is_adult()
        && npc.partner.is_none()
        && has_partner_candidate
        && !emergency
        && !situation.disease_outbreak
    {
        actions.push(Action::SeekPartner);
    }
    if situation.family_needs_care {
        actions.push(Action::CareForFamily);
    }

    if situation.can_move && !emergency {
        let mut destinations = Vec::new();
        for destination in &situation.move_candidates {
            if destination.town != npc.town && !destinations.contains(&destination.town) {
                destinations.push(destination.town);
                actions.push(Action::MoveTown(destination.town));
            }
        }
    }

    if situation.can_move && emergency {
        let mut destinations = Vec::new();
        for destination in &situation.move_candidates {
            if destination.town != npc.town
                && destination.safe_for_fleeing
                && !destinations.contains(&destination.town)
            {
                destinations.push(destination.town);
                actions.push(Action::FleeTown(destination.town));
            }
        }
    }

    if situation.at_war && situation.can_defend_town && npc.is_adult() {
        actions.push(Action::DefendTown);
    }
    actions
}

pub fn is_action_available(npc: &Npc, situation: &Situation, action: Action) -> bool {
    available_actions(npc, situation).contains(&action)
}

/// 実行可能な行動だけを採点する。実行不能なら`None`。
pub fn score_action(npc: &Npc, situation: &Situation, action: Action) -> Option<f32> {
    is_action_available(npc, situation, action)
        .then(|| score_available_action(npc, situation, action))
}

/// 実行可能行動へ絞った後で採点し、最高点を選ぶ。
/// 同点なら`available_actions`で先に現れた候補を必ず選ぶ。
pub fn choose_scored_action(npc: &Npc, situation: &Situation) -> Option<ScoredAction> {
    let actions = available_actions(npc, situation);
    let mut best: Option<ScoredAction> = None;
    for action in actions {
        let candidate = ScoredAction {
            action,
            score: score_available_action(npc, situation, action),
        };
        if best
            .as_ref()
            .is_none_or(|current| candidate.score > current.score)
        {
            best = Some(candidate);
        }
    }
    best
}

pub fn choose_action(npc: &Npc, situation: &Situation) -> Option<Action> {
    choose_scored_action(npc, situation).map(|choice| choice.action)
}

/// `choose_action`の意味を明示した別名。
pub fn select_action(npc: &Npc, situation: &Situation) -> Option<Action> {
    choose_action(npc, situation)
}

fn score_available_action(npc: &Npc, situation: &Situation, action: Action) -> f32 {
    let danger = f32::from(clamp_situation_value(situation.danger));
    let tiredness = f32::from(clamp_situation_value(situation.tiredness));
    match action {
        Action::Work => {
            3.5 + f32::from(npc.attributes.physical) * 0.10
                + f32::from(npc.attributes.dexterity) * 0.15
                + f32::from(npc.attributes.intelligence) * 0.15
                + goal_bonus(npc.goal.kind, &[GoalKind::GainWealth], 2.5)
                + goal_bonus(npc.goal.kind, &[GoalKind::BecomeSkilled], 2.0)
                + goal_bonus(npc.goal.kind, &[GoalKind::GainStatus], 1.5)
                + belief_score(npc, BeliefKind::ValueWealth, 0.25)
                - danger * 0.4
                - tiredness * 0.25
                - if situation.disease_outbreak { 1.5 } else { 0.0 }
        }
        Action::Rest => {
            2.0 + tiredness * 0.75
                + if situation.is_emergency() { 1.0 } else { 0.0 }
                + if npc.state == NpcState::Sick {
                    3.0
                } else {
                    0.0
                }
        }
        Action::Socialize => {
            let relation_bonus = average_reachable_relation(situation) * 0.15;
            3.5 + f32::from(npc.attributes.charisma) * 0.3
                + relation_bonus
                + goal_bonus(npc.goal.kind, &[GoalKind::FindPartner], 1.5)
                + goal_bonus(npc.goal.kind, &[GoalKind::GainStatus], 1.5)
                + belief_score(npc, BeliefKind::HelpOthers, 0.15)
                - danger * 0.4
        }
        Action::HelpPerson(target) => {
            let person = first_person(situation, target);
            let relation = person.map_or(0.0, |person| {
                f32::from(clamp_situation_value(person.relation))
            });
            let help_danger = person.map_or(danger, |person| {
                f32::from(clamp_situation_value(person.danger)).max(danger)
            });
            let is_family = person.is_some_and(|person| {
                person.is_family
                    || npc.partner == Some(target)
                    || npc.parents.contains(&target)
                    || npc.children.contains(&target)
            });
            1.0 + relation * 0.6
                + belief_score(npc, BeliefKind::HelpOthers, 0.4)
                + if is_family {
                    belief_score(npc, BeliefKind::ProtectFamily, 0.5)
                        + goal_bonus(
                            npc.goal.kind,
                            &[GoalKind::ProtectFamily, GoalKind::RaiseChildren],
                            2.5,
                        )
                } else {
                    0.0
                }
                - help_danger * 0.5
        }
        Action::AvoidPerson(target) => {
            let person = first_person(situation, target);
            let relation = person.map_or(5.0, |person| {
                f32::from(clamp_situation_value(person.relation))
            });
            let person_danger = person.map_or(0.0, |person| {
                f32::from(clamp_situation_value(person.danger))
            });
            let outsider_bonus = if person.is_some_and(|person| person.is_outsider) {
                belief_score(npc, BeliefKind::DistrustOutsiders, 0.4)
            } else {
                0.0
            };
            1.0 + (10.0 - relation) * 0.45 + person_danger * 0.65 + outsider_bonus
        }
        Action::SeekPartner => {
            let best_relation = situation
                .people
                .iter()
                .filter(|person| person.reachable && person.partner_eligible)
                .map(|person| clamp_situation_value(person.relation))
                .max()
                .map_or(0.0, f32::from);
            5.0 + f32::from(npc.attributes.charisma) * 0.35
                + best_relation * 0.25
                + goal_bonus(npc.goal.kind, &[GoalKind::FindPartner], 5.0)
                + goal_bonus(npc.goal.kind, &[GoalKind::LivePeacefully], 0.8)
                - danger * 0.4
        }
        Action::CareForFamily => {
            3.0 + belief_score(npc, BeliefKind::ProtectFamily, 0.55)
                + goal_bonus(
                    npc.goal.kind,
                    &[
                        GoalKind::ProtectFamily,
                        GoalKind::RaiseChildren,
                        GoalKind::LivePeacefully,
                    ],
                    2.5,
                )
                - danger * 0.25
        }
        Action::MoveTown(town) => {
            let destination = first_destination(situation, town);
            destination.map_or(f32::NEG_INFINITY, |destination| {
                let attractiveness = clamp_attractiveness(destination.attractiveness);
                let distance = f32::from(clamp_distance(destination.distance));
                let known_people_bonus = f32::from(destination.known_people.min(10)) * 0.25;
                attractiveness
                    + known_people_bonus
                    + clamp_compatibility(destination.belief_compatibility)
                    + goal_bonus(npc.goal.kind, &[GoalKind::MoveToBetterTown], 3.0)
                    + belief_score(npc, BeliefKind::ValueFreedom, 0.1)
                    - distance * 0.35
                    - clamp_nonnegative(destination.family_separation_penalty)
                    - clamp_nonnegative(destination.moving_cost)
            })
        }
        Action::FleeTown(town) => {
            let destination = first_destination(situation, town);
            destination.map_or(f32::NEG_INFINITY, |destination| {
                2.0 + danger * 0.85
                    + clamp_attractiveness(destination.attractiveness) * 0.35
                    + f32::from(npc.attributes.willpower) * 0.1
                    - f32::from(clamp_distance(destination.distance)) * 0.2
                    - clamp_nonnegative(destination.moving_cost) * 0.4
            })
        }
        Action::DefendTown => {
            2.0 + f32::from(npc.attributes.physical) * 0.3
                + f32::from(npc.attributes.willpower) * 0.3
                + belief_score(npc, BeliefKind::ProtectHometown, 0.55)
                + goal_bonus(npc.goal.kind, &[GoalKind::ProtectTown], 3.0)
                + danger * 0.15
        }
    }
}

fn first_person(situation: &Situation, target: NpcId) -> Option<&PersonCandidate> {
    situation.people.iter().find(|person| person.npc == target)
}

fn first_destination(situation: &Situation, town: TownId) -> Option<&MoveCandidate> {
    situation
        .move_candidates
        .iter()
        .find(|destination| destination.town == town)
}

fn average_reachable_relation(situation: &Situation) -> f32 {
    let mut total = 0_u32;
    let mut count = 0_u32;
    for person in &situation.people {
        if person.reachable {
            total += u32::from(clamp_situation_value(person.relation));
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        total as f32 / count as f32
    }
}

fn belief_score(npc: &Npc, kind: BeliefKind, factor: f32) -> f32 {
    f32::from(npc.belief_strength(kind)) * factor
}

fn goal_bonus(current: GoalKind, matching: &[GoalKind], bonus: f32) -> f32 {
    if matching.contains(&current) {
        bonus
    } else {
        0.0
    }
}

const fn clamp_situation_value(value: u8) -> u8 {
    if value > MAX_SITUATION_VALUE {
        MAX_SITUATION_VALUE
    } else {
        value
    }
}

const fn clamp_distance(distance: u8) -> u8 {
    if distance < MIN_DISTANCE {
        MIN_DISTANCE
    } else if distance > MAX_DISTANCE {
        MAX_DISTANCE
    } else {
        distance
    }
}

fn clamp_attractiveness(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 10.0)
    }
}

fn clamp_compatibility(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-10.0, 10.0)
    } else {
        0.0
    }
}

fn clamp_nonnegative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn deserialize_situation_value<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(clamp_situation_value(u8::deserialize(deserializer)?))
}

fn deserialize_distance<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(clamp_distance(u8::deserialize(deserializer)?))
}

fn deserialize_attractiveness<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(clamp_attractiveness(f32::deserialize(deserializer)?))
}

fn deserialize_compatibility<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(clamp_compatibility(f32::deserialize(deserializer)?))
}

fn deserialize_nonnegative<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(clamp_nonnegative(f32::deserialize(deserializer)?))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::belief::Belief;
    use crate::goal::Goal;
    use crate::npc::{Attributes, Sex};

    use super::*;

    fn npc_with(beliefs: Vec<Belief>, goal: GoalKind) -> Npc {
        Npc::with_default_name(
            NpcId(1),
            30,
            Sex::Female,
            TownId(1),
            TownId(1),
            Attributes::uniform(5),
            beliefs,
            Goal::new(goal, 0),
        )
    }

    #[test]
    fn family_belief_and_strong_relation_select_help() {
        let npc = npc_with(
            vec![Belief::new(BeliefKind::ProtectFamily, 9)],
            GoalKind::ProtectFamily,
        );
        let situation = Situation {
            danger: 3,
            people: vec![
                PersonCandidate::new(NpcId(2), 9)
                    .as_family()
                    .needing_help(3),
            ],
            ..Situation::default()
        };
        assert_eq!(
            choose_action(&npc, &situation),
            Some(Action::HelpPerson(NpcId(2)))
        );
    }

    #[test]
    fn weak_belief_low_relation_and_extreme_danger_do_not_select_help() {
        let npc = npc_with(
            vec![Belief::new(BeliefKind::HelpOthers, 1)],
            GoalKind::GainWealth,
        );
        let situation = Situation {
            danger: 10,
            people: vec![PersonCandidate::new(NpcId(2), 2).needing_help(10)],
            ..Situation::default()
        };
        assert_ne!(
            choose_action(&npc, &situation),
            Some(Action::HelpPerson(NpcId(2)))
        );
    }

    #[test]
    fn impossible_actions_are_filtered_before_scoring() {
        let npc = npc_with(vec![], GoalKind::BecomeSkilled);
        let peaceful = Situation::default();
        assert!(available_actions(&npc, &peaceful).contains(&Action::Work));

        let war = Situation {
            at_war: true,
            danger: 6,
            ..Situation::default()
        };
        assert!(!available_actions(&npc, &war).contains(&Action::Work));
        assert_eq!(score_action(&npc, &war, Action::Work), None);
        assert!(available_actions(&npc, &war).contains(&Action::DefendTown));
    }

    #[test]
    fn ties_use_first_candidate_order() {
        let npc = npc_with(
            vec![Belief::new(BeliefKind::HelpOthers, 10)],
            GoalKind::Survive,
        );
        let situation = Situation {
            people: vec![
                PersonCandidate::new(NpcId(20), 10).needing_help(0),
                PersonCandidate::new(NpcId(10), 10).needing_help(0),
            ],
            ..Situation::default()
        };
        assert_eq!(
            choose_action(&npc, &situation),
            Some(Action::HelpPerson(NpcId(20)))
        );
    }

    #[test]
    fn duplicate_candidates_do_not_duplicate_actions() {
        let npc = npc_with(vec![], GoalKind::Survive);
        let situation = Situation {
            people: vec![
                PersonCandidate::new(NpcId(2), 2).needing_help(1),
                PersonCandidate::new(NpcId(2), 2).needing_help(1),
            ],
            move_candidates: vec![
                MoveCandidate::new(TownId(2), 8.0, 2),
                MoveCandidate::new(TownId(2), 8.0, 2),
            ],
            ..Situation::default()
        };
        let actions = available_actions(&npc, &situation);
        assert_eq!(
            actions
                .iter()
                .filter(|action| **action == Action::HelpPerson(NpcId(2)))
                .count(),
            1
        );
        assert_eq!(
            actions
                .iter()
                .filter(|action| **action == Action::MoveTown(TownId(2)))
                .count(),
            1
        );
    }

    #[test]
    fn emergency_exposes_flee_not_regular_move() {
        let npc = npc_with(vec![], GoalKind::Survive);
        let situation = Situation {
            danger: 9,
            move_candidates: vec![
                MoveCandidate::new(TownId(2), 8.0, 2).safe_for_fleeing(true),
                MoveCandidate::new(TownId(3), 10.0, 1).safe_for_fleeing(false),
            ],
            ..Situation::default()
        };
        let actions = available_actions(&npc, &situation);
        assert!(actions.contains(&Action::FleeTown(TownId(2))));
        assert!(!actions.contains(&Action::FleeTown(TownId(3))));
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, Action::MoveTown(_)))
        );
    }

    #[test]
    fn dead_or_external_npc_has_no_available_action() {
        let mut npc = npc_with(vec![], GoalKind::Survive);
        npc.leave_world();
        assert!(available_actions(&npc, &Situation::default()).is_empty());
        assert_eq!(choose_action(&npc, &Situation::default()), None);
    }

    #[test]
    fn relationship_conversion_sorts_ids() {
        let mut npc = npc_with(vec![], GoalKind::Survive);
        npc.relationships = HashMap::from([
            (NpcId(9), Relationship::default()),
            (NpcId(2), Relationship::default()),
            (NpcId(7), Relationship::default()),
        ]);
        assert_eq!(
            sorted_relationship_candidates(&npc)
                .iter()
                .map(|person| person.npc)
                .collect::<Vec<_>>(),
            vec![NpcId(2), NpcId(7), NpcId(9)]
        );
    }

    #[test]
    fn situation_json_clamps_bounded_values() {
        let situation: Situation = serde_json::from_str(
            r#"{
                "danger":200,
                "tiredness":100,
                "people":[{"npc":2,"relation":99,"danger":98}],
                "move_candidates":[{"town":2,"attractiveness":99.0,"distance":0}]
            }"#,
        )
        .unwrap();
        assert_eq!(situation.danger, 10);
        assert_eq!(situation.people[0].relation, 10);
        assert_eq!(situation.move_candidates[0].attractiveness, 10.0);
        assert_eq!(situation.move_candidates[0].distance, 1);
        assert!(situation.values_in_range());
    }
}
