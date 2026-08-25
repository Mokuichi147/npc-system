mod cli_timeline;

use clap::{Args, Parser, Subcommand};
use cli_timeline::TimelineCollector;
use npc_system::belief::BeliefKind;
use npc_system::goal::GoalKind;
use npc_system::id::NpcId;
use npc_system::npc::{Npc, NpcState, Sex};
use npc_system::statistics::{
    CumulativeStatistics, SimulationHealthMetrics, SimulationWarning, YearStatistics,
};
use npc_system::{Simulation, SimulationConfig, SimulationError, World, WorldDanger};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::num::{NonZeroU16, NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Parser)]
#[command(
    name = "npc-system",
    version,
    about = "ルールベースでNPC社会の世代交代を検証するCLIシミュレーター"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// NPC・都市・世代交代シミュレーションを実行する。
    Simulate(SimulateArgs),
    /// status JSONを読み、シミュレーションの稼働状態を表示する。
    Status(StatusArgs),
}

#[derive(Debug, Args)]
struct SimulateArgs {
    /// 生成する都市数（1以上）。
    #[arg(long)]
    towns: NonZeroU16,

    /// 初期NPC数（1以上）。
    #[arg(long)]
    population: NonZeroU32,

    /// シミュレーションする年数（1以上）。省略すると無期限に実行する。
    #[arg(long)]
    years: Option<NonZeroU64>,

    /// 再現可能な乱数seed。
    #[arg(long)]
    seed: u64,

    /// 負の世界イベントの頻度。
    #[arg(long, default_value_t = WorldDanger::Normal)]
    world_danger: WorldDanger,

    /// 年次統計を保存するJSONファイル。
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// 終了時に詳細表示するNPC ID。複数回指定できる。
    #[arg(long = "npc", value_name = "ID")]
    npc_ids: Vec<u32>,

    /// 10年ごとの進捗を省略し、最終結果だけを表示する。
    #[arg(long)]
    summary_only: bool,

    /// 世界全体の年次統計と重大イベントをタイムライン表示する。
    #[arg(long)]
    timeline_world: bool,

    /// 指定都市IDの人口・出生死亡・転入出をタイムライン表示する。複数回指定できる。
    #[arg(long = "timeline-town", value_name = "ID")]
    timeline_town_ids: Vec<u16>,

    /// 指定NPC IDの人生イベントをタイムライン表示する。複数回指定できる。
    #[arg(long = "timeline-npc", value_name = "ID")]
    timeline_npc_ids: Vec<u32>,

    /// 稼働状態を原子的に更新するJSONファイル。無期限実行時の既定値は npc-system-status.json。
    #[arg(long, value_name = "FILE")]
    status_file: Option<PathBuf>,

    /// status JSONを更新する間隔（シミュレーション年）。
    #[arg(long, default_value_t = NonZeroU64::new(1).expect("non-zero"))]
    status_interval_years: NonZeroU64,
}

#[derive(Debug, Args)]
struct StatusArgs {
    /// 読み込むstatus JSONファイル。
    #[arg(long, value_name = "FILE", default_value = "npc-system-status.json")]
    file: PathBuf,

    /// 指定秒ごとに状態を再読込して監視し続ける。
    #[arg(long)]
    watch: bool,

    /// --watch の更新間隔（秒）。
    #[arg(long, default_value_t = NonZeroU64::new(2).expect("non-zero"))]
    interval_seconds: NonZeroU64,

    /// 最終更新からこの秒数を超えたらstaleと表示する。
    #[arg(long, default_value_t = 30)]
    stale_after_seconds: u64,
}

