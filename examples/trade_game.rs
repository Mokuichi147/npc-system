use std::io::{self, Stdout};
use std::num::{NonZeroU16, NonZeroU32};
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use npc_system::economy::{Good, Money, TownEconomicStatistics};
use npc_system::id::TownId;
use npc_system::trade_game::PlayerAccount;
use npc_system::{Simulation, SimulationConfig};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, Borders, Cell, Chart, Clear, Dataset, Gauge, GraphType, HighlightSpacing,
    Paragraph, Row, Table, TableState, Wrap,
};

type Tui = Terminal<CrosstermBackend<Stdout>>;

const GOODS: [Good; 5] = Good::ALL;
const MIN_WIDTH: u16 = 72;
const MIN_HEIGHT: u16 = 20;
const FULL_WIDTH: u16 = 100;
const FULL_HEIGHT: u16 = 31;

#[derive(Debug, Parser)]
#[command(
    name = "trade-game",
    about = "都市経済を観察しながら商品を売買するnpc-system TUIゲーム"
)]
struct Args {
    /// 都市数。
    #[arg(long, default_value = "5")]
    towns: NonZeroU16,
    /// NPC初期人口。
    #[arg(long, default_value = "500")]
    population: NonZeroU32,
    /// 売買する年数。
    #[arg(long, default_value = "10")]
    years: NonZeroU16,
    /// ゲーム開始前にNPC経済だけを進める年数。
    #[arg(long, default_value = "10")]
    warmup_years: u16,
    /// 再現可能な乱数seed。
    #[arg(long, default_value = "12345")]
    seed: u64,
    /// 取引市場とする都市ID。
    #[arg(long, default_value = "0")]
    town: u16,
    /// 開始時の所持金（通貨単位。小数不可）。
    #[arg(long, default_value = "1000")]
    starting_money: u64,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    args.warmup_years
        .checked_add(args.years.get())
        .ok_or_else(|| "事前進行年数とプレイ年数の合計が大きすぎます".to_owned())?;
    let starting_cash_cents = args
        .starting_money
        .checked_mul(100)
        .ok_or_else(|| "開始所持金が大きすぎます".to_owned())?;
    let mut simulation = Simulation::new(
        usize::from(args.towns.get()),
        args.population.get() as usize,
        args.seed,
        SimulationConfig::default(),
    )
    .map_err(|error| error.to_string())?;
    simulation
        .run(args.warmup_years)
        .map_err(|error| error.to_string())?;
    let mut app = App::new(
        simulation,
        TownId(args.town),
        args.years.get(),
        starting_cash_cents,
    )?;
    let mut terminal = init_terminal().map_err(|error| error.to_string())?;
    let game_result = run_game(&mut terminal, &mut app);
    let restore_result = restore_terminal(&mut terminal);
    game_result
        .and(restore_result)
        .map_err(|error| error.to_string())
}

fn init_terminal() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error);
    }
    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            let mut stdout = io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            Err(error)
        }
    }
}

fn restore_terminal(terminal: &mut Tui) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

