use serde::{Deserialize, Deserializer, Serialize};

pub const MIN_RELATIONSHIP_VALUE: u8 = 0;
pub const MAX_RELATIONSHIP_VALUE: u8 = 10;
pub const NEUTRAL_RELATION: u8 = 5;
pub const MAX_SINGLE_RELATION_CHANGE: i8 = 3;
pub const WEAK_RELATION_FORGET_YEARS: u64 = 4;
pub const FRIENDLY_RELATION_FORGET_YEARS: u64 = 8;

#[derive(
    Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum RelationshipKind {
    #[default]
    Acquaintance,
    Friend,
    CloseFriend,
    Family,
    Partner,
    Rival,
    Enemy,
}

impl RelationshipKind {
    pub const fn is_permanent(self) -> bool {
        matches!(self, Self::Family | Self::Partner)
    }
}

/// 2人の間の有向な関係。
///
/// `affinity`は基本的に固定の相性、`relation`は現在の関係値。どちらも
/// コンストラクタ、setter、JSON復元の各経路で`0..=10`へ丸められる。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Relationship {
    #[serde(deserialize_with = "deserialize_relationship_value")]
    pub affinity: u8,
    #[serde(deserialize_with = "deserialize_relationship_value")]
    pub relation: u8,
    pub kind: RelationshipKind,
    pub last_interaction_year: u64,
    #[serde(deserialize_with = "deserialize_month")]
    pub last_interaction_month: u8,
}

impl Relationship {
    /// 新規交流を中立の関係値5で作る。
    pub const fn new(affinity: u8, kind: RelationshipKind, year: u64, month: u8) -> Self {
        Self {
            affinity: clamp_relationship_value(affinity),
            relation: NEUTRAL_RELATION,
            kind,
            last_interaction_year: year,
            last_interaction_month: clamp_month(month),
        }
    }

    /// 初期関係値も明示したい、家族・既存関係の復元用コンストラクタ。
    pub const fn with_relation(
        affinity: u8,
        relation: u8,
        kind: RelationshipKind,
        year: u64,
        month: u8,
    ) -> Self {
        Self {
            affinity: clamp_relationship_value(affinity),
            relation: clamp_relationship_value(relation),
            kind,
            last_interaction_year: year,
            last_interaction_month: clamp_month(month),
        }
    }

    pub const fn values_in_range(&self) -> bool {
        self.affinity <= MAX_RELATIONSHIP_VALUE
            && self.relation <= MAX_RELATIONSHIP_VALUE
            && self.last_interaction_month >= 1
            && self.last_interaction_month <= 12
    }

    pub fn normalize(&mut self) {
        self.affinity = clamp_relationship_value(self.affinity);
        self.relation = clamp_relationship_value(self.relation);
        self.last_interaction_month = clamp_month(self.last_interaction_month);
    }

    pub fn set_affinity(&mut self, affinity: u8) {
        self.affinity = clamp_relationship_value(affinity);
    }

    pub fn set_relation(&mut self, relation: u8) {
        self.relation = clamp_relationship_value(relation);
    }

    /// 関係変化を適用する。一回の出来事は`-3..=3`、結果は`0..=10`。
    /// 戻り値は境界の丸めを反映した実際の変化量。
    pub fn adjust_relation(&mut self, requested_change: i8) -> i8 {
        let change =
            requested_change.clamp(-MAX_SINGLE_RELATION_CHANGE, MAX_SINGLE_RELATION_CHANGE);
        let previous = self.relation;
        self.relation = (i16::from(previous) + i16::from(change)).clamp(
            i16::from(MIN_RELATIONSHIP_VALUE),
            i16::from(MAX_RELATIONSHIP_VALUE),
        ) as u8;
        self.relation as i8 - previous as i8
    }

    pub fn record_interaction(&mut self, year: u64, month: u8) {
        self.last_interaction_year = year;
        self.last_interaction_month = clamp_month(month);
    }

    pub const fn is_strong(&self) -> bool {
        self.relation >= 7
    }

    pub const fn is_permanent(&self) -> bool {
        self.kind.is_permanent()
    }

    /// 年次tick向けの忘却判定。
    pub const fn should_forget(&self, current_year: u64) -> bool {
        if self.is_permanent() || self.relation >= 7 {
            return false;
        }

        let inactive_years = current_year.saturating_sub(self.last_interaction_year);
        if self.relation <= 5 {
            inactive_years >= WEAK_RELATION_FORGET_YEARS
        } else {
            // relation == 6（7以上は上で除外）
            inactive_years >= FRIENDLY_RELATION_FORGET_YEARS
        }
    }