#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("シミュレーションに失敗しました: {0}")]
    Simulation(#[from] SimulationError),

    #[error("JSON出力ファイル '{path}' を作成できません: {source}")]
    CreateOutput {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("JSON出力ファイル '{path}' へのシリアライズに失敗しました: {source}")]
    SerializeOutput {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("JSON出力ファイル '{path}' への書き込みに失敗しました: {source}")]
    WriteOutput {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("無期限実行では {0} を使用できません（履歴が増え続けるため）")]
    UnsupportedContinuousOption(&'static str),

    #[error("statusファイル '{path}' の処理に失敗しました: {source}")]
    StatusIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("statusファイル '{path}' のJSON処理に失敗しました: {source}")]
    StatusJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// JSONのフィールド順も出力仕様の一部とするため、mapではなくstructで保持する。
#[derive(Debug, Serialize)]
struct SimulationReport<'a> {
    format_version: u8,
    seed: u64,
    requested_years: u64,
    config: &'a SimulationConfig,
    initial_state: StateMetadata,
    final_state: StateMetadata,
    yearly_statistics: &'a [YearStatistics],
    cumulative_statistics: &'a CumulativeStatistics,
    relationship_health: SimulationHealthMetrics,
    warnings: &'a [SimulationWarning],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct StateMetadata {
    year: u64,
    total_population: usize,
    total_unique_npcs: usize,
    town_populations: Vec<TownPopulation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct TownPopulation {
    id: u16,
    name: String,
    population: usize,
    capacity: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RunState {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
struct StatusReport {
    format_version: u8,
    state: RunState,
    pid: u32,
    continuous: bool,
    seed: u64,
    started_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
    uptime_seconds: u64,
    year: u64,
    years_completed: u64,
    target_years: Option<u64>,
    population: usize,
    total_unique_npcs: usize,
    town_populations: Vec<usize>,
    last_year_statistics: Option<YearStatistics>,
    relationship_health: SimulationHealthMetrics,
    warnings: Vec<SimulationWarning>,
    error: Option<String>,
}

struct StatusContext {
    path: PathBuf,
    started_at_unix_seconds: u64,
    started_at: Instant,
    initial_year: u64,
    target_years: Option<u64>,
    seed: u64,
}

impl StatusContext {
    fn new(path: PathBuf, initial_year: u64, target_years: Option<u64>, seed: u64) -> Self {
        Self {
            path,
            started_at_unix_seconds: unix_seconds(),
            started_at: Instant::now(),
            initial_year,
            target_years,
            seed,
        }
    }

    fn report(
        &self,
        simulation: &Simulation,
        state: RunState,
        error: Option<String>,
    ) -> StatusReport {
        let health = simulation.health_metrics();
        StatusReport {
            format_version: 1,
            state,
            pid: std::process::id(),
            continuous: self.target_years.is_none(),
            seed: self.seed,
            started_at_unix_seconds: self.started_at_unix_seconds,
            updated_at_unix_seconds: unix_seconds(),
            uptime_seconds: self.started_at.elapsed().as_secs(),
            year: simulation.world.year,
            years_completed: simulation.world.year.saturating_sub(self.initial_year),
            target_years: self.target_years,
            population: simulation.world.active_population(),
            total_unique_npcs: simulation.world.total_unique_npcs(),
            town_populations: simulation.world.town_populations(),
            last_year_statistics: simulation.world.statistics.latest().cloned(),
            relationship_health: health,
            warnings: simulation.world.statistics.detect_warnings(health),
            error,
        }
    }

    fn write(
        &self,
        simulation: &Simulation,
        state: RunState,
        error: Option<String>,
    ) -> Result<(), AppError> {
        write_status(&self.path, &self.report(simulation, state, error))
    }
}

/// 前回表示したcheckpointから今回までの値を保持する。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PeriodSummary {
    births: usize,
    deaths: usize,
    external_immigration: usize,
    external_emigration: usize,
    internal_migrations: usize,
    partnerships: usize,
    belief_changes: usize,
    goal_changes: usize,
}

impl PeriodSummary {
    fn add_year(&mut self, year: &YearStatistics) {
        self.births = self.births.saturating_add(year.births);
        self.deaths = self.deaths.saturating_add(year.deaths);
        self.external_immigration = self
            .external_immigration
            .saturating_add(year.external_immigration);
        self.external_emigration = self
            .external_emigration
            .saturating_add(year.external_emigration);
        self.internal_migrations = self
            .internal_migrations
            .saturating_add(year.internal_migrations);
        self.partnerships = self.partnerships.saturating_add(year.partnerships);
        self.belief_changes = self.belief_changes.saturating_add(year.belief_changes);
        self.goal_changes = self.goal_changes.saturating_add(year.goal_changes);
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), AppError> {
    match cli.command {
        Command::Simulate(args) => simulate(args),
        Command::Status(args) => show_status(args),
    }
}

fn simulate(args: SimulateArgs) -> Result<(), AppError> {
    let target_years = args.years.map(NonZeroU64::get);
    let continuous = target_years.is_none();
    if continuous && args.output.is_some() {
        return Err(AppError::UnsupportedContinuousOption("--output"));
    }
    if continuous && !args.npc_ids.is_empty() {
        return Err(AppError::UnsupportedContinuousOption("--npc"));
    }
    if continuous && args.timeline_world {
        return Err(AppError::UnsupportedContinuousOption("--timeline-world"));
    }
    if continuous && !args.timeline_town_ids.is_empty() {
        return Err(AppError::UnsupportedContinuousOption("--timeline-town"));
    }
    if continuous && !args.timeline_npc_ids.is_empty() {
        return Err(AppError::UnsupportedContinuousOption("--timeline-npc"));
    }

    let town_count = usize::from(args.towns.get());
    let initial_population = args.population.get() as usize;
    let config = SimulationConfig::for_danger(args.world_danger);
    let mut simulation =
        Simulation::new(town_count, initial_population, args.seed, config.clone())?;
    if continuous {
        simulation.world.statistics.retain_only_latest_year();
    }
    simulation.world.capture_year_events = args.timeline_world
        || !args.timeline_town_ids.is_empty()
        || !args.timeline_npc_ids.is_empty();
    let initial_state = capture_state(&simulation.world);
    let mut timelines = TimelineCollector::new(
        &simulation.world,
        args.timeline_world,
        &args.timeline_town_ids,
        &args.timeline_npc_ids,
    );
    let status_path = args
        .status_file
        .clone()
        .or_else(|| continuous.then(|| PathBuf::from("npc-system-status.json")));
    let status = status_path
        .map(|path| StatusContext::new(path, simulation.world.year, target_years, args.seed));

    if let Some(status) = &status {
        status.write(&simulation, RunState::Running, None)?;
        println!("status file: {}", status.path.display());
    }
    if continuous {
        println!("無期限モードで実行します（停止するまで年次処理を継続します）");
    }

    if !args.summary_only {
        print_progress_header(&initial_state);
    }

    let mut period = PeriodSummary::default();
    let mut period_start_year = 1;
    let mut elapsed_years = 0u64;
    loop {
        if target_years.is_some_and(|target| elapsed_years >= target) {
            break;
        }
        elapsed_years = elapsed_years.saturating_add(1);
        // 参照を保持したまま次の年へ進めないよう、直後にcloneする。
        let year = match simulation.run_year() {
            Ok(year) => year.clone(),
            Err(error) => {
                if let Some(status) = &status {
                    let _ = status.write(&simulation, RunState::Failed, Some(error.to_string()));
                }
                return Err(error.into());
            }
        };
        timelines.record_year(&simulation.world, &year);
        period.add_year(&year);

        let reached_target = target_years.is_some_and(|target| elapsed_years == target);
        if elapsed_years % 10 == 0 || reached_target {
            if !args.summary_only {
                print_period_summary(period_start_year, &year, &period);
            }
            period_start_year = year.year.saturating_add(1);
            period = PeriodSummary::default();
        }
        if let Some(status) = &status {
            if elapsed_years % args.status_interval_years.get() == 0 || reached_target {
                status.write(&simulation, RunState::Running, None)?;
            }
        }
    }

    let cumulative = simulation.world.statistics.cumulative();
    let health = simulation.health_metrics();
    let warnings = simulation.world.statistics.detect_warnings(health);
    let final_state = capture_state(&simulation.world);

    print_final_summary(&initial_state, &final_state, &cumulative, health, &warnings);
    print_requested_npcs(&simulation.world, &args.npc_ids);
    timelines.print(&simulation.world);

    if let Some(status) = &status {
        status.write(&simulation, RunState::Completed, None)?;
    }

    if let Some(path) = args.output.as_deref() {
        let report = SimulationReport {
            format_version: 1,
            seed: args.seed,
            requested_years: target_years.expect("output is unavailable in continuous mode"),
            config: &config,
            initial_state,
            final_state,
            yearly_statistics: &simulation.world.statistics.years,
            cumulative_statistics: &cumulative,
            relationship_health: health,
            warnings: &warnings,
        };
        write_report(path, &report)?;
        println!("\nJSON output: {}", path.display());
    }

    Ok(())
}

fn capture_state(world: &World) -> StateMetadata {
    let populations = world.town_populations();
    let town_populations = world
        .towns
        .iter()
        .map(|town| TownPopulation {
            id: town.id.0,
            name: town.name.clone(),
            population: populations
                .get(usize::from(town.id.0))
                .copied()
                .unwrap_or_default(),
            capacity: town.population_capacity,
        })
        .collect();

    StateMetadata {
        year: world.year,
        total_population: world.active_population(),
        total_unique_npcs: world.total_unique_npcs(),
        town_populations,
    }
}

fn print_progress_header(initial: &StateMetadata) {
    println!("=== シミュレーション進捗 ===");
    println!(
        "{:<13} {:>10} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "期間", "人口", "出生", "死亡", "流入", "流出", "都市移住", "提携"
    );
    println!(
        "{:<13} {:>10} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        format!("{}年", initial.year),
        format_number(initial.total_population),
        "-",
        "-",
        "-",
        "-",
        "-",
        "-"
    );
}

fn print_period_summary(start_year: u64, year: &YearStatistics, period: &PeriodSummary) {
    println!(
        "{:<13} {:>10} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        format!("{}-{}年", start_year, year.year),
        format_number(year.total_population),
        format_number(period.births),
        format_number(period.deaths),
        format_number(period.external_immigration),
        format_number(period.external_emigration),
        format_number(period.internal_migrations),
        format_number(period.partnerships),
    );
}

fn print_final_summary(
    initial: &StateMetadata,
    final_state: &StateMetadata,
    cumulative: &CumulativeStatistics,
    health: SimulationHealthMetrics,
    warnings: &[SimulationWarning],
) {
    let population_change = if initial.total_population == 0 {
        0.0
    } else {
        (final_state.total_population as f64 / initial.total_population as f64 - 1.0) * 100.0
    };

    println!("\n=== 統計サマリー ===");
    println!(
        "期間              : {}年",
        final_state.year.saturating_sub(initial.year)
    );
    println!(
        "人口              : {} → {} ({population_change:+.1}%)",
        format_number(initial.total_population),
        format_number(final_state.total_population)
    );
    println!(
        "登場した固有NPC   : {}",
        format_number(final_state.total_unique_npcs)
    );

    println!("\nイベント累計");
    println!("  出生            : {}", format_number(cumulative.births));
    println!("  死亡            : {}", format_number(cumulative.deaths));
    println!(
        "  外部流入 / 流出 : {} / {}",
        format_number(cumulative.external_immigration),
        format_number(cumulative.external_emigration)
    );
    println!(
        "  都市間移住      : {}",
        format_number(cumulative.internal_migrations)
    );
    println!(
        "  パートナー成立  : {}",
        format_number(cumulative.partnerships)
    );

    println!("\n死亡内訳");
    print_death_cause("自然死", cumulative.natural_deaths, cumulative.deaths);
    print_death_cause("自然災害", cumulative.disaster_deaths, cumulative.deaths);
    print_death_cause("疫病", cumulative.disease_deaths, cumulative.deaths);
    print_death_cause("戦争", cumulative.war_deaths, cumulative.deaths);
    print_death_cause("飢饉", cumulative.famine_deaths, cumulative.deaths);

    println!("\n社会状態");
    println!(
        "  平均関係数/NPC       : {:.2}",
        health.average_active_relationships
    );
    println!(
        "  平均強関係数/NPC     : {:.2}",
        health.average_strong_relationships
    );
    println!(
        "  極端な関係の割合     : {:.1}%",
        health.extreme_relationship_fraction * 100.0
    );
    println!(
        "  信念変更 / 目標変更  : {} / {}",
        format_number(cumulative.belief_changes),
        format_number(cumulative.goal_changes)
    );

    println!("\n都市人口");
    println!(
        "  {:<4} {:<12} {:>10} {:>10} {:>9}",
        "ID", "都市", "人口", "収容力", "使用率"
    );
    for town in &final_state.town_populations {
        let occupancy = town.population as f64 / f64::from(town.capacity) * 100.0;
        println!(
            "  {:<4} {:<12} {:>10} {:>10} {:>8.1}%",
            town.id,
            town.name,
            format_number(town.population),
            format_number(town.capacity as usize),
            occupancy
        );
    }

    if warnings.is_empty() {
        println!("\n警告: なし");
    } else {
        println!("\n警告");
        for warning in warnings {
            println!("  - {}", warning_label(warning));
        }
    }
}

fn print_death_cause(label: &str, count: usize, total: usize) {
    let share = if total == 0 {
        0.0
    } else {
        count as f64 / total as f64 * 100.0
    };
    println!("  - {label}: {} ({share:.1}%)", format_number(count));
}

fn warning_label(warning: &SimulationWarning) -> String {
    match warning {
        SimulationWarning::RelationshipPolarization { fraction } => format!(
            "関係値が0または10の関係が全体の{:.0}%を占めています",
            fraction * 100.0
        ),
        SimulationWarning::DenseRelationshipGraph { average } => {
            format!("NPCあたりの平均関係数が{average:.1}件で過密です")
        }
        SimulationWarning::TownConcentration { share } => {
            format!("最大都市に全人口の{:.0}%が集中しています", share * 100.0)
        }
        SimulationWarning::NoMigration => "都市間移住が発生していません".to_owned(),
        SimulationWarning::NoGoalChanges => "目標変更が発生していません".to_owned(),
        SimulationWarning::FrequentGoalChanges {
            average_per_npc_year,
        } => format!("NPCあたりの目標変更が年平均{average_per_npc_year:.2}回で頻繁です"),
        SimulationWarning::PopulationExplosion { factor } => {
            format!("人口が初期値の{factor:.1}倍に増加しています")
        }
        SimulationWarning::PopulationCollapse { decline } => {
            format!("人口が初期値から{:.0}%減少しています", decline * 100.0)
        }
    }
}

fn print_requested_npcs(world: &World, requested_ids: &[u32]) {
    if requested_ids.is_empty() {
        return;
    }

    println!("\n=== NPC詳細 ===");
    let mut displayed = BTreeSet::new();
    for &raw_id in requested_ids {
        if !displayed.insert(raw_id) {
            continue;
        }
        let id = NpcId(raw_id);
        match world.npc(id) {
            Some(npc) => print_npc_details(world, npc),
            None => println!(
                "\nNPC #{raw_id}: 見つかりません（存在するID: 0..={}）",
                world.npcs.len().saturating_sub(1)
            ),
        }
    }
}

fn print_npc_details(world: &World, npc: &Npc) {
    println!("\n--- {} [ID: {}] ---", npc.name, npc.id.0);
    println!("状態       : {}", npc_status(npc));
    println!("年齢       : {}", npc_age(npc));
    println!("性別       : {}", sex_label(npc.sex));
    println!("出生地     : {}", town_name(world, npc.hometown));
    println!("最終居住地 : {}", town_name(world, npc.town));
    println!("現在状態   : {}", state_label(npc.state));
    println!(
        "能力       : 身体 {}/10 | 器用 {}/10 | 知性 {}/10 | 魅力 {}/10 | 意志 {}/10",
        npc.attributes.physical,
        npc.attributes.dexterity,
        npc.attributes.intelligence,
        npc.attributes.charisma,
        npc.attributes.willpower
    );
    println!(
        "目標       : {}（進捗 {:.0}%、{}年から）",
        goal_label(npc.goal.kind),
        npc.goal.progress * 100.0,
        npc.goal.since_year
    );

    println!("信念");
    if npc.beliefs.is_empty() {
        println!("  - なし");
    } else {
        for belief in &npc.beliefs {
            println!("  - {}: {}/10", belief_label(belief.kind), belief.strength);
        }
    }

    let strong_relationships = npc
        .relationships
        .values()
        .filter(|relationship| relationship.is_strong())
        .count();
    println!(
        "関係       : {}件（強い関係 {}件）",
        npc.relationships.len(),
        strong_relationships
    );

    println!("家族");
    print_relative_group(world, "パートナー", npc.partner.into_iter().collect());
    print_relative_group(world, "親", npc.parents.clone());
    print_relative_group(world, "祖父母", grandparents(world, npc));
    print_relative_group(world, "兄弟姉妹", siblings(world, npc));
    print_relative_group(world, "子", npc.children.clone());
    print_relative_group(world, "孫", grandchildren(world, npc));
}

fn print_relative_group(world: &World, label: &str, mut ids: Vec<NpcId>) {
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        println!("  {label}: なし");
        return;
    }

    println!("  {label}:");
    for id in ids {
        if let Some(relative) = world.npc(id) {
            println!(
                "    - {} [ID: {}] / {} / {} / {}",
                relative.name,
                relative.id.0,
                sex_label(relative.sex),
                npc_age(relative),
                npc_status(relative)
            );
        }
    }
}

fn grandparents(world: &World, npc: &Npc) -> Vec<NpcId> {
    npc.parents
        .iter()
        .filter_map(|&id| world.npc(id))
        .flat_map(|parent| parent.parents.iter().copied())
        .collect()
}

fn siblings(world: &World, npc: &Npc) -> Vec<NpcId> {
    npc.parents
        .iter()
        .filter_map(|&id| world.npc(id))
        .flat_map(|parent| parent.children.iter().copied())
        .filter(|&id| id != npc.id)
        .collect()
}

fn grandchildren(world: &World, npc: &Npc) -> Vec<NpcId> {
    npc.children
        .iter()
        .filter_map(|&id| world.npc(id))
        .flat_map(|child| child.children.iter().copied())
        .collect()
}

fn town_name(world: &World, id: npc_system::id::TownId) -> &str {
    world.town(id).map_or("不明", |town| town.name.as_str())
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

fn sex_label(sex: Sex) -> &'static str {
    match sex {
        Sex::Male => "男性",
        Sex::Female => "女性",
    }
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

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn write_status(path: &Path, report: &StatusReport) -> Result<(), AppError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("npc-system-status.json");
    let temporary = path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()));
    let file = File::create(&temporary).map_err(|source| AppError::StatusIo {
        path: path.to_path_buf(),
        source,
    })?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, report).map_err(|source| AppError::StatusJson {
        path: path.to_path_buf(),
        source,
    })?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|source| AppError::StatusIo {
            path: path.to_path_buf(),
            source,
        })?;
    fs::rename(&temporary, path).map_err(|source| AppError::StatusIo {
        path: path.to_path_buf(),
        source,
    })
}

