use std::collections::{BTreeMap, BTreeSet};

use npc_system::World;
use npc_system::belief::BeliefKind;
use npc_system::event::{TimedWorldEvent, WorldEvent};
use npc_system::goal::GoalKind;
use npc_system::id::{NpcId, TownId};
use npc_system::npc::{Npc, NpcState};
use npc_system::statistics::YearStatistics;

pub(crate) struct TimelineCollector {
    world_enabled: bool,
    world_years: Vec<WorldYearEntry>,
    towns: BTreeMap<TownId, TownTimeline>,
    npcs: BTreeMap<NpcId, NpcTimeline>,
}

impl TimelineCollector {
    pub(crate) fn new(
        world: &World,
        world_enabled: bool,
        town_ids: &[u16],
        npc_ids: &[u32],
    ) -> Self {
        let populations = world.town_populations();
        let towns = town_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|raw_id| {
                let id = TownId(raw_id);
                let initial_population = populations.get(usize::from(raw_id)).copied();
                (id, TownTimeline::new(id, initial_population))
            })
            .collect();
        let npcs = npc_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|raw_id| {
                let id = NpcId(raw_id);
                (id, NpcTimeline::new(world, id))
            })
            .collect();

        Self {
            world_enabled,
            world_years: Vec::new(),
            towns,
            npcs,
        }
    }

    pub(crate) fn record_year(&mut self, world: &World, statistics: &YearStatistics) {
        if self.world_enabled {
            self.world_years
                .push(WorldYearEntry::new(statistics, &world.year_events, world));
        }
        for timeline in self.towns.values_mut() {
            timeline.record_year(world, statistics);
        }
        for timeline in self.npcs.values_mut() {
            timeline.record_year(world);
        }
    }

    pub(crate) fn print(&self, world: &World) {
        if self.world_enabled {
            print_world_timeline(&self.world_years);
        }
        for timeline in self.towns.values() {
            timeline.print(world);
        }
        for timeline in self.npcs.values() {
            timeline.print(world);
        }
    }
}

struct WorldYearEntry {
    year: u64,
    population: usize,
    births: usize,
    deaths: usize,
    immigration: usize,
    emigration: usize,
    migrations: usize,
    partnerships: usize,
    notable: Vec<String>,
}

impl WorldYearEntry {
    fn new(statistics: &YearStatistics, events: &[TimedWorldEvent], world: &World) -> Self {
        let notable = unique_strings(events.iter().filter_map(|timed| {
            notable_world_event(world, &timed.event).map(|label| {
                if timed.month == 0 {
                    label
                } else {
                    format!("{}月 {label}", timed.month)
                }
            })
        }));
        Self {
            year: statistics.year,
            population: statistics.total_population,
            births: statistics.births,
            deaths: statistics.deaths,
            immigration: statistics.external_immigration,
            emigration: statistics.external_emigration,
            migrations: statistics.internal_migrations,
            partnerships: statistics.partnerships,
            notable,
        }
    }
}

fn print_world_timeline(entries: &[WorldYearEntry]) {
    println!("\n=== 世界タイムライン ===");
    if entries.is_empty() {
        println!("記録なし");
        return;
    }
    println!(
        "  {:>5} {:>9} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} 重大イベント",
        "年", "人口", "出生", "死亡", "流入", "流出", "移住", "提携"
    );
    let final_year = entries.last().map_or(0, |entry| entry.year);
    for entry in entries {
        if entry.year != 1
            && entry.year != final_year
            && entry.year % 10 != 0
            && entry.notable.is_empty()
        {
            continue;
        }
        println!(
            "  {:>5} {:>9} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {}",
            entry.year,
            format_number(entry.population),
            format_number(entry.births),
            format_number(entry.deaths),
            format_number(entry.immigration),
            format_number(entry.emigration),
            format_number(entry.migrations),
            format_number(entry.partnerships),
            entry.notable.join(" / ")
        );
    }
}

struct TownTimeline {
    id: TownId,
    initial_population: Option<usize>,
    previous_population: Option<usize>,
    years: Vec<TownYearEntry>,
}

impl TownTimeline {
    fn new(id: TownId, initial_population: Option<usize>) -> Self {
        Self {
            id,
            initial_population,
            previous_population: initial_population,
            years: Vec::new(),
        }
    }

    fn record_year(&mut self, world: &World, statistics: &YearStatistics) {
        let Some(&population) = statistics.town_populations.get(usize::from(self.id.0)) else {
            return;
        };
        let mut entry = TownYearEntry {
            year: statistics.year,
            population,
            population_change: population as i64
                - self.previous_population.unwrap_or(population) as i64,
            ..TownYearEntry::default()
        };
        for timed in &world.year_events {
            entry.record_event(world, self.id, timed);
        }
        self.previous_population = Some(population);
        self.years.push(entry);
    }

