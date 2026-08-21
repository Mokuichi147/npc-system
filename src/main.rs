use clap::{Args, Parser, Subcommand};
use npc_system::belief::BeliefKind;
use npc_system::goal::GoalKind;
use npc_system::id::NpcId;
use npc_system::npc::{Npc, NpcState, Sex};
use npc_system::statistics::{
    CumulativeStatistics, SimulationHealthMetrics, SimulationWarning, YearStatistics,
};
use npc_system::{Simulation, SimulationConfig, SimulationError, World, WorldDanger};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
}

#[derive(Debug, Args)]
struct SimulateArgs {
    /// 生成する都市数（1以上）。
    #[arg(long)]
    towns: NonZeroU16,

    /// 初期NPC数（1以上）。
    #[arg(long)]
    population: NonZeroU32,

    /// シミュレーションする年数（1以上）。
    #[arg(long)]
    years: NonZeroU16,

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
}

/// JSONのフィールド順も出力仕様の一部とするため、mapではなくstructで保持する。
#[derive(Debug, Serialize)]
struct SimulationReport<'a> {
    format_version: u8,
    seed: u64,
    requested_years: u16,
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
    year: u16,
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
    }
}

fn simulate(args: SimulateArgs) -> Result<(), AppError> {
    let town_count = usize::from(args.towns.get());
    let initial_population = args.population.get() as usize;
    let requested_years = args.years.get();
    let config = SimulationConfig::for_danger(args.world_danger);
    let mut simulation =
        Simulation::new(town_count, initial_population, args.seed, config.clone())?;
    let initial_state = capture_state(&simulation.world);

    if !args.summary_only {
        print_progress_header(&initial_state);
    }

    let mut period = PeriodSummary::default();
    let mut period_start_year = 1;
    for elapsed_years in 1..=requested_years {
        // 参照を保持したまま次の年へ進めないよう、直後にcloneする。
        let year = simulation.run_year()?.clone();
        period.add_year(&year);

        if elapsed_years % 10 == 0 || elapsed_years == requested_years {
            if !args.summary_only {
                print_period_summary(period_start_year, &year, &period);
            }
            period_start_year = year.year.saturating_add(1);
            period = PeriodSummary::default();
        }
    }

    let cumulative = simulation.world.statistics.cumulative();
    let health = simulation.health_metrics();
    let warnings = simulation.world.statistics.detect_warnings(health);
    let final_state = capture_state(&simulation.world);

    print_final_summary(&initial_state, &final_state, &cumulative, health, &warnings);
    print_requested_npcs(&simulation.world, &args.npc_ids);

    if let Some(path) = args.output.as_deref() {
        let report = SimulationReport {
            format_version: 1,
            seed: args.seed,
            requested_years,
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

fn print_period_summary(start_year: u16, year: &YearStatistics, period: &PeriodSummary) {
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
        ])
        .unwrap();

        let Command::Simulate(args) = cli.command;
        assert_eq!(args.towns.get(), 20);
        assert_eq!(args.population.get(), 5_000);
        assert_eq!(args.years.get(), 100);
        assert_eq!(args.seed, 12_345);
        assert_eq!(args.world_danger, WorldDanger::Harsh);
        assert_eq!(args.output, Some(PathBuf::from("result.json")));
        assert_eq!(args.npc_ids, vec![3_185, 11_023]);
        assert!(args.summary_only);
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
