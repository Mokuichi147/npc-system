use serde::{Deserialize, Deserializer, Serialize};

pub const MIN_BELIEF_STRENGTH: u8 = 0;
pub const MAX_BELIEF_STRENGTH: u8 = 10;
pub const RECOMMENDED_MIN_BELIEFS: usize = 2;
pub const MAX_BELIEFS: usize = 3;
pub const MAX_SINGLE_BELIEF_CHANGE: i8 = 3;

/// 初期プロトタイプで扱う固定の信念。
#[derive(
    Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum BeliefKind {
    #[default]
    ProtectFamily,
    HelpOthers,
    KeepPromises,
    ValueFreedom,
    ValueOrder,
    ValueWealth,
    ValueKnowledge,
    ProtectHometown,
    DistrustOutsiders,
    JudgeIndividuals,
}

impl BeliefKind {
    pub const ALL: [Self; 10] = [
        Self::ProtectFamily,
        Self::HelpOthers,
        Self::KeepPromises,
        Self::ValueFreedom,
        Self::ValueOrder,
        Self::ValueWealth,
        Self::ValueKnowledge,
        Self::ProtectHometown,
        Self::DistrustOutsiders,
        Self::JudgeIndividuals,
    ];

    pub const fn all() -> &'static [Self] {
        &Self::ALL
    }
}

/// NPCが持つ信念と、その強さ。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Belief {
    pub kind: BeliefKind,
    #[serde(deserialize_with = "deserialize_strength")]
    pub strength: u8,
}

impl Belief {
    /// 範囲外の強さは`0..=10`へ丸める。
    pub const fn new(kind: BeliefKind, strength: u8) -> Self {
        Self {
            kind,
            strength: clamp_strength(strength),
        }
    }

    pub const fn is_valid(&self) -> bool {
        self.strength <= MAX_BELIEF_STRENGTH
    }

    pub fn normalize(&mut self) {
        self.strength = clamp_strength(self.strength);
    }

    pub fn set_strength(&mut self, strength: u8) {
        self.strength = clamp_strength(strength);
    }

    /// 重大イベントによる強度変化を適用する。
    ///
    /// 一度の変化は設計上の最大値である`-3..=3`へ丸められ、結果も
    /// `0..=10`に収まる。戻り値は、境界での丸めを反映した実際の変化量。
    pub fn adjust(&mut self, requested_change: i8) -> i8 {
        let change = requested_change.clamp(-MAX_SINGLE_BELIEF_CHANGE, MAX_SINGLE_BELIEF_CHANGE);
        let previous = self.strength;
        let adjusted = i16::from(previous) + i16::from(change);
        self.strength = adjusted.clamp(
            i16::from(MIN_BELIEF_STRENGTH),
            i16::from(MAX_BELIEF_STRENGTH),
        ) as u8;
        self.strength as i8 - previous as i8
    }
}

impl Default for Belief {
    fn default() -> Self {
        Self::new(BeliefKind::default(), 5)
    }
}

pub const fn clamp_strength(strength: u8) -> u8 {
    if strength > MAX_BELIEF_STRENGTH {
        MAX_BELIEF_STRENGTH
    } else {
        strength
    }
}

fn deserialize_strength<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(clamp_strength(u8::deserialize(deserializer)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_and_setter_clamp_strength() {
        let mut belief = Belief::new(BeliefKind::HelpOthers, 255);
        assert_eq!(belief.strength, 10);
        assert!(belief.is_valid());

        belief.set_strength(42);
        assert_eq!(belief.strength, 10);
    }

    #[test]
    fn adjustment_is_limited_per_event_and_at_boundaries() {
        let mut belief = Belief::new(BeliefKind::KeepPromises, 5);
        assert_eq!(belief.adjust(100), 3);
        assert_eq!(belief.strength, 8);
        assert_eq!(belief.adjust(-100), -3);
        assert_eq!(belief.strength, 5);

        belief.set_strength(9);
        assert_eq!(belief.adjust(3), 1);
        assert_eq!(belief.strength, 10);
        assert_eq!(belief.adjust(-30), -3);
        assert_eq!(belief.strength, 7);
    }

    #[test]
    fn deserialization_also_clamps_strength() {
        let belief: Belief =
            serde_json::from_str(r#"{"kind":"ValueKnowledge","strength":200}"#).unwrap();
        assert_eq!(belief, Belief::new(BeliefKind::ValueKnowledge, 10));
    }

    #[test]
    fn all_contains_each_kind_once() {
        let mut kinds = BeliefKind::all().to_vec();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), BeliefKind::ALL.len());
        assert_eq!(BeliefKind::ALL.len(), 10);
    }
}