    fn print(&self, world: &World) {
        println!("\n=== 都市タイムライン: ID {} ===", self.id.0);
        let Some(town) = world.town(self.id) else {
            println!(
                "都市が見つかりません（存在するID: 0..={}）",
                world.towns.len().saturating_sub(1)
            );
            return;
        };
        println!(
            "{} / 収容力 {}",
            town.name,
            format_number(town.population_capacity as usize)
        );
        println!(
            "  {:>5} {:>9} {:>7} {:>6} {:>6} {:>6} {:>6} {:>6} 主な出来事",
            "年", "人口", "増減", "出生", "死亡", "転入", "転出", "提携"
        );
        if let Some(initial) = self.initial_population {
            println!("  {:>5} {:>9} {:>7}", 0, format_number(initial), "-");
        }
        for entry in &self.years {
            if !entry.should_print() {
                continue;
            }
            println!(
                "  {:>5} {:>9} {:>+7} {:>6} {:>6} {:>6} {:>6} {:>6} {}",
                entry.year,
                format_number(entry.population),
                entry.population_change,
                format_number(entry.births),
                format_number(entry.deaths),
                format_number(entry.arrivals),
                format_number(entry.departures),
                format_number(entry.partnerships),
                entry.notable.join(" / ")
            );
        }
    }
}

#[derive(Default)]
struct TownYearEntry {
    year: u64,
    population: usize,
    population_change: i64,
    births: usize,
    deaths: usize,
    arrivals: usize,
    departures: usize,
    partnerships: usize,
    notable: Vec<String>,
}

impl TownYearEntry {
    fn record_event(&mut self, world: &World, town: TownId, timed: &TimedWorldEvent) {
        match &timed.event {
            WorldEvent::Birth { npc } => {
                self.births += usize::from(npc_is_in_town(world, *npc, town));
            }
            WorldEvent::Death { npc } => {
                self.deaths += usize::from(npc_is_in_town(world, *npc, town));
            }
            WorldEvent::Partnership { a, b } => {
                self.partnerships +=
                    usize::from(npc_is_in_town(world, *a, town) || npc_is_in_town(world, *b, town));
            }
            WorldEvent::Migration { from, to, .. } => {
                self.departures += usize::from(*from == town);
                self.arrivals += usize::from(*to == town);
            }
            WorldEvent::ExternalImmigration { to, .. } => {
                self.arrivals += usize::from(*to == town);
            }
            WorldEvent::ExternalEmigration { from, .. } => {
                self.departures += usize::from(*from == town);
            }
            WorldEvent::NaturalDisaster { town: affected } if *affected == town => {
                self.notable.push(at_time(timed, "自然災害"));
            }
            WorldEvent::FamineStarted { town: affected } if *affected == town => {
                self.notable.push(at_time(timed, "飢饉発生"));
            }
            WorldEvent::FamineEnded { town: affected } if *affected == town => {
                self.notable.push(at_time(timed, "飢饉終息"));
            }
            _ => {}
        }
    }

    fn should_print(&self) -> bool {
        self.population_change != 0
            || self.births > 0
            || self.deaths > 0
            || self.arrivals > 0
            || self.departures > 0
            || self.partnerships > 0
            || !self.notable.is_empty()
    }
}

struct NpcTimeline {
    id: NpcId,
    previous: Option<NpcSnapshot>,
    entries: Vec<NpcTimelineEntry>,
}

impl NpcTimeline {
    fn new(world: &World, id: NpcId) -> Self {
        let previous = world.npc(id).map(NpcSnapshot::from);
        let mut entries = Vec::new();
        if let Some(npc) = world.npc(id) {
            entries.push(NpcTimelineEntry::new(
                0,
                0,
                format!(
                    "初期状態: {}歳、{}、目標「{}」",
                    npc.age,
                    town_name(world, npc.town),
                    goal_label(npc.goal.kind)
                ),
            ));
        }
        Self {
            id,
            previous,
            entries,
        }
    }

