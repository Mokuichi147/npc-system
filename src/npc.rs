use std::collections::{HashMap, HashSet, hash_map::Entry};

use serde::{Deserialize, Deserializer, Serialize};

use crate::belief::{Belief, BeliefKind, MAX_BELIEFS};
use crate::goal::{Goal, GoalKind};
use crate::id::{NpcId, TownId};
use crate::relationship::{Relationship, RelationshipKind};

pub const MIN_ATTRIBUTE: u8 = 0;
pub const MAX_ATTRIBUTE: u8 = 10;
pub const ADULT_AGE: u8 = 18;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Sex {
    #[default]
    Male,
    Female,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NpcState {
    #[default]
    Normal,
    Sick,
    Evacuating,
}

/// NPCの基礎能力。各値は`0..=10`。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Attributes {
    #[serde(deserialize_with = "deserialize_attribute")]
    pub physical: u8,
    #[serde(deserialize_with = "deserialize_attribute")]
    pub dexterity: u8,
    #[serde(deserialize_with = "deserialize_attribute")]
    pub intelligence: u8,
    #[serde(deserialize_with = "deserialize_attribute")]
    pub charisma: u8,
    #[serde(deserialize_with = "deserialize_attribute")]
    pub willpower: u8,
}

impl Attributes {
    pub const fn new(
        physical: u8,
        dexterity: u8,
        intelligence: u8,
        charisma: u8,
        willpower: u8,
    ) -> Self {
        Self {
            physical: clamp_attribute(physical),
            dexterity: clamp_attribute(dexterity),
            intelligence: clamp_attribute(intelligence),
            charisma: clamp_attribute(charisma),
            willpower: clamp_attribute(willpower),
        }
    }

    pub const fn uniform(value: u8) -> Self {
        let value = clamp_attribute(value);
        Self::new(value, value, value, value, value)
    }

    /// 親2人の平均へ能力ごとの変動を加え、範囲内の子供能力を作る。
    ///
    /// 通常は各変動に`-2..=2`を渡す。防御的に、それを超える入力でも
    /// 最終結果は必ず`0..=10`へ丸める。
    pub fn average_with_variation(parent_a: &Self, parent_b: &Self, variation: [i8; 5]) -> Self {
        Self {
            physical: average_attribute(parent_a.physical, parent_b.physical, variation[0]),
            dexterity: average_attribute(parent_a.dexterity, parent_b.dexterity, variation[1]),
            intelligence: average_attribute(
                parent_a.intelligence,
                parent_b.intelligence,
                variation[2],
            ),
            charisma: average_attribute(parent_a.charisma, parent_b.charisma, variation[3]),
            willpower: average_attribute(parent_a.willpower, parent_b.willpower, variation[4]),
        }
    }

    pub const fn values_in_range(&self) -> bool {
        self.physical <= MAX_ATTRIBUTE
            && self.dexterity <= MAX_ATTRIBUTE
            && self.intelligence <= MAX_ATTRIBUTE
            && self.charisma <= MAX_ATTRIBUTE
            && self.willpower <= MAX_ATTRIBUTE
    }

    pub fn normalize(&mut self) {
        self.physical = clamp_attribute(self.physical);
        self.dexterity = clamp_attribute(self.dexterity);
        self.intelligence = clamp_attribute(self.intelligence);
        self.charisma = clamp_attribute(self.charisma);
        self.willpower = clamp_attribute(self.willpower);
    }
}

impl Default for Attributes {
    fn default() -> Self {
        Self::uniform(5)
    }
}

/// 単独のNPCだけで検査できる不変条件の違反。
///
/// partner相互参照や親子相互参照、都市IDの実在はWorld側で検査する。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum NpcInvariantError {
    AttributeOutOfRange,
    BeliefOutOfRange { kind: BeliefKind },
    TooManyBeliefs { count: usize },
    DuplicateBelief { kind: BeliefKind },
    GoalProgressOutOfRange,
    SelfRelationship,
    RelationshipValueOutOfRange { other: NpcId },
    SelfParent,
    DuplicateParent { parent: NpcId },
    SelfChild,
    DuplicateChild { child: NpcId },
    SelfPartner,
    DeadNpcMarkedInWorld,
}

