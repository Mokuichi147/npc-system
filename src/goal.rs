use serde::{Deserialize, Deserializer, Serialize};

pub const MIN_GOAL_PROGRESS: f32 = 0.0;
pub const MAX_GOAL_PROGRESS: f32 = 1.0;

#[derive(
    Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum GoalKind {
    #[default]
    Survive,
    ProtectFamily,
    FindPartner,
    RaiseChildren,
    BecomeSkilled,
    GainWealth,
    GainStatus,
    MoveToBetterTown,
    ProtectTown,
    SeekKnowledge,
    LivePeacefully,
}

impl GoalKind {
    pub const ALL: [Self; 11] = [
        Self::Survive,
        Self::ProtectFamily,
        Self::FindPartner,
        Self::RaiseChildren,
        Self::BecomeSkilled,
        Self::GainWealth,
        Self::GainStatus,
        Self::MoveToBetterTown,
        Self::ProtectTown,
        Self::SeekKnowledge,
        Self::LivePeacefully,
    ];

    pub const fn all() -> &'static [Self] {
        &Self::ALL
    }
}

/// NPCが現在追っている唯一の主要目標。
///
/// `progress`は`0.0..=1.0`の正規化値。`since_year`はクールダウンの
/// 判定基準であり、通常の目標変更はそこから指定年数が経つまで抑止する。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Goal {
    pub kind: GoalKind,
    #[serde(deserialize_with = "deserialize_progress")]
    pub progress: f32,
    pub since_year: u64,
}

impl Goal {
    pub const fn new(kind: GoalKind, since_year: u64) -> Self {
        Self {
            kind,
            progress: MIN_GOAL_PROGRESS,
            since_year,
        }
    }

    pub fn with_progress(kind: GoalKind, progress: f32, since_year: u64) -> Self {
        Self {
            kind,
            progress: clamp_progress(progress),
            since_year,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.progress.is_finite()
            && (MIN_GOAL_PROGRESS..=MAX_GOAL_PROGRESS).contains(&self.progress)
    }

    pub fn normalize(&mut self) {
        self.progress = clamp_progress(self.progress);
    }

    pub fn set_progress(&mut self, progress: f32) {
        self.progress = clamp_progress(progress);
    }

    pub fn advance(&mut self, amount: f32) -> f32 {
        let previous = self.progress;
        self.progress = clamp_progress(self.progress + finite_or_zero(amount));
        self.progress - previous
    }

    pub fn is_complete(&self) -> bool {
        self.progress >= MAX_GOAL_PROGRESS
    }

    pub const fn years_held(&self, current_year: u64) -> u64 {
        current_year.saturating_sub(self.since_year)
    }

    /// 通常変更ならクールダウンを確認し、重大イベントなら即時変更を許す。
    pub const fn can_change(
        &self,
        current_year: u64,
        cooldown_years: u16,
        major_event: bool,
    ) -> bool {
        major_event || self.years_held(current_year) >= cooldown_years as u64
    }

    /// 変更できた場合だけ種類・開始年・進捗をまとめて更新する。
    pub fn change_to(
        &mut self,
        new_kind: GoalKind,
        current_year: u64,
        cooldown_years: u16,
        major_event: bool,
    ) -> bool {
        if self.kind == new_kind || !self.can_change(current_year, cooldown_years, major_event) {
            return false;
        }

        self.kind = new_kind;
        self.progress = MIN_GOAL_PROGRESS;
        self.since_year = current_year;
        true
    }
}

impl Default for Goal {
    fn default() -> Self {
        Self::new(GoalKind::default(), 0)
    }
}

pub fn clamp_progress(progress: f32) -> f32 {
    if progress.is_nan() {
        MIN_GOAL_PROGRESS
    } else {
        progress.clamp(MIN_GOAL_PROGRESS, MAX_GOAL_PROGRESS)
    }
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

fn deserialize_progress<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(clamp_progress(f32::deserialize(deserializer)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_always_normalized() {
        let mut goal = Goal::with_progress(GoalKind::BecomeSkilled, 3.5, 10);
        assert_eq!(goal.progress, 1.0);
        assert!(goal.is_complete());

        goal.set_progress(-2.0);
        assert_eq!(goal.progress, 0.0);
        assert_eq!(goal.advance(0.4), 0.4);
        assert_eq!(goal.advance(f32::NAN), 0.0);
        assert!(goal.is_valid());
    }

    #[test]
    fn normal_change_obeys_cooldown() {
        let mut goal = Goal::new(GoalKind::GainWealth, 20);
        assert!(!goal.change_to(GoalKind::GainStatus, 22, 3, false));
        assert_eq!(goal.kind, GoalKind::GainWealth);

        goal.advance(0.8);
        assert!(goal.change_to(GoalKind::GainStatus, 23, 3, false));
        assert_eq!(goal.kind, GoalKind::GainStatus);
        assert_eq!(goal.progress, 0.0);
        assert_eq!(goal.since_year, 23);
    }

    #[test]
    fn major_event_bypasses_cooldown_but_noop_is_not_a_change() {
        let mut goal = Goal::new(GoalKind::LivePeacefully, 100);
        assert!(goal.change_to(GoalKind::Survive, 100, 3, true));
        assert!(!goal.change_to(GoalKind::Survive, 100, 3, true));
    }

    #[test]
    fn deserialization_clamps_progress() {
        let goal: Goal =
            serde_json::from_str(r#"{"kind":"SeekKnowledge","progress":12.0,"since_year":4}"#)
                .unwrap();
        assert_eq!(goal.progress, 1.0);
        assert!(goal.is_valid());
    }

    #[test]
    fn all_goal_kinds_are_unique() {
        let mut kinds = GoalKind::all().to_vec();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), 11);
    }
}