fn run_game(terminal: &mut Tui, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|frame| render(frame, app))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && app.handle_key(key)
        {
            return Ok(());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Market,
    History,
    Help,
    Finished,
    ConfirmQuit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MessageKind {
    Info,
    Success,
    Error,
}

#[derive(Clone, Debug)]
struct StatusMessage {
    text: String,
    kind: MessageKind,
}

struct App {
    simulation: Simulation,
    player: PlayerAccount,
    town_id: TownId,
    total_years: u16,
    completed_years: u16,
    selected_good: usize,
    quantity: u32,
    screen: Screen,
    history_offset: usize,
    message: StatusMessage,
    final_inventory_value: Money,
    final_cash: Option<Money>,
}

impl App {
    fn new(
        simulation: Simulation,
        town_id: TownId,
        total_years: u16,
        starting_cash_cents: Money,
    ) -> Result<Self, String> {
        if simulation.world.town(town_id).is_none() {
            return Err(format!("都市ID {} は存在しません", town_id.0));
        }
        let initial_world_year = simulation.world.year;
        Ok(Self {
            simulation,
            player: PlayerAccount::new(starting_cash_cents),
            town_id,
            total_years,
            completed_years: 0,
            selected_good: 0,
            quantity: 1,
            screen: Screen::Market,
            history_offset: 0,
            message: StatusMessage {
                text: if initial_world_year == 0 {
                    "商品を選び、購入または売却してから翌年へ進んでください".to_owned()
                } else {
                    format!(
                        "NPC経済を{}年間進行済みです。履歴を判断材料に売買してください",
                        initial_world_year
                    )
                },
                kind: MessageKind::Info,
            },
            final_inventory_value: 0,
            final_cash: None,
        })
    }

    fn selected_good(&self) -> Good {
        GOODS[self.selected_good]
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        match self.screen {
            Screen::Finished => return matches!(key.code, KeyCode::Enter | KeyCode::Char('q')),
            Screen::ConfirmQuit => match key.code {
                KeyCode::Char('q') | KeyCode::Enter => return true,
                KeyCode::Esc | KeyCode::Char('n') => self.screen = Screen::Market,
                _ => {}
            },
            Screen::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('h') => {
                    self.screen = Screen::Market;
                }
                KeyCode::Char('q') => self.screen = Screen::ConfirmQuit,
                _ => {}
            },
            Screen::History => match key.code {
                KeyCode::Esc | KeyCode::Char('h') => self.screen = Screen::Market,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.history_offset = self.history_offset.saturating_add(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.history_offset = self.history_offset.saturating_sub(1);
                }
                KeyCode::Home => self.history_offset = usize::MAX,
                KeyCode::End => self.history_offset = 0,
                KeyCode::Char('q') => self.screen = Screen::ConfirmQuit,
                _ => {}
            },
            Screen::Market => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.selected_good = self.selected_good.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected_good = (self.selected_good + 1).min(GOODS.len() - 1);
                }
                KeyCode::Char(value @ '1'..='5') => {
                    self.selected_good = usize::from(value as u8 - b'1');
                }
                KeyCode::Left | KeyCode::Char('-') => self.change_quantity(false, key.modifiers),
                KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') => {
                    self.change_quantity(true, key.modifiers);
                }
                KeyCode::Char('b') => self.buy(),
                KeyCode::Char('s') => self.sell(),
                KeyCode::Char('n') | KeyCode::Enter => self.advance_year(),
                KeyCode::Char('h') => {
                    self.history_offset = 0;
                    self.screen = Screen::History;
                }
                KeyCode::Char('?') => self.screen = Screen::Help,
                KeyCode::Char('q') | KeyCode::Esc => self.screen = Screen::ConfirmQuit,
                _ => {}
            },
        }
        false
    }

    fn change_quantity(&mut self, increase: bool, modifiers: KeyModifiers) {
        let step = if modifiers.contains(KeyModifiers::SHIFT) {
            10
        } else {
            1
        };
        self.quantity = if increase {
            self.quantity.saturating_add(step)
        } else {
            self.quantity.saturating_sub(step).max(1)
        };
    }

    fn buy(&mut self) {
        let good = self.selected_good();
        match self.player.buy(
            &mut self.simulation.world,
            self.town_id,
            good,
            self.quantity,
        ) {
            Ok(receipt) => {
                self.message = StatusMessage {
                    text: format!(
                        "{}を{}個購入しました（総額 {}）",
                        good_label(good),
                        receipt.quantity,
                        format_money(receipt.total_cents)
                    ),
                    kind: MessageKind::Success,
                };
            }
            Err(error) => self.set_error(error.to_string()),
        }
    }

    fn sell(&mut self) {
        let good = self.selected_good();
        match self.player.sell(
            &mut self.simulation.world,
            self.town_id,
            good,
            self.quantity,
        ) {
            Ok(receipt) => {
                self.message = StatusMessage {
                    text: format!(
                        "{}を{}個売却しました（総額 {}）",
                        good_label(good),
                        receipt.quantity,
                        format_money(receipt.total_cents)
                    ),
                    kind: MessageKind::Success,
                };
            }
            Err(error) => self.set_error(error.to_string()),
        }
    }

    fn advance_year(&mut self) {
        match self.simulation.run_year() {
            Ok(year) => {
                self.completed_years = self.completed_years.saturating_add(1);
                self.message = StatusMessage {
                    text: format!(
                        "{}年が終了しました。価格と経済指標が更新されています",
                        year.year
                    ),
                    kind: MessageKind::Info,
                };
                if self.completed_years >= self.total_years {
                    self.finish();
                }
            }
            Err(error) => self.set_error(error.to_string()),
        }
    }

    fn finish(&mut self) {
        match self
            .player
            .inventory_market_value(&self.simulation.world, self.town_id)
            .and_then(|value| {
                self.final_inventory_value = value;
                self.player
                    .settle_at_market_value(&self.simulation.world, self.town_id)
            }) {
            Ok(final_cash) => {
                self.final_cash = Some(final_cash);
                self.screen = Screen::Finished;
            }
            Err(error) => self.set_error(error.to_string()),
        }
    }

    fn set_error(&mut self, text: String) {
        self.message = StatusMessage {
            text,
            kind: MessageKind::Error,
        };
    }

    fn town_economy(&self) -> Option<TownEconomicStatistics> {
        self.simulation
            .world
            .town_economic_statistics()
            .into_iter()
            .find(|economy| economy.town == self.town_id)
    }
}

fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_too_small(frame, area);
        return;
    }
    if area.width < FULL_WIDTH || area.height < FULL_HEIGHT {
        render_compact(frame, app, area);
    } else {
        render_full(frame, app, area);
    }

    match app.screen {
        Screen::History => render_history_overlay(frame, app),
        Screen::Help => render_help_overlay(frame),
        Screen::Finished => render_finished_overlay(frame, app),
        Screen::ConfirmQuit => render_quit_overlay(frame),
        Screen::Market => {}
    }
}

fn render_full(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(10),
            Constraint::Length(9),
            Constraint::Length(4),
        ])
        .split(area);

    render_header(frame, app, sections[0]);
    render_kpis(frame, app, sections[1]);
    render_market_and_portfolio(frame, app, sections[2]);
    render_trends(frame, app, sections[3]);
    render_footer(frame, app, sections[4]);
}

fn render_compact(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(9),
            Constraint::Length(4),
        ])
        .split(area);
    render_header(frame, app, sections[0]);
    render_kpis(frame, app, sections[1]);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(sections[2]);
    render_market_compact(frame, app, columns[0]);
    render_portfolio_compact(frame, app, columns[1]);
    render_footer(frame, app, sections[3]);
}

fn render_too_small(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let message = format!(
        "画面が小さすぎます\n\n現在: {}×{} / 必要: {}×{}以上",
        area.width, area.height, MIN_WIDTH, MIN_HEIGHT
    );
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" trade_game ")),
        area,
    );
}

fn render_header(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let town_name = &app
        .simulation
        .world
        .town(app.town_id)
        .expect("town existence is validated")
        .name;
    let progress = f64::from(app.completed_years) / f64::from(app.total_years);
    let title = format!(
        " npc-system 経済取引ゲーム  │  {}  │  世界 {}年  │  プレイ {} / {}年 ",
        town_name, app.simulation.world.year, app.completed_years, app.total_years
    );
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(title))
            .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray))
            .ratio(progress)
            .label(format!("進行度 {:.0}%", progress * 100.0)),
        area,
    );
}