/// シミュレーションに保持するNPC本体。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Npc {
    pub id: NpcId,
    pub name: String,
    pub age: u8,
    pub sex: Sex,
    pub hometown: TownId,
    pub town: TownId,
    pub attributes: Attributes,
    pub beliefs: Vec<Belief>,
    pub goal: Goal,
    pub relationships: HashMap<NpcId, Relationship>,
    pub parents: Vec<NpcId>,
    pub children: Vec<NpcId>,
    pub partner: Option<NpcId>,
    pub alive: bool,
    /// World内に現在存在するか。外部転出者は生存したまま`false`になる。
    #[serde(default = "default_true")]
    pub in_world: bool,
    #[serde(default)]
    pub state: NpcState,
}

impl Npc {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: NpcId,
        name: impl Into<String>,
        age: u8,
        sex: Sex,
        hometown: TownId,
        town: TownId,
        attributes: Attributes,
        beliefs: Vec<Belief>,
        goal: Goal,
    ) -> Self {
        let mut npc = Self {
            id,
            name: name.into(),
            age,
            sex,
            hometown,
            town,
            attributes,
            beliefs,
            goal,
            relationships: HashMap::new(),
            parents: Vec::new(),
            children: Vec::new(),
            partner: None,
            alive: true,
            in_world: true,
            state: NpcState::Normal,
        };
        npc.normalize();
        npc
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_default_name(
        id: NpcId,
        age: u8,
        sex: Sex,
        hometown: TownId,
        town: TownId,
        attributes: Attributes,
        beliefs: Vec<Belief>,
        goal: Goal,
    ) -> Self {
        Self::new(
            id,
            Self::default_name(id),
            age,
            sex,
            hometown,
            town,
            attributes,
            beliefs,
            goal,
        )
    }

    pub fn default_name(id: NpcId) -> String {
        format!("NPC-{:06}", id.0)
    }

    pub const fn is_adult(&self) -> bool {
        self.age >= ADULT_AGE
    }

    pub const fn is_active(&self) -> bool {
        self.alive && self.in_world
    }

    pub fn age_one_year(&mut self) {
        self.age = self.age.saturating_add(1);
    }

    pub fn move_to(&mut self, town: TownId) -> bool {
        if !self.is_active() || self.town == town {
            return false;
        }
        self.town = town;
        true
    }

    /// 死亡状態にし、解除すべき以前のpartner IDを返す。
    pub fn mark_dead(&mut self) -> Option<NpcId> {
        self.alive = false;
        self.in_world = false;
        self.state = NpcState::Normal;
        self.partner.take()
    }

    /// NPCを生存したまま外部世界へ移す。
    pub fn leave_world(&mut self) -> bool {
        if !self.alive || !self.in_world {
            return false;
        }
        self.in_world = false;
        true
    }

    /// 外部世界にいる生存NPCを指定都市へ戻す。
    pub fn enter_world(&mut self, town: TownId) -> bool {
        if !self.alive || self.in_world {
            return false;
        }
        self.town = town;
        self.in_world = true;
        true
    }

    pub fn belief(&self, kind: BeliefKind) -> Option<&Belief> {
        self.beliefs.iter().find(|belief| belief.kind == kind)
    }

    pub fn belief_mut(&mut self, kind: BeliefKind) -> Option<&mut Belief> {
        self.beliefs.iter_mut().find(|belief| belief.kind == kind)
    }

    pub fn belief_strength(&self, kind: BeliefKind) -> u8 {
        self.belief(kind).map_or(0, |belief| belief.strength)
    }

    /// 同種の信念なら置換し、新種なら最大3件まで追加する。
    pub fn add_belief(&mut self, mut belief: Belief) -> bool {
        belief.normalize();
        if let Some(existing) = self.belief_mut(belief.kind) {
            if *existing == belief {
                return false;
            }
            *existing = belief;
            return true;
        }
        if self.beliefs.len() >= MAX_BELIEFS {
            return false;
        }
        self.beliefs.push(belief);
        true
    }

    pub fn adjust_belief(&mut self, kind: BeliefKind, change: i8) -> Option<i8> {
        self.belief_mut(kind).map(|belief| belief.adjust(change))
    }

    pub fn replace_beliefs(&mut self, beliefs: Vec<Belief>) {
        self.beliefs = beliefs;
        normalize_beliefs(&mut self.beliefs);
    }

    pub fn change_goal(
        &mut self,
        new_kind: GoalKind,
        current_year: u64,
        cooldown_years: u16,
        major_event: bool,
    ) -> bool {
        self.goal
            .change_to(new_kind, current_year, cooldown_years, major_event)
    }

    /// 自分自身への関係は拒否する。戻り値は挿入・更新できたか。
    pub fn set_relationship(&mut self, other: NpcId, mut relationship: Relationship) -> bool {
        if other == self.id {
            return false;
        }
        relationship.normalize();
        self.relationships.insert(other, relationship);
        true
    }

    /// 関係がなければ中立の知人関係を作り、その可変参照を返す。
    pub fn ensure_relationship(
        &mut self,
        other: NpcId,
        affinity: u8,
        year: u64,
        month: u8,
    ) -> Option<&mut Relationship> {
        if other == self.id {
            return None;
        }

        Some(match self.relationships.entry(other) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(Relationship::new(
                affinity,
                RelationshipKind::Acquaintance,
                year,
                month,
            )),
        })
    }

    pub fn relationship(&self, other: NpcId) -> Option<&Relationship> {
        self.relationships.get(&other)
    }

    pub fn relationship_mut(&mut self, other: NpcId) -> Option<&mut Relationship> {
        if other == self.id {
            return None;
        }
        self.relationships.get_mut(&other)
    }

    pub fn remove_relationship(&mut self, other: NpcId) -> Option<Relationship> {
        self.relationships.remove(&other)
    }

    pub fn relationship_count(&self) -> usize {
        self.relationships.len()
    }

    pub fn strong_relationship_count(&self) -> usize {
        self.relationships
            .values()
            .filter(|relationship| relationship.is_strong())
            .count()
    }

    /// 年次tickで忘却候補を削除する。戻り値は常にID昇順で再現可能。
    pub fn forget_stale_relationships(&mut self, current_year: u64) -> Vec<NpcId> {
        let mut forgotten = self
            .relationships
            .iter()
            .filter_map(|(id, relationship)| {
                relationship.should_forget(current_year).then_some(*id)
            })
            .collect::<Vec<_>>();
        forgotten.sort_unstable();
        for id in &forgotten {
            self.relationships.remove(id);
        }
        forgotten
    }

    /// 月単位の厳密な忘却版。戻り値は常にID昇順。
    pub fn forget_stale_relationships_at(
        &mut self,
        current_year: u64,
        current_month: u8,
    ) -> Vec<NpcId> {
        let mut forgotten = self
            .relationships
            .iter()
            .filter_map(|(id, relationship)| {
                relationship
                    .should_forget_at(current_year, current_month)
                    .then_some(*id)
            })
            .collect::<Vec<_>>();
        forgotten.sort_unstable();
        for id in &forgotten {
            self.relationships.remove(id);
        }
        forgotten
    }

    pub fn add_parent(&mut self, parent: NpcId) -> bool {
        add_unique_relative(self.id, &mut self.parents, parent)
    }

    pub fn remove_parent(&mut self, parent: NpcId) -> bool {
        remove_relative(&mut self.parents, parent)
    }

    pub fn add_child(&mut self, child: NpcId) -> bool {
        add_unique_relative(self.id, &mut self.children, child)
    }

    pub fn remove_child(&mut self, child: NpcId) -> bool {
        remove_relative(&mut self.children, child)
    }

    /// 有効な別NPCならpartnerを設定する。戻り値は値が変わったか。
    pub fn set_partner(&mut self, partner: NpcId) -> bool {
        if partner == self.id || self.partner == Some(partner) {
            return false;
        }
        self.partner = Some(partner);
        true
    }

    pub fn clear_partner(&mut self) -> Option<NpcId> {
        self.partner.take()
    }

    pub fn clear_partner_if(&mut self, expected: NpcId) -> bool {
        if self.partner == Some(expected) {
            self.partner = None;
            true
        } else {
            false
        }
    }

    /// 0〜10系の値と正規化進捗がすべて範囲内かを高速に確認する。
    pub fn values_in_range(&self) -> bool {
        self.attributes.values_in_range()
            && self.beliefs.iter().all(Belief::is_valid)
            && self.goal.is_valid()
            && self
                .relationships
                .values()
                .all(Relationship::values_in_range)
    }

    /// 単独NPC内で検証可能な不変条件を検査する。
    pub fn validate(&self) -> Result<(), NpcInvariantError> {
        if !self.attributes.values_in_range() {
            return Err(NpcInvariantError::AttributeOutOfRange);
        }
        if self.beliefs.len() > MAX_BELIEFS {
            return Err(NpcInvariantError::TooManyBeliefs {
                count: self.beliefs.len(),
            });
        }

        let mut belief_kinds = HashSet::with_capacity(self.beliefs.len());
        for belief in &self.beliefs {
            if !belief.is_valid() {
                return Err(NpcInvariantError::BeliefOutOfRange { kind: belief.kind });
            }
            if !belief_kinds.insert(belief.kind) {
                return Err(NpcInvariantError::DuplicateBelief { kind: belief.kind });
            }
        }

        if !self.goal.is_valid() {
            return Err(NpcInvariantError::GoalProgressOutOfRange);
        }
        if self.relationships.contains_key(&self.id) {
            return Err(NpcInvariantError::SelfRelationship);
        }
        if let Some((other, _)) = self
            .relationships
            .iter()
            .find(|(_, relationship)| !relationship.values_in_range())
        {
            return Err(NpcInvariantError::RelationshipValueOutOfRange { other: *other });
        }

        validate_relatives(
            self.id,
            &self.parents,
            NpcInvariantError::SelfParent,
            |parent| NpcInvariantError::DuplicateParent { parent },
        )?;
        validate_relatives(
            self.id,
            &self.children,
            NpcInvariantError::SelfChild,
            |child| NpcInvariantError::DuplicateChild { child },
        )?;

        if self.partner == Some(self.id) {
            return Err(NpcInvariantError::SelfPartner);
        }
        if !self.alive && self.in_world {
            return Err(NpcInvariantError::DeadNpcMarkedInWorld);
        }
        Ok(())
    }

    /// 公開フィールドや外部データから入った値を安全な形へ修復する。
    /// World全体を必要とする相互参照までは変更しない。
    pub fn normalize(&mut self) {
        self.attributes.normalize();
        normalize_beliefs(&mut self.beliefs);
        self.goal.normalize();

        self.relationships.remove(&self.id);
        for relationship in self.relationships.values_mut() {
            relationship.normalize();
        }

        normalize_relatives(self.id, &mut self.parents);
        normalize_relatives(self.id, &mut self.children);
        if self.partner == Some(self.id) {
            self.partner = None;
        }
        if !self.alive {
            self.in_world = false;
        }
    }

    pub fn debug_assert_valid(&self) {
        debug_assert!(
            self.validate().is_ok(),
            "NPC {} violates an invariant: {:?}",
            self.id,
            self.validate().err()
        );
    }
}