    fn record_year(&mut self, world: &World) {
        let current = world.npc(self.id).map(NpcSnapshot::from);
        let mut relationship_changes = 0usize;
        let mut appeared_by_event = false;
        for timed in &world.year_events {
            if !timed.event.npc_ids().contains(&self.id) {
                continue;
            }
            if matches!(timed.event, WorldEvent::RelationshipChanged { .. }) {
                relationship_changes += 1;
                continue;
            }
            appeared_by_event |= matches!(
                timed.event,
                WorldEvent::Birth { npc } | WorldEvent::ExternalImmigration { npc, .. }
                    if npc == self.id
            );
            if let Some(description) = npc_event_label(world, self.id, &timed.event) {
                self.entries
                    .push(NpcTimelineEntry::new(timed.year, timed.month, description));
            }
        }
        if relationship_changes > 0 {
            self.entries.push(NpcTimelineEntry::new(
                world.year,
                13,
                format!("年間の人間関係変化: {relationship_changes}回"),
            ));
        }

        if self.previous.is_none() && current.is_some() && !appeared_by_event {
            self.entries.push(NpcTimelineEntry::new(
                world.year,
                0,
                "世界に登場".to_owned(),
            ));
        }
        if let (Some(previous), Some(current)) = (self.previous.clone(), &current) {
            self.record_snapshot_changes(world, &previous, current);
        }
        self.previous = current;
    }

    fn record_snapshot_changes(
        &mut self,
        world: &World,
        previous: &NpcSnapshot,
        current: &NpcSnapshot,
    ) {
        if let (Some(former), None) = (previous.partner, current.partner) {
            let month = world
                .year_events
                .iter()
                .find_map(|timed| match timed.event {
                    WorldEvent::Death { npc } if npc == former => Some(timed.month),
                    _ => None,
                })
                .unwrap_or(0);
            self.entries.push(NpcTimelineEntry::new(
                world.year,
                month,
                format!("パートナーを失う: {}", npc_name(world, former)),
            ));
        }
        for child in current
            .children
            .iter()
            .filter(|child| !previous.children.contains(child))
        {
            let month = world
                .year_events
                .iter()
                .find_map(|timed| match timed.event {
                    WorldEvent::Birth { npc } if npc == *child => Some(timed.month),
                    _ => None,
                })
                .unwrap_or(0);
            self.entries.push(NpcTimelineEntry::new(
                world.year,
                month,
                format!("子どもが誕生: {}", npc_name(world, *child)),
            ));
        }
        if current.alive && previous.state != current.state {
            self.entries.push(NpcTimelineEntry::new(
                world.year,
                13,
                format!("年末状態が「{}」に変化", state_label(current.state)),
            ));
        }
    }

