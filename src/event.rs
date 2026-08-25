use crate::belief::BeliefKind;
use crate::goal::GoalKind;
use crate::id::{NpcId, TownId};
use serde::{Deserialize, Serialize};

/// 年次死亡統計を分類するための原因。イベントログの `Death` 自体は簡潔に保つ。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeathCause {
    #[default]
    Natural,
    Disaster,
    Disease,
    War,
    Famine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorldEventKind {
    Birth,
    Death,
    Partnership,
    Migration,
    RelationshipChanged,
    BeliefChanged,
    GoalChanged,
    NaturalDisaster,
    DiseaseOutbreak,
    WarStarted,
    WarEnded,
    ExternalImmigration,
    ExternalEmigration,
    FamineStarted,
    FamineEnded,
}

/// 永続ログへ残す価値のある、低頻度な世界イベントだけを表す。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorldEvent {
    Birth {
        npc: NpcId,
    },
    Death {
        npc: NpcId,
    },
    Partnership {
        a: NpcId,
        b: NpcId,
    },
    Migration {
        npc: NpcId,
        from: TownId,
        to: TownId,
    },
    RelationshipChanged {
        a: NpcId,
        b: NpcId,
    },
    BeliefChanged {
        npc: NpcId,
        belief: BeliefKind,
    },
    GoalChanged {
        npc: NpcId,
        old: GoalKind,
        new: GoalKind,
    },
    NaturalDisaster {
        town: TownId,
    },
    DiseaseOutbreak,
    WarStarted,
    WarEnded,
    ExternalImmigration {
        npc: NpcId,
        to: TownId,
    },
    ExternalEmigration {
        npc: NpcId,
        from: TownId,
    },
    FamineStarted {
        town: TownId,
    },
    FamineEnded {
        town: TownId,
    },
}

/// 1年分のタイムライン表示に使う、発生時刻付きイベント。
///
/// `month=0` は年初の年次処理、`1..=12` は月次処理を表す。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedWorldEvent {
    pub year: u64,
    pub month: u8,
    pub event: WorldEvent,
}

impl TimedWorldEvent {
    pub const fn new(year: u64, month: u8, event: WorldEvent) -> Self {
        Self { year, month, event }
    }
}

impl WorldEvent {
    pub const fn kind(&self) -> WorldEventKind {
        match self {
            Self::Birth { .. } => WorldEventKind::Birth,
            Self::Death { .. } => WorldEventKind::Death,
            Self::Partnership { .. } => WorldEventKind::Partnership,
            Self::Migration { .. } => WorldEventKind::Migration,
            Self::RelationshipChanged { .. } => WorldEventKind::RelationshipChanged,
            Self::BeliefChanged { .. } => WorldEventKind::BeliefChanged,
            Self::GoalChanged { .. } => WorldEventKind::GoalChanged,
            Self::NaturalDisaster { .. } => WorldEventKind::NaturalDisaster,
            Self::DiseaseOutbreak => WorldEventKind::DiseaseOutbreak,
            Self::WarStarted => WorldEventKind::WarStarted,
            Self::WarEnded => WorldEventKind::WarEnded,
            Self::ExternalImmigration { .. } => WorldEventKind::ExternalImmigration,
            Self::ExternalEmigration { .. } => WorldEventKind::ExternalEmigration,
            Self::FamineStarted { .. } => WorldEventKind::FamineStarted,
            Self::FamineEnded { .. } => WorldEventKind::FamineEnded,
        }
    }

    /// イベントに直接登場するNPC。順序はvariant内の順序で安定している。
    pub fn npc_ids(&self) -> Vec<NpcId> {
        match self {
            Self::Birth { npc }
            | Self::Death { npc }
            | Self::Migration { npc, .. }
            | Self::BeliefChanged { npc, .. }
            | Self::GoalChanged { npc, .. }
            | Self::ExternalImmigration { npc, .. }
            | Self::ExternalEmigration { npc, .. } => vec![*npc],
            Self::Partnership { a, b } | Self::RelationshipChanged { a, b } => vec![*a, *b],
            _ => Vec::new(),
        }
    }

    /// イベントに直接登場する都市。移住では出発地、到着地の順。
    pub fn town_ids(&self) -> Vec<TownId> {
        match self {
            Self::Migration { from, to, .. } => vec![*from, *to],
            Self::NaturalDisaster { town }
            | Self::FamineStarted { town }
            | Self::FamineEnded { town } => vec![*town],
            Self::ExternalImmigration { to, .. } => vec![*to],
            Self::ExternalEmigration { from, .. } => vec![*from],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_exposes_stable_entities() {
        let event = WorldEvent::Migration {
            npc: NpcId(3),
            from: TownId(1),
            to: TownId(2),
        };
        assert_eq!(event.kind(), WorldEventKind::Migration);
        assert_eq!(event.npc_ids(), vec![NpcId(3)]);
        assert_eq!(event.town_ids(), vec![TownId(1), TownId(2)]);
    }

    #[test]
    fn event_json_round_trip_is_lossless() {
        let event = WorldEvent::Partnership {
            a: NpcId(10),
            b: NpcId(20),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<WorldEvent>(&json).unwrap(), event);
    }

    #[test]
    fn timed_event_json_round_trip_preserves_timestamp() {
        let timed = TimedWorldEvent::new(
            42,
            7,
            WorldEvent::Migration {
                npc: NpcId(3),
                from: TownId(1),
                to: TownId(2),
            },
        );
        let json = serde_json::to_string(&timed).unwrap();
        assert_eq!(
            serde_json::from_str::<TimedWorldEvent>(&json).unwrap(),
            timed
        );
    }
}