pub const fn clamp_attribute(value: u8) -> u8 {
    if value > MAX_ATTRIBUTE {
        MAX_ATTRIBUTE
    } else {
        value
    }
}

fn average_attribute(parent_a: u8, parent_b: u8, variation: i8) -> u8 {
    let average = (i16::from(parent_a) + i16::from(parent_b)) / 2;
    (average + i16::from(variation)).clamp(i16::from(MIN_ATTRIBUTE), i16::from(MAX_ATTRIBUTE)) as u8
}

fn normalize_beliefs(beliefs: &mut Vec<Belief>) {
    let mut normalized: Vec<Belief> = Vec::with_capacity(beliefs.len().min(MAX_BELIEFS));
    for mut belief in beliefs.drain(..) {
        belief.normalize();
        if let Some(existing) = normalized
            .iter_mut()
            .find(|existing| existing.kind == belief.kind)
        {
            existing.strength = existing.strength.max(belief.strength);
        } else if normalized.len() < MAX_BELIEFS {
            normalized.push(belief);
        }
    }
    *beliefs = normalized;
}

fn normalize_relatives(owner: NpcId, relatives: &mut Vec<NpcId>) {
    let mut seen = HashSet::with_capacity(relatives.len());
    relatives.retain(|relative| *relative != owner && seen.insert(*relative));
}

