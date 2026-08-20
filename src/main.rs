use clap::{Args, Parser, Subcommand};
use npc_system::statistics::{
    CumulativeStatistics, SimulationHealthMetrics, SimulationWarning, YearStatistics,
};
use npc_system::{Simulation, SimulationConfig, SimulationError, World, WorldDanger};
use serde::Serialize;
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

    print_initial_summary(&initial_state);

    let mut period = PeriodSummary::default();
    for elapsed_years in 1..=requested_years {
        // 参照を保持したまま次の年へ進めないよう、直後にcloneする。
        let year = simulation.run_year()?.clone();
        period.add_year(&year);

        if elapsed_years % 10 == 0 || elapsed_years == requested_years {
            print_period_summary(&year, &period);
            period = PeriodSummary::default();
        }
    }

    let cumulative = simulation.world.statistics.cumulative();
    let health = simulation.health_metrics();
    let warnings = simulation.world.statistics.detect_warnings(health);
    let final_state = capture_state(&simulation.world);

    print_final_summary(&initial_state, &final_state, &cumulative, health, &warnings);

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

fn print_initial_summary(initial: &StateMetadata) {
    println!("Year {}", initial.year);
    println!("Population: {}", initial.total_population);
}

fn print_period_summary(year: &YearStatistics, period: &PeriodSummary) {
    println!("\nYear {}", year.year);
    println!("Population: {}", year.total_population);
    println!("Births: {}", period.births);
    println!("Deaths: {}", period.deaths);
    println!("External immigration: {}", period.external_immigration);
    println!("External emigration: {}", period.external_emigration);
    println!("Internal migrations: {}", period.internal_migrations);
    println!("Partnerships: {}", period.partnerships);
    println!("Belief changes: {}", period.belief_changes);
    println!("Goal changes: {}", period.goal_changes);
}

fn print_final_summary(
    initial: &StateMetadata,
    final_state: &StateMetadata,
    cumulative: &CumulativeStatistics,
    health: SimulationHealthMetrics,
    warnings: &[SimulationWarning],
) {
    println!("\nSimulation finished");
    println!("\nInitial population: {}", initial.total_population);
    println!("Final population: {}", final_state.total_population);
    println!("Unique NPCs: {}", final_state.total_unique_npcs);
    println!("\nBirths: {}", cumulative.births);
    println!("Deaths: {}", cumulative.deaths);
    println!("Immigration: {}", cumulative.external_immigration);
    println!("Emigration: {}", cumulative.external_emigration);
    println!("Internal migrations: {}", cumulative.internal_migrations);
    println!("Partnerships: {}", cumulative.partnerships);
    println!("\nNatural deaths: {}", cumulative.natural_deaths);
    println!("Natural disaster deaths: {}", cumulative.disaster_deaths);
    println!("Disease deaths: {}", cumulative.disease_deaths);
    println!("War deaths: {}", cumulative.war_deaths);
    println!("Famine deaths: {}", cumulative.famine_deaths);
    println!(
        "\nAverage relationships/NPC: {:.2}",
        health.average_active_relationships
    );
    println!(
        "Average strong relationships/NPC: {:.2}",
        health.average_strong_relationships
    );
    println!(
        "Extreme relationships (0 or 10): {:.1}%",
        health.extreme_relationship_fraction * 100.0
    );
    println!("Belief changes: {}", cumulative.belief_changes);
    println!("Goal changes: {}", cumulative.goal_changes);
    println!("\nTown populations:");
    for town in &final_state.town_populations {
        println!("  {}: {}", town.name, town.population);
    }

    if warnings.is_empty() {
        println!("\nWarnings: none");
    } else {
        println!("\nWarnings:");
        for warning in warnings {
            println!("  {warning}");
        }
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
        ])
        .unwrap();

        let Command::Simulate(args) = cli.command;
        assert_eq!(args.towns.get(), 20);
        assert_eq!(args.population.get(), 5_000);
        assert_eq!(args.years.get(), 100);
        assert_eq!(args.seed, 12_345);
        assert_eq!(args.world_danger, WorldDanger::Harsh);
        assert_eq!(args.output, Some(PathBuf::from("result.json")));
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
}