fn render_kpis(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25); 4])
        .split(area);
    let inventory_value = app
        .player
        .inventory_market_value(&app.simulation.world, app.town_id)
        .unwrap_or_default();
    let economy = app.town_economy();
    let total_assets = app.player.cash_cents.saturating_add(inventory_value);
    let values = [
        (" 現金 ", format_money(app.player.cash_cents), Color::Green),
        (" 総資産 ", format_money(total_assets), Color::Yellow),
        (
            " 都市経済力 ",
            economy.as_ref().map_or_else(
                || "-".to_owned(),
                |value| format_money(value.economic_power_cents),
            ),
            Color::Cyan,
        ),
        (
            " 物価 / インフレ率 ",
            economy.as_ref().map_or_else(
                || "-".to_owned(),
                |value| {
                    format!(
                        "{:.2} / {:+.2}%",
                        value.price_index as f64 / 10_000.0,
                        value.inflation_basis_points as f64 / 100.0
                    )
                },
            ),
            inflation_color(
                economy
                    .as_ref()
                    .map_or(0, |value| value.inflation_basis_points),
            ),
        ),
    ];
    for (column, (title, value, color)) in columns.iter().zip(values) {
        frame.render_widget(
            Paragraph::new(value)
                .alignment(Alignment::Center)
                .style(Style::default().fg(color).add_modifier(Modifier::BOLD))
                .block(Block::default().borders(Borders::ALL).title(title)),
            *column,
        );
    }
}

fn render_market_and_portfolio(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(64), Constraint::Percentage(36)])
        .split(area);
    render_market(frame, app, columns[0]);
    render_portfolio(frame, app, columns[1]);
}