fn add_unique_relative(owner: NpcId, relatives: &mut Vec<NpcId>, relative: NpcId) -> bool {
    if owner == relative || relatives.contains(&relative) {
        return false;
    }
    relatives.push(relative);
    true
}

fn remove_relative(relatives: &mut Vec<NpcId>, relative: NpcId) -> bool {
    let previous_len = relatives.len();
    relatives.retain(|candidate| *candidate != relative);
    relatives.len() != previous_len
}

fn validate_relatives<F>(
    owner: NpcId,
    relatives: &[NpcId],
    self_error: NpcInvariantError,
    duplicate_error: F,
) -> Result<(), NpcInvariantError>
where
    F: Fn(NpcId) -> NpcInvariantError,
{
    let mut seen = HashSet::with_capacity(relatives.len());
    for relative in relatives {
        if *relative == owner {
            return Err(self_error);
        }
        if !seen.insert(*relative) {
            return Err(duplicate_error(*relative));
        }
    }
    Ok(())
}

fn deserialize_attribute<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(clamp_attribute(u8::deserialize(deserializer)?))
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_npc() -> Npc {
        Npc::with_default_name(
            NpcId(1),
            30,
            Sex::Female,
            TownId(2),
            TownId(2),
            Attributes::default(),
            vec![
                Belief::new(BeliefKind::ProtectFamily, 9),
                Belief::new(BeliefKind::HelpOthers, 7),
            ],
            Goal::new(GoalKind::ProtectFamily, 0),
        )
    }

    #[test]
    fn attributes_clamp_every_input_path() {
        let attributes = Attributes::new(11, 12, 13, 14, 255);
        assert_eq!(attributes, Attributes::uniform(10));

        let attributes: Attributes = serde_json::from_str(
            r#"{"physical":255,"dexterity":20,"intelligence":10,"charisma":5,"willpower":0}"#,
        )
        .unwrap();
        assert_eq!(attributes, Attributes::new(10, 10, 10, 5, 0));
    }

    #[test]
    fn child_attributes_use_parent_average_and_clamp_variation() {
        let weak = Attributes::uniform(0);
        let strong = Attributes::uniform(10);
        assert_eq!(
            Attributes::average_with_variation(&weak, &strong, [-2, -1, 0, 1, 2]),
            Attributes::new(3, 4, 5, 6, 7)
        );
        assert_eq!(
            Attributes::average_with_variation(&strong, &strong, [100; 5]),
            Attributes::uniform(10)
        );
    }

    #[test]
    fn constructor_normalizes_beliefs_and_default_name() {
        let npc = Npc::with_default_name(
            NpcId(12),
            10,
            Sex::Male,
            TownId(1),
            TownId(1),
            Attributes::default(),
            vec![
                Belief::new(BeliefKind::HelpOthers, 2),
                Belief::new(BeliefKind::HelpOthers, 8),
                Belief::new(BeliefKind::ValueOrder, 5),
                Belief::new(BeliefKind::ValueKnowledge, 6),
                Belief::new(BeliefKind::ValueFreedom, 7),
            ],
            Goal::default(),
        );
        assert_eq!(npc.name, "NPC-000012");
        assert_eq!(npc.beliefs.len(), 3);
        assert_eq!(npc.belief_strength(BeliefKind::HelpOthers), 8);
        assert!(npc.validate().is_ok());
    }

    #[test]
    fn self_relationship_is_rejected() {
        let mut npc = test_npc();
        assert!(!npc.set_relationship(npc.id, Relationship::default()));
        assert!(npc.ensure_relationship(npc.id, 5, 0, 1).is_none());
        assert!(npc.relationships.is_empty());
    }

    #[test]
    fn stale_relationship_removal_is_deterministic_and_preserves_strong_links() {
        let mut npc = test_npc();
        for id in [NpcId(9), NpcId(3), NpcId(7)] {
            npc.set_relationship(
                id,
                Relationship::with_relation(5, 5, RelationshipKind::Acquaintance, 0, 1),
            );
        }
        npc.set_relationship(
            NpcId(5),
            Relationship::with_relation(5, 8, RelationshipKind::CloseFriend, 0, 1),
        );
        assert_eq!(
            npc.forget_stale_relationships(4),
            vec![NpcId(3), NpcId(7), NpcId(9)]
        );
        assert_eq!(
            npc.relationships.keys().copied().collect::<Vec<_>>(),
            vec![NpcId(5)]
        );
    }

    #[test]
    fn family_helpers_reject_self_and_duplicates() {
        let mut npc = test_npc();
        assert!(!npc.add_parent(npc.id));
        assert!(npc.add_parent(NpcId(2)));
        assert!(!npc.add_parent(NpcId(2)));
        assert!(npc.add_child(NpcId(3)));
        assert!(!npc.add_child(NpcId(3)));
        assert!(!npc.set_partner(npc.id));
        assert!(npc.set_partner(NpcId(4)));
        assert!(!npc.set_partner(NpcId(4)));
        assert!(npc.validate().is_ok());
    }

    #[test]
    fn death_and_external_departure_are_distinct() {
        let mut emigrant = test_npc();
        assert!(emigrant.leave_world());
        assert!(emigrant.alive);
        assert!(!emigrant.in_world);
        assert!(emigrant.enter_world(TownId(8)));
        assert_eq!(emigrant.town, TownId(8));

        emigrant.set_partner(NpcId(2));
        assert_eq!(emigrant.mark_dead(), Some(NpcId(2)));
        assert!(!emigrant.alive);
        assert!(!emigrant.in_world);
        assert!(!emigrant.enter_world(TownId(1)));
        assert!(emigrant.validate().is_ok());
    }

    #[test]
    fn normalize_repairs_locally_repairable_invariants() {
        let mut npc = test_npc();
        npc.attributes.physical = 200;
        npc.beliefs.push(Belief {
            kind: BeliefKind::HelpOthers,
            strength: 200,
        });
        npc.relationships.insert(
            npc.id,
            Relationship::with_relation(200, 200, RelationshipKind::Enemy, 0, 0),
        );
        npc.parents = vec![npc.id, NpcId(2), NpcId(2)];
        npc.children = vec![NpcId(3), npc.id, NpcId(3)];
        npc.partner = Some(npc.id);
        npc.alive = false;
        npc.in_world = true;

        npc.normalize();
        assert!(npc.validate().is_ok());
        assert!(!npc.in_world);
        assert_eq!(npc.parents, vec![NpcId(2)]);
        assert_eq!(npc.children, vec![NpcId(3)]);
        assert_eq!(npc.partner, None);
    }

    #[test]
    fn validate_reports_direct_public_field_corruption() {
        let mut npc = test_npc();
        npc.relationships.insert(npc.id, Relationship::default());
        assert_eq!(npc.validate(), Err(NpcInvariantError::SelfRelationship));
    }
}