fn read_status(path: &Path) -> Result<StatusReport, AppError> {
    let file = File::open(path).map_err(|source| AppError::StatusIo {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_reader(file).map_err(|source| AppError::StatusJson {
        path: path.to_path_buf(),
        source,
    })
}

fn show_status(args: StatusArgs) -> Result<(), AppError> {
    loop {
        let report = read_status(&args.file)?;
        if args.watch {
            print!("\x1b[2J\x1b[H");
        }
        print_status(&args.file, &report, args.stale_after_seconds);
        io::stdout().flush().map_err(|source| AppError::StatusIo {
            path: args.file.clone(),
            source,
        })?;
        if !args.watch {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(args.interval_seconds.get()));
    }
}

fn print_status(path: &Path, report: &StatusReport, stale_after_seconds: u64) {
    let age = unix_seconds().saturating_sub(report.updated_at_unix_seconds);
    let stale = report.state == RunState::Running && age > stale_after_seconds;
    let state = if stale {
        "stale（更新停止）"
    } else {
        match report.state {
            RunState::Running => "running（実行中）",
            RunState::Completed => "completed（完了）",
            RunState::Failed => "failed（失敗）",
        }
    };
    println!("=== npc-system status ===");
    println!("ファイル       : {}", path.display());
    println!("状態           : {state}");
    println!("PID            : {}", report.pid);
    println!(
        "モード         : {}",
        if report.continuous {
            "無期限"
        } else {
            "期間指定"
        }
    );
    println!("現在年         : {}", report.year);
    match report.target_years {
        Some(target) => println!("進捗           : {} / {}年", report.years_completed, target),
        None => println!("経過           : {}年", report.years_completed),
    }
    println!("人口           : {}", format_number(report.population));
    println!(
        "登場した固有NPC: {}",
        format_number(report.total_unique_npcs)
    );
    println!("稼働時間       : {}秒", report.uptime_seconds);
    println!("最終更新       : {age}秒前");
    println!("警告数         : {}", report.warnings.len());
    if let Some(error) = &report.error {
        println!("エラー         : {error}");
    }
}

fn write_report(path: &Path, report: &SimulationReport<'_>) -> Result<(), AppError> {
    let file = File::create(path).map_err(|source| AppError::CreateOutput {
        path: path.to_path_buf(),
        source,
    })?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, report).map_err(|source| {
        AppError::SerializeOutput {
            path: path.to_path_buf(),
            source,
        }
    })?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|source| AppError::WriteOutput {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn parses_documented_simulate_command() {
        let cli = Cli::try_parse_from([
            "npc-system",
            "simulate",
            "--towns",
            "20",
            "--population",
            "5000",
            "--years",
            "100",
            "--seed",
            "12345",
            "--world-danger",
            "harsh",
            "--output",
            "result.json",
            "--npc",
            "3185",
            "--npc",
            "11023",
            "--summary-only",
            "--timeline-world",
            "--timeline-town",
            "5",
            "--timeline-town",
            "12",
            "--timeline-npc",
            "3185",
            "--timeline-npc",
            "11023",
        ])
        .unwrap();

        let Command::Simulate(args) = cli.command else {
            panic!("simulate subcommand was expected");
        };
        assert_eq!(args.towns.get(), 20);
        assert_eq!(args.population.get(), 5_000);
        assert_eq!(args.years.map(NonZeroU64::get), Some(100));
        assert_eq!(args.seed, 12_345);
        assert_eq!(args.world_danger, WorldDanger::Harsh);
        assert_eq!(args.output, Some(PathBuf::from("result.json")));
        assert_eq!(args.npc_ids, vec![3_185, 11_023]);
        assert!(args.summary_only);
        assert!(args.timeline_world);
        assert_eq!(args.timeline_town_ids, vec![5, 12]);
        assert_eq!(args.timeline_npc_ids, vec![3_185, 11_023]);
    }

    #[test]
    fn rejects_zero_sized_simulation() {
        let error = Cli::try_parse_from([
            "npc-system",
            "simulate",
            "--towns",
            "0",
            "--population",
            "5000",
            "--years",
            "100",
            "--seed",
            "1",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    #[test]
    fn omitting_years_selects_continuous_mode() {
        let cli = Cli::try_parse_from([
            "npc-system",
            "simulate",
            "--towns",
            "2",
            "--population",
            "20",
            "--seed",
            "7",
        ])
        .unwrap();
        let Command::Simulate(args) = cli.command else {
            panic!("simulate subcommand was expected");
        };

        assert!(args.years.is_none());
        assert!(args.status_file.is_none());
        assert_eq!(args.status_interval_years.get(), 1);
    }

    #[test]
    fn status_file_round_trip_is_atomic_and_readable() {
        let path = std::env::temp_dir().join(format!(
            "npc-system-status-test-{}-{}.json",
            std::process::id(),
            unix_seconds()
        ));
        let simulation =
            Simulation::new(1, 10, 7, SimulationConfig::normal()).expect("valid simulation");
        let context = StatusContext::new(path.clone(), 0, None, 7);
        context
            .write(&simulation, RunState::Running, None)
            .expect("status is writable");

        let status = read_status(&path).expect("status is readable");
        assert_eq!(status.state, RunState::Running);
        assert!(status.continuous);
        assert_eq!(status.population, 10);
        fs::remove_file(path).expect("test status can be removed");
    }

    #[test]
    fn period_summary_accumulates_checkpoint_years() {
        let mut first = YearStatistics::new(1, 101, vec![101]);
        first.births = 3;
        first.deaths = 2;
        first.internal_migrations = 4;
        let mut second = YearStatistics::new(2, 102, vec![102]);
        second.births = 5;
        second.deaths = 1;
        second.internal_migrations = 6;

        let mut summary = PeriodSummary::default();
        summary.add_year(&first);
        summary.add_year(&second);

        assert_eq!(summary.births, 8);
        assert_eq!(summary.deaths, 3);
        assert_eq!(summary.internal_migrations, 10);
    }

    #[test]
    fn formats_counts_with_group_separators() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1,000");
        assert_eq!(format_number(12_345_678), "12,345,678");
    }
}
