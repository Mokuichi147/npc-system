use crate::config::SimulationConfig;
use crate::id::NpcId;
use crate::statistics::YearStatistics;
use crate::utility::Action;
use crate::world::World;

/// シミュレーション本体から独立して年・月・死亡イベントへ接続する拡張機能。
pub trait SimulationExtension {
    const ID: &'static str;

    fn begin_year(world: &mut World);
    fn run_month(world: &mut World, config: &SimulationConfig, actions: &[Option<Action>]);
    fn before_npc_death(world: &mut World, deceased: NpcId, family: &[NpcId]);
    fn finish_year(world: &World, statistics: &mut YearStatistics);
}

#[cfg(feature = "economy-extension")]
pub mod economy;

pub fn enabled_extension_ids() -> &'static [&'static str] {
    #[cfg(feature = "economy-extension")]
    {
        &[economy::EconomyExtension::ID]
    }
    #[cfg(not(feature = "economy-extension"))]
    {
        &[]
    }
}