fn render_market(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let economy = app.town_economy();
    let rows = economy
        .as_ref()
        .map(|economy| {
            economy
                .goods
                .iter()
                .map(|item| {
                    Row::new(vec![
                        Cell::from(good_label(item.good)),
                        Cell::from(format_money(item.unit_price_cents)),
                        Cell::from(format!(
                            "{:+.2}%",
                            item.inflation_basis_points as f64 / 100.0
                        ))
                        .style(Style::default().fg(inflation_color(item.inflation_basis_points))),
                        Cell::from(format!(
                            "{:+.2}%",
                            item.supply_shock_basis_points as f64 / 100.0
                        )),
                        Cell::from(format_u64(item.annual_quantity)),
                    ])
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let header = Row::new(["商品", "単価", "騰落率", "供給変動", "年間数量"])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);
    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Min(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" 商品市場  │  取引数量: {} ", app.quantity)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ")
    .highlight_spacing(HighlightSpacing::Always);
    let mut state = TableState::default().with_selected(Some(app.selected_good));
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_market_compact(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let economy = app.town_economy();
    let rows = economy
        .as_ref()
        .map(|economy| {
            economy
                .goods
                .iter()
                .map(|item| {
                    Row::new(vec![
                        Cell::from(good_label(item.good)),
                        Cell::from(format_money(item.unit_price_cents)),
                        Cell::from(format!(
                            "{:+.2}%",
                            item.inflation_basis_points as f64 / 100.0
                        ))
                        .style(Style::default().fg(inflation_color(item.inflation_basis_points))),
                    ])
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Length(14),
            Constraint::Min(10),
        ],
    )
    .header(
        Row::new(["商品", "単価", "騰落率"])
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" 市場  │  数量 {} ", app.quantity)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ")
    .highlight_spacing(HighlightSpacing::Always);
    let mut state = TableState::default().with_selected(Some(app.selected_good));
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_portfolio(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let town = app
        .simulation
        .world
        .town(app.town_id)
        .expect("town existence is validated");
    let rows = GOODS.into_iter().map(|good| {
        let quantity = app.player.inventory.quantity(good);
        let value = town
            .economy
            .good_price(good)
            .saturating_mul(Money::from(quantity));
        Row::new(vec![
            Cell::from(good_label(good)),
            Cell::from(format!("{quantity}個")),
            Cell::from(format_money(value)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Min(12),
        ],
    )
    .header(
        Row::new(["保有商品", "数量", "時価"])
            .style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .bottom_margin(1),
    )
    .block(Block::default().borders(Borders::ALL).title(format!(
        " ポートフォリオ  │  売買 {}回 ",
        app.player.trades.len()
    )));
    frame.render_widget(table, area);
}

fn render_portfolio_compact(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let town = app
        .simulation
        .world
        .town(app.town_id)
        .expect("town existence is validated");
    let rows = GOODS.into_iter().map(|good| {
        let quantity = app.player.inventory.quantity(good);
        let value = town
            .economy
            .good_price(good)
            .saturating_mul(Money::from(quantity));
        Row::new(vec![
            Cell::from(good_label(good)),
            Cell::from(quantity.to_string()),
            Cell::from(format_money(value)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Min(9),
        ],
    )
    .header(
        Row::new(["保有", "数", "時価"])
            .style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" 資産  │  {}取引 ", app.player.trades.len())),
    );
    frame.render_widget(table, area);
}

fn render_trends(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let good = app.selected_good();
    let price_points = history_points(app, |economy| {
        economy
            .goods
            .iter()
            .find(|item| item.good == good)
            .map_or(0, |item| item.unit_price_cents)
    });
    let economy_points = history_points(app, |economy| economy.economic_power_cents);
    render_chart(
        frame,
        columns[0],
        &format!(" {} 単価推移 ", good_label(good)),
        &price_points,
        Color::Yellow,
    );
    render_chart(
        frame,
        columns[1],
        " 都市経済力の推移 ",
        &economy_points,
        Color::Cyan,
    );
}

fn history_points(app: &App, value: impl Fn(&TownEconomicStatistics) -> Money) -> Vec<(f64, f64)> {
    app.simulation
        .world
        .statistics
        .years
        .iter()
        .filter_map(|year| {
            year.town_economies
                .iter()
                .find(|economy| economy.town == app.town_id)
                .map(|economy| (f64::from(year.year), value(economy) as f64 / 100.0))
        })
        .collect()
}

fn render_chart(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    points: &[(f64, f64)],
    color: Color,
) {
    if points.is_empty() {
        frame.render_widget(
            Paragraph::new("翌年へ進むと推移が表示されます")
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::ALL).title(title)),
            area,
        );
        return;
    }
    let min_x = points.first().map_or(0.0, |point| point.0);
    let mut max_x = points.last().map_or(1.0, |point| point.0);
    if (max_x - min_x).abs() < f64::EPSILON {
        max_x += 1.0;
    }
    let min_value = points
        .iter()
        .map(|point| point.1)
        .fold(f64::INFINITY, f64::min);
    let max_value = points
        .iter()
        .map(|point| point.1)
        .fold(f64::NEG_INFINITY, f64::max);
    let padding = ((max_value - min_value) * 0.15).max(1.0);
    let min_y = (min_value - padding).max(0.0);
    let max_y = max_value + padding;
    let datasets = vec![
        Dataset::default()
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(color))
            .data(points),
    ];
    let chart = Chart::new(datasets)
        .block(Block::default().borders(Borders::ALL).title(title))
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::Gray))
                .bounds([min_x, max_x])
                .labels([format!("{min_x:.0}年"), format!("{max_x:.0}年")]),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(Color::Gray))
                .bounds([min_y, max_y])
                .labels([format!("{min_y:.0}"), format!("{max_y:.0}")]),
        );
    frame.render_widget(chart, area);
}

fn render_footer(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let style = match app.message.kind {
        MessageKind::Info => Style::default().fg(Color::White),
        MessageKind::Success => Style::default().fg(Color::Green),
        MessageKind::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    };
    let help = Line::from(vec![
        Span::styled(" ↑↓ ", key_style()),
        Span::raw("商品  "),
        Span::styled("←→", key_style()),
        Span::raw(" 数量  "),
        Span::styled("B", key_style()),
        Span::raw(" 購入  "),
        Span::styled("S", key_style()),
        Span::raw(" 売却  "),
        Span::styled("N", key_style()),
        Span::raw(" 翌年  "),
        Span::styled("H", key_style()),
        Span::raw(" 履歴  "),
        Span::styled("?", key_style()),
        Span::raw(" ヘルプ  "),
        Span::styled("Q", key_style()),
        Span::raw(" 終了"),
    ]);
    frame.render_widget(
        Paragraph::new(vec![Line::styled(&app.message.text, style), help])
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_history_overlay(frame: &mut ratatui::Frame<'_>, app: &App) {
    if frame.area().width < FULL_WIDTH {
        render_history_overlay_compact(frame, app);
        return;
    }
    let area = centered_rect(90, 82, frame.area());
    frame.render_widget(Clear, area);
    let years = &app.simulation.world.statistics.years;
    let available_rows = usize::from(area.height.saturating_sub(5));
    let max_offset = years.len().saturating_sub(available_rows);
    let offset = app.history_offset.min(max_offset);
    let end = years.len().saturating_sub(offset);
    let start = end.saturating_sub(available_rows);
    let rows = years[start..end].iter().filter_map(|year| {
        let economy = year
            .town_economies
            .iter()
            .find(|economy| economy.town == app.town_id)?;
        Some(Row::new(vec![
            Cell::from(year.year.to_string()),
            Cell::from(format_money(economy.economic_power_cents)),
            Cell::from(format!("{:.2}", economy.price_index as f64 / 10_000.0)),
            Cell::from(format!(
                "{:+.2}%",
                economy.inflation_basis_points as f64 / 100.0
            )),
            Cell::from(history_price(economy, Good::Food)),
            Cell::from(history_price(economy, Good::Clothing)),
            Cell::from(history_price(economy, Good::Medicine)),
            Cell::from(history_price(economy, Good::Tools)),
            Cell::from(history_price(economy, Good::Luxury)),
        ]))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Length(15),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Min(10),
        ],
    )
    .header(
        Row::new([
            "年",
            "経済力",
            "物価",
            "変動",
            "食料",
            "衣料",
            "医薬品",
            "工具",
            "嗜好品",
        ])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" 全期間の経済履歴  │  ↑↓ スクロール  H/Esc 戻る "),
    );
    frame.render_widget(table, area);
}

fn render_history_overlay_compact(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = centered_rect(96, 84, frame.area());
    frame.render_widget(Clear, area);
    let years = &app.simulation.world.statistics.years;
    let available_rows = usize::from(area.height.saturating_sub(5));
    let max_offset = years.len().saturating_sub(available_rows);
    let offset = app.history_offset.min(max_offset);
    let end = years.len().saturating_sub(offset);
    let start = end.saturating_sub(available_rows);
    let good = app.selected_good();
    let rows = years[start..end].iter().filter_map(|year| {
        let economy = year
            .town_economies
            .iter()
            .find(|economy| economy.town == app.town_id)?;
        Some(Row::new(vec![
            Cell::from(year.year.to_string()),
            Cell::from(format_money(economy.economic_power_cents)),
            Cell::from(format!("{:.2}", economy.price_index as f64 / 10_000.0)),
            Cell::from(format!(
                "{:+.2}%",
                economy.inflation_basis_points as f64 / 100.0
            )),
            Cell::from(history_price(economy, good)),
        ]))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Length(15),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Min(12),
        ],
    )
    .header(
        Row::new(["年", "経済力", "物価", "変動", good_label(good)])
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" 経済履歴  │  ↑↓ スクロール  H/Esc 戻る "),
    );
    frame.render_widget(table, area);
}

fn render_help_overlay(frame: &mut ratatui::Frame<'_>) {
    let area = centered_rect(64, 70, frame.area());
    frame.render_widget(Clear, area);
    let help = vec![
        Line::styled(
            "目標: 最終年の時価決済後に所持金をできるだけ増やす",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        help_line("↑ / ↓ または J / K", "取引する商品を選択"),
        help_line("1 ～ 5", "商品を直接選択"),
        help_line("← / → または - / +", "取引数量を1ずつ変更"),
        help_line("Shift + ← / →", "取引数量を10ずつ変更"),
        help_line("B", "選択した商品を購入"),
        help_line("S", "選択した商品を売却"),
        help_line("N または Enter", "翌年へ進む"),
        help_line("H", "全期間の経済履歴を表示"),
        help_line("Q または Esc", "終了確認を表示"),
        Line::raw(""),
        Line::styled(
            "売買は現在の都市価格で行われます。都市財政や在庫が不足する取引は拒否されます。",
            Style::default().fg(Color::Gray),
        ),
    ];
    frame.render_widget(
        Paragraph::new(help).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" ヘルプ  │  ? / Esc で戻る "),
        ),
        area,
    );
}

fn render_finished_overlay(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = centered_rect(58, 58, frame.area());
    frame.render_widget(Clear, area);
    let final_cash = app.final_cash.unwrap_or(app.player.cash_cents);
    let profit = i128::from(final_cash) - i128::from(app.player.starting_cash_cents);
    let return_rate = if app.player.starting_cash_cents == 0 {
        0.0
    } else {
        profit as f64 / app.player.starting_cash_cents as f64 * 100.0
    };
    let profit_color = if profit >= 0 {
        Color::Green
    } else {
        Color::Red
    };
    let lines = vec![
        Line::styled(
            "ゲーム終了",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        score_line("開始所持金", format_money(app.player.starting_cash_cents)),
        score_line("商品決済額", format_money(app.final_inventory_value)),
        score_line("最終所持金", format_money(final_cash)),
        Line::from(vec![
            Span::raw("損益          "),
            Span::styled(
                format_signed_money(profit),
                Style::default()
                    .fg(profit_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("収益率        "),
            Span::styled(
                format!("{return_rate:+.2}%"),
                Style::default()
                    .fg(profit_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        score_line("売買回数", format!("{}回", app.player.trades.len())),
        Line::raw(""),
        Line::styled("Enter または Q で終了", Style::default().fg(Color::Cyan)),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" 最終成績 ")),
        area,
    );
}

fn render_quit_overlay(frame: &mut ratatui::Frame<'_>) {
    let area = centered_rect(46, 24, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw("最終年まで進まずに終了しますか？"),
            Line::raw(""),
            Line::styled("Q / Enter: 終了    N / Esc: 戻る", key_style()),
        ])
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title(" 終了確認 ")),
        area,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn history_price(economy: &TownEconomicStatistics, good: Good) -> String {
    economy
        .goods
        .iter()
        .find(|item| item.good == good)
        .map_or_else(
            || "-".to_owned(),
            |item| format_money(item.unit_price_cents),
        )
}

fn help_line(key: &'static str, description: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<22}"), key_style()),
        Span::raw(description),
    ])
}

fn score_line(label: &'static str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!("{label:<14}")),
        Span::styled(
            value,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn key_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn inflation_color(basis_points: i32) -> Color {
    match basis_points.cmp(&0) {
        std::cmp::Ordering::Greater => Color::Red,
        std::cmp::Ordering::Less => Color::Green,
        std::cmp::Ordering::Equal => Color::Gray,
    }
}

fn good_label(good: Good) -> &'static str {
    match good {
        Good::Food => "食料",
        Good::Clothing => "衣料",
        Good::Medicine => "医薬品",
        Good::Tools => "工具",
        Good::Luxury => "嗜好品",
    }
}

fn format_money(cents: Money) -> String {
    format!("{}.{:02}", format_u64(cents / 100), cents % 100)
}

fn format_signed_money(cents: i128) -> String {
    let sign = if cents >= 0 { "+" } else { "-" };
    let absolute = cents.unsigned_abs();
    format!(
        "{sign}{}.{:02}",
        format_u128(absolute / 100),
        absolute % 100
    )
}

fn format_u64(value: u64) -> String {
    format_u128(u128::from(value))
}

fn format_u128(value: u128) -> String {
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
    use ratatui::backend::TestBackend;

    fn app(years: u16) -> App {
        let simulation = Simulation::new(1, 20, 12345, SimulationConfig::default())
            .expect("simulation should build");
        App::new(simulation, TownId(0), years, 100_000).expect("app should build")
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn keyboard_controls_select_quantity_and_trade() {
        let mut app = app(2);
        let initial_cash = app.player.cash_cents;
        let food_price = app
            .simulation
            .world
            .town(app.town_id)
            .expect("town should exist")
            .economy
            .good_price(Good::Food);

        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Char('b')));

        assert_eq!(app.quantity, 2);
        assert_eq!(app.player.inventory.quantity(Good::Food), 2);
        assert_eq!(app.player.cash_cents, initial_cash - food_price * 2);
        assert_eq!(app.message.kind, MessageKind::Success);

        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_good(), Good::Clothing);
        app.handle_key(key(KeyCode::Char('5')));
        assert_eq!(app.selected_good(), Good::Luxury);
    }

    #[test]
    fn warmup_history_exists_before_player_turns_begin() {
        let mut simulation =
            Simulation::new(1, 20, 12345, SimulationConfig::default()).expect("should build");
        simulation.run(6).expect("warmup should run");

        let mut app = App::new(simulation, TownId(0), 2, 100_000).expect("app should build");

        assert_eq!(app.simulation.world.year, 6);
        assert_eq!(app.simulation.world.statistics.years.len(), 6);
        assert_eq!(app.completed_years, 0);
        assert_eq!(app.player.cash_cents, 100_000);
        assert_eq!(
            history_points(&app, |economy| economy.economic_power_cents).len(),
            6
        );
        assert!(app.message.text.contains("6年間進行済み"));

        app.advance_year();
        assert_eq!(app.simulation.world.year, 7);
        assert_eq!(app.completed_years, 1);
    }

    #[test]
    fn last_year_settles_inventory_and_opens_score_screen() {
        let mut app = app(1);
        app.handle_key(key(KeyCode::Char('b')));

        let should_exit = app.handle_key(key(KeyCode::Char('n')));

        assert!(!should_exit);
        assert_eq!(app.completed_years, 1);
        assert_eq!(app.screen, Screen::Finished);
        assert!(app.final_cash.is_some());
        assert!(
            GOODS
                .into_iter()
                .all(|good| app.player.inventory.quantity(good) == 0)
        );
        assert!(app.handle_key(key(KeyCode::Enter)));
    }

    #[test]
    fn main_screen_and_overlays_render_on_test_terminal() {
        let mut app = app(2);
        app.advance_year();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal should build");

        terminal
            .draw(|frame| render(frame, &app))
            .expect("main screen should render");
        app.screen = Screen::History;
        terminal
            .draw(|frame| render(frame, &app))
            .expect("history should render");
        app.screen = Screen::Help;
        terminal
            .draw(|frame| render(frame, &app))
            .expect("help should render");

        let compact_backend = TestBackend::new(80, 24);
        let mut compact_terminal =
            Terminal::new(compact_backend).expect("compact terminal should build");
        app.screen = Screen::Market;
        compact_terminal
            .draw(|frame| render(frame, &app))
            .expect("compact main screen should render");
        app.screen = Screen::History;
        compact_terminal
            .draw(|frame| render(frame, &app))
            .expect("compact history should render");
    }
}