    /// 月まで考慮する、より厳密な忘却判定。月は`1..=12`として扱う。
    pub const fn should_forget_at(&self, current_year: u64, current_month: u8) -> bool {
        if self.is_permanent() || self.relation >= 7 {
            return false;
        }

        let last = absolute_month(self.last_interaction_year, self.last_interaction_month);
        let current = absolute_month(current_year, current_month);
        let inactive_months = current.saturating_sub(last);
        let required_years = if self.relation <= 5 {
            WEAK_RELATION_FORGET_YEARS
        } else {
            FRIENDLY_RELATION_FORGET_YEARS
        };
        inactive_months >= required_years as u128 * 12
    }
}

impl Default for Relationship {
    fn default() -> Self {
        Self::new(5, RelationshipKind::Acquaintance, 0, 1)
    }
}

pub const fn clamp_relationship_value(value: u8) -> u8 {
    if value > MAX_RELATIONSHIP_VALUE {
        MAX_RELATIONSHIP_VALUE
    } else {
        value
    }
}

pub const fn clamp_month(month: u8) -> u8 {
    if month == 0 {
        1
    } else if month > 12 {
        12
    } else {
        month
    }
}

const fn absolute_month(year: u64, month: u8) -> u128 {
    year as u128 * 12 + (clamp_month(month) - 1) as u128
}

fn deserialize_relationship_value<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(clamp_relationship_value(u8::deserialize(deserializer)?))
}

fn deserialize_month<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(clamp_month(u8::deserialize(deserializer)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_relationship_is_neutral_and_values_are_clamped() {
        let relationship = Relationship::new(250, RelationshipKind::Acquaintance, 10, 0);
        assert_eq!(relationship.affinity, 10);
        assert_eq!(relationship.relation, 5);
        assert_eq!(relationship.last_interaction_month, 1);
        assert!(relationship.values_in_range());

        let existing = Relationship::with_relation(200, 100, RelationshipKind::Friend, 8, 80);
        assert_eq!((existing.affinity, existing.relation), (10, 10));
        assert_eq!(existing.last_interaction_month, 12);
    }

    #[test]
    fn relation_adjustment_never_leaves_range() {
        for initial in 0..=10 {
            for change in i8::MIN..=i8::MAX {
                let mut relationship =
                    Relationship::with_relation(5, initial, RelationshipKind::Acquaintance, 0, 1);
                let actual = relationship.adjust_relation(change);
                assert!(relationship.relation <= 10);
                assert!((-3..=3).contains(&actual));
            }
        }
    }

    #[test]
    fn weak_relationship_is_forgotten_after_four_full_years() {
        let relationship = Relationship::with_relation(5, 5, RelationshipKind::Acquaintance, 20, 6);
        assert!(!relationship.should_forget_at(24, 5));
        assert!(relationship.should_forget_at(24, 6));
        assert!(relationship.should_forget(24));
    }

    #[test]
    fn relation_six_needs_eight_years() {
        let relationship = Relationship::with_relation(5, 6, RelationshipKind::Friend, 20, 1);
        assert!(!relationship.should_forget(27));
        assert!(relationship.should_forget(28));
    }

    #[test]
    fn strong_family_and_partner_relationships_are_retained() {
        for relationship in [
            Relationship::with_relation(5, 7, RelationshipKind::Acquaintance, 0, 1),
            Relationship::with_relation(2, 0, RelationshipKind::Family, 0, 1),
            Relationship::with_relation(2, 0, RelationshipKind::Partner, 0, 1),
        ] {
            assert!(!relationship.should_forget(u64::MAX));
            assert!(!relationship.should_forget_at(u64::MAX, 12));
        }
    }

    #[test]
    fn serde_clamps_all_bounded_fields() {
        let relationship: Relationship = serde_json::from_str(
            r#"{"affinity":200,"relation":201,"kind":"Rival","last_interaction_year":3,"last_interaction_month":99}"#,
        )
        .unwrap();
        assert_eq!(relationship.affinity, 10);
        assert_eq!(relationship.relation, 10);
        assert_eq!(relationship.last_interaction_month, 12);
    }
}
