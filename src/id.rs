use std::fmt;

use serde::{Deserialize, Serialize};

/// NPCを指す、プロセス内で一意な整数ID。
///
/// NPC本体を`Vec`から削除しても参照がずれないよう、NPC間の参照には
/// インデックスやポインタではなく、この型を使用する。
#[derive(
    Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct NpcId(pub u32);

impl NpcId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl From<u32> for NpcId {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<NpcId> for u32 {
    fn from(value: NpcId) -> Self {
        value.0
    }
}

impl fmt::Display for NpcId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// 都市を指す、プロセス内で一意な整数ID。
#[derive(
    Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct TownId(pub u16);

impl TownId {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

impl From<u16> for TownId {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

impl From<TownId> for u16 {
    fn from(value: TownId) -> Self {
        value.0
    }
}

impl fmt::Display for TownId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use super::*;

    #[test]
    fn ids_are_hashable_and_ordered() {
        let mut hash_ids = HashSet::new();
        hash_ids.insert(NpcId::new(7));
        assert!(hash_ids.contains(&NpcId(7)));

        let ordered = [TownId(3), TownId(1), TownId(2)]
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        assert_eq!(ordered, vec![TownId(1), TownId(2), TownId(3)]);
    }

    #[test]
    fn ids_round_trip_as_transparent_serde_values() {
        assert_eq!(serde_json::to_string(&NpcId(42)).unwrap(), "42");
        assert_eq!(serde_json::from_str::<TownId>("12").unwrap(), TownId(12));
    }

    #[test]
    fn ids_convert_to_and_from_their_integer_types() {
        let npc = NpcId::from(99_u32);
        let town = TownId::from(8_u16);
        assert_eq!(u32::from(npc), 99);
        assert_eq!(u16::from(town), 8);
        assert_eq!(npc.to_string(), "99");
        assert_eq!(town.to_string(), "8");
    }
}