    fn print(&self, world: &World) {
        println!("\n=== NPCタイムライン: ID {} ===", self.id.0);
        let Some(npc) = world.npc(self.id) else {
            println!(
                "NPCが見つかりません（存在するID: 0..={}）",
                world.npcs.len().saturating_sub(1)
            );
            return;
        };
        println!("{} / {} / {}", npc.name, npc_age(npc), npc_status(npc));
        if self.entries.is_empty() {
            println!("記録なし");
            return;
        }
        let mut entries = self.entries.iter().collect::<Vec<_>>();
        entries.sort_by_key(|entry| (entry.year, entry.month));
        for entry in entries {
            println!(
                "  {}  {}",
                timestamp(entry.year, entry.month),
                entry.description
            );
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct NpcSnapshot {
    alive: bool,
    state: NpcState,
    partner: Option<NpcId>,
    children: Vec<NpcId>,
}

impl From<&Npc> for NpcSnapshot {
    fn from(npc: &Npc) -> Self {
        Self {
            alive: npc.alive,
            state: npc.state,
            partner: npc.partner,
            children: npc.children.clone(),
        }
    }
}

struct NpcTimelineEntry {
    year: u64,
    month: u8,
    description: String,
}

impl NpcTimelineEntry {
    fn new(year: u64, month: u8, description: String) -> Self {
        Self {
            year,
            month,
            description,
        }
    }
}

fn notable_world_event(world: &World, event: &WorldEvent) -> Option<String> {
    match event {
        WorldEvent::NaturalDisaster { town } => {
            Some(format!("{}で自然災害", town_name(world, *town)))
        }
        WorldEvent::DiseaseOutbreak => Some("疫病発生".to_owned()),
        WorldEvent::WarStarted => Some("戦争開始".to_owned()),
        WorldEvent::WarEnded => Some("戦争終結".to_owned()),
        WorldEvent::FamineStarted { town } => {
            Some(format!("{}で飢饉発生", town_name(world, *town)))
        }
        WorldEvent::FamineEnded { town } => Some(format!("{}で飢饉終息", town_name(world, *town))),
        _ => None,
    }
}

fn npc_event_label(world: &World, target: NpcId, event: &WorldEvent) -> Option<String> {
    match event {
        WorldEvent::Birth { npc } if *npc == target => Some("出生".to_owned()),
        WorldEvent::Death { npc } if *npc == target => Some("死亡".to_owned()),
        WorldEvent::Partnership { a, b } => {
            let other = if *a == target { *b } else { *a };
            Some(format!("パートナー成立: {}", npc_name(world, other)))
        }
        WorldEvent::Migration { from, to, .. } => Some(format!(
            "都市移住: {} → {}",
            town_name(world, *from),
            town_name(world, *to)
        )),
        WorldEvent::BeliefChanged { belief, .. } => {
            Some(format!("信念が変化: {}", belief_label(*belief)))
        }
        WorldEvent::GoalChanged { old, new, .. } => Some(format!(
            "目標変更: {} → {}",
            goal_label(*old),
            goal_label(*new)
        )),
        WorldEvent::ExternalImmigration { to, .. } => {
            Some(format!("外部から流入: {}", town_name(world, *to)))
        }
        WorldEvent::ExternalEmigration { from, .. } => {
            Some(format!("外部へ転出: {}", town_name(world, *from)))
        }
        _ => None,
    }
}

fn npc_is_in_town(world: &World, npc: NpcId, town: TownId) -> bool {
    world.npc(npc).is_some_and(|npc| npc.town == town)
}

fn town_name(world: &World, id: TownId) -> &str {
    world
        .town(id)
        .map_or("不明な都市", |town| town.name.as_str())
}

fn npc_name(world: &World, id: NpcId) -> String {
    world.npc(id).map_or_else(
        || format!("NPC #{id}"),
        |npc| format!("{} [ID: {}]", npc.name, id.0),
    )
}

fn npc_status(npc: &Npc) -> &'static str {
    match (npc.alive, npc.in_world) {
        (false, _) => "死亡",
        (true, false) => "生存・外部転出",
        (true, true) => "生存・世界内",
    }
}

fn npc_age(npc: &Npc) -> String {
    match (npc.alive, npc.in_world) {
        (false, _) => format!("{}歳（死亡時）", npc.age),
        (true, false) => format!("{}歳（転出時）", npc.age),
        (true, true) => format!("{}歳", npc.age),
    }
}

fn timestamp(year: u64, month: u8) -> String {
    match month {
        0 => format!("{year:>4}年    "),
        13 => format!("{year:>4}年末  "),
        _ => format!("{year:>4}年{month:02}月"),
    }
}

fn at_time(timed: &TimedWorldEvent, label: &str) -> String {
    if timed.month == 0 {
        label.to_owned()
    } else {
        format!("{}月 {label}", timed.month)
    }
}

fn unique_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn state_label(state: NpcState) -> &'static str {
    match state {
        NpcState::Normal => "通常",
        NpcState::Sick => "病気",
        NpcState::Evacuating => "避難中",
    }
}

fn goal_label(goal: GoalKind) -> &'static str {
    match goal {
        GoalKind::Survive => "生き延びる",
        GoalKind::ProtectFamily => "家族を守る",
        GoalKind::FindPartner => "パートナーを探す",
        GoalKind::RaiseChildren => "子どもを育てる",
        GoalKind::BecomeSkilled => "技能を身につける",
        GoalKind::GainWealth => "富を得る",
        GoalKind::GainStatus => "地位を得る",
        GoalKind::MoveToBetterTown => "より良い都市へ移る",
        GoalKind::ProtectTown => "都市を守る",
        GoalKind::SeekKnowledge => "知識を求める",
        GoalKind::LivePeacefully => "平穏に暮らす",
    }
}

fn belief_label(belief: BeliefKind) -> &'static str {
    match belief {
        BeliefKind::ProtectFamily => "家族を守る",
        BeliefKind::HelpOthers => "他者を助ける",
        BeliefKind::KeepPromises => "約束を守る",
        BeliefKind::ValueFreedom => "自由を重んじる",
        BeliefKind::ValueOrder => "秩序を重んじる",
        BeliefKind::ValueWealth => "富を重んじる",
        BeliefKind::ValueKnowledge => "知識を重んじる",
        BeliefKind::ProtectHometown => "故郷を守る",
        BeliefKind::DistrustOutsiders => "外部者を警戒する",
        BeliefKind::JudgeIndividuals => "個人を見て判断する",
    }
}

fn format_number(value: usize) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(character);
    }
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_timeline_filters_regular_events_from_notable_labels() {
        let world = World::empty(1);
        assert!(notable_world_event(&world, &WorldEvent::Birth { npc: NpcId(1) }).is_none());
        assert_eq!(
            notable_world_event(&world, &WorldEvent::DiseaseOutbreak),
            Some("疫病発生".to_owned())
        );
    }

    #[test]
    fn timestamp_distinguishes_annual_and_monthly_events() {
        assert_eq!(timestamp(12, 0), "  12年    ");
        assert_eq!(timestamp(12, 3), "  12年03月");
        assert_eq!(timestamp(12, 13), "  12年末  ");
    }
}
