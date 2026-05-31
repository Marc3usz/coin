use crate::node::NodeServer;
use crate::tui::address_book::AddressBook;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Tabs},
    Terminal,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::{io, time::Duration};
use tui_input::Input;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Wallet,
    Transfer,
    Contracts,
    Deploy,
    AddressBook,
    AbiWizard,
    Peers,
    Search,
}

impl Tab {
    const ALL: [Tab; 9] = [
        Tab::Dashboard,
        Tab::Wallet,
        Tab::Transfer,
        Tab::Contracts,
        Tab::Deploy,
        Tab::AddressBook,
        Tab::AbiWizard,
        Tab::Peers,
        Tab::Search,
    ];

    fn title(&self) -> &'static str {
        match self {
            Tab::Dashboard => "1. Dashboard",
            Tab::Wallet => "2. Wallet",
            Tab::Transfer => "3. Transfer",
            Tab::Contracts => "4. Contracts",
            Tab::Deploy => "5. Deploy",
            Tab::AddressBook => "6. Address Book",
            Tab::AbiWizard => "7. ABI Wizard",
            Tab::Peers => "8. Peers",
            Tab::Search => "9. Search",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Auto,
    Address,
    Contract,
    Transaction,
    Block,
    Logs,
}

impl SearchMode {
    pub fn label(self) -> &'static str {
        match self {
            SearchMode::Auto => "Auto",
            SearchMode::Address => "Address",
            SearchMode::Contract => "Contract",
            SearchMode::Transaction => "Transaction",
            SearchMode::Block => "Block",
            SearchMode::Logs => "Logs",
        }
    }

    pub fn next(self) -> Self {
        match self {
            SearchMode::Auto => SearchMode::Address,
            SearchMode::Address => SearchMode::Contract,
            SearchMode::Contract => SearchMode::Transaction,
            SearchMode::Transaction => SearchMode::Block,
            SearchMode::Block => SearchMode::Logs,
            SearchMode::Logs => SearchMode::Auto,
        }
    }
}

pub struct SearchState {
    pub query_input: Input,
    pub mode: SearchMode,
    pub results: Vec<String>,
    pub status: String,
    pub scroll: usize,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query_input: Input::default(),
            mode: SearchMode::Auto,
            results: vec![
                "Enter a height, block hash, tx hash, address, contract, or log text.".to_string(),
            ],
            status: "Ready. Tab cycles search mode, Enter searches.".to_string(),
            scroll: 0,
        }
    }
}

pub struct TransferState {
    pub to_input: Input,
    pub amount_input: Input,
    pub gas_limit_input: Input,
    pub max_gas_price_input: Input,
    pub fee_input: Input,
    pub grind_input: Input,
    pub focus: usize,
    pub result_msg: String,
}

impl Default for TransferState {
    fn default() -> Self {
        Self {
            to_input: Input::default(),
            amount_input: Input::default(),
            gas_limit_input: Input::default().with_value("100000".to_string()),
            max_gas_price_input: Input::default().with_value("1000".to_string()),
            fee_input: Input::default().with_value("1".to_string()),
            grind_input: Input::default().with_value("0".to_string()),
            focus: 6,
            result_msg: String::new(),
        }
    }
}

pub struct ContractsState {
    pub address_input: Input,
    pub method_input: Input,
    pub args_input: Input,
    pub value_input: Input,
    pub gas_limit_input: Input,
    pub max_gas_price_input: Input,
    pub fee_input: Input,
    pub grind_input: Input,
    pub arg_inputs: Vec<Input>,
    pub arg_labels: Vec<String>,
    pub arg_types: Vec<String>,
    pub abi_signature: Option<String>,
    pub focus: usize,
    pub result_msg: String,
    pub logs: Vec<String>,
}

pub struct DeployState {
    pub deploy_path_input: Input,
    pub deploy_gas_input: Input,
    pub value_input: Input,
    pub max_gas_price_input: Input,
    pub fee_input: Input,
    pub grind_input: Input,
    pub focus: usize,
    pub result_msg: String,
    pub logs: Vec<String>,
}

impl Default for ContractsState {
    fn default() -> Self {
        Self {
            address_input: Input::default(),
            method_input: Input::default(),
            args_input: Input::default(),
            value_input: Input::default().with_value("0".to_string()),
            gas_limit_input: Input::default().with_value("10000000".to_string()),
            max_gas_price_input: Input::default().with_value("1000".to_string()),
            fee_input: Input::default().with_value("1".to_string()),
            grind_input: Input::default().with_value("0".to_string()),
            arg_inputs: Vec::new(),
            arg_labels: Vec::new(),
            arg_types: Vec::new(),
            abi_signature: None,
            focus: 5,
            result_msg: String::new(),
            logs: vec!["contract wizard ready".to_string()],
        }
    }
}

impl Default for DeployState {
    fn default() -> Self {
        Self {
            deploy_path_input: Input::default(),
            deploy_gas_input: Input::default().with_value("10000000".to_string()),
            value_input: Input::default().with_value("0".to_string()),
            max_gas_price_input: Input::default().with_value("1000".to_string()),
            fee_input: Input::default().with_value("1".to_string()),
            grind_input: Input::default().with_value("0".to_string()),
            focus: 6,
            result_msg: String::new(),
            logs: vec!["deploy wizard ready".to_string()],
        }
    }
}

pub struct AddressBookState {
    pub selected: usize,
    pub editing: bool,
    pub name_input: Input,
    pub address_input: Input,
    pub result_msg: String,
}

impl Default for AddressBookState {
    fn default() -> Self {
        Self {
            selected: 0,
            editing: false,
            name_input: Input::default(),
            address_input: Input::default(),
            result_msg: String::new(),
        }
    }
}

pub struct AbiWizardState {
    pub entry_input: Input,
    pub method_id_input: Input,
    pub name_input: Input,
    pub args_input: Input,
    pub params_input: Input,
    pub rets_input: Input,
    pub focus: usize,
    pub result_msg: String,
}

impl Default for AbiWizardState {
    fn default() -> Self {
        Self {
            entry_input: Input::default(),
            method_id_input: Input::default(),
            name_input: Input::default(),
            args_input: Input::default().with_value("0".to_string()),
            params_input: Input::default(),
            rets_input: Input::default().with_value("0".to_string()),
            focus: 6,
            result_msg: String::new(),
        }
    }
}

pub struct PeersState {
    pub peer_input: Input,
    pub focus: usize,
    pub result_msg: String,
}

impl Default for PeersState {
    fn default() -> Self {
        Self {
            peer_input: Input::default(),
            focus: 1,
            result_msg: "LAN discovery runs in the background. Add a URL or host:port.".to_string(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WalletPrompt {
    None,
    CreateMissing,
    ConfirmOverwrite,
}

pub struct WalletState {
    pub prompt: WalletPrompt,
    pub confirm_input: Input,
    pub result_msg: String,
    pub missing_prompt_dismissed: bool,
    pub logs: Vec<String>,
}

impl Default for WalletState {
    fn default() -> Self {
        Self {
            prompt: WalletPrompt::None,
            confirm_input: Input::default(),
            result_msg: String::new(),
            missing_prompt_dismissed: false,
            logs: vec!["wallet panel ready".to_string()],
        }
    }
}

pub fn push_log(logs: &mut Vec<String>, msg: impl Into<String>) {
    logs.push(msg.into());
    if logs.len() > 14 {
        logs.remove(0);
    }
}

pub struct App {
    pub node: Arc<Mutex<NodeServer>>,
    pub config_path: PathBuf,
    pub address_book: AddressBook,
    pub active_tab: Tab,
    pub running: bool,
    pub wallet_state: WalletState,
    pub transfer_state: TransferState,
    pub contracts_state: ContractsState,
    pub deploy_state: DeployState,
    pub address_book_state: AddressBookState,
    pub abi_wizard_state: AbiWizardState,
    pub peers_state: PeersState,
    pub search_state: SearchState,
}

impl App {
    pub fn new(node: Arc<Mutex<NodeServer>>, config_path: Option<PathBuf>) -> Result<Self> {
        let (ab_path, config_path) = {
            let n = node.lock().unwrap();
            (
                n.core.cfg.config_dir.join("address_book.toml"),
                config_path.unwrap_or_else(|| n.core.cfg.config_dir.join("config.toml")),
            )
        };
        let address_book = AddressBook::load(&ab_path).unwrap_or_default();
        Ok(Self {
            node,
            config_path,
            address_book,
            active_tab: Tab::Dashboard,
            running: true,
            wallet_state: WalletState::default(),
            transfer_state: TransferState::default(),
            contracts_state: ContractsState::default(),
            deploy_state: DeployState::default(),
            address_book_state: AddressBookState::default(),
            abi_wizard_state: AbiWizardState::default(),
            peers_state: PeersState::default(),
            search_state: SearchState::default(),
        })
    }

    pub fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        while self.running {
            terminal.draw(|f| self.draw(f))?;

            if event::poll(Duration::from_millis(250))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Char('1') if !self.text_input_focused() => {
                                self.active_tab = Tab::Dashboard
                            }
                            KeyCode::Char('2') if !self.text_input_focused() => {
                                self.active_tab = Tab::Wallet
                            }
                            KeyCode::Char('3') if !self.text_input_focused() => {
                                self.active_tab = Tab::Transfer
                            }
                            KeyCode::Char('4') if !self.text_input_focused() => {
                                self.active_tab = Tab::Contracts
                            }
                            KeyCode::Char('5') if !self.text_input_focused() => {
                                self.active_tab = Tab::Deploy
                            }
                            KeyCode::Char('6') if !self.text_input_focused() => {
                                self.active_tab = Tab::AddressBook
                            }
                            KeyCode::Char('7') if !self.text_input_focused() => {
                                self.active_tab = Tab::AbiWizard
                            }
                            KeyCode::Char('8') if !self.text_input_focused() => {
                                self.active_tab = Tab::Peers
                            }
                            KeyCode::Char('9') if !self.text_input_focused() => {
                                self.active_tab = Tab::Search
                            }
                            KeyCode::Esc => {
                                if self.active_tab == Tab::Dashboard {
                                    self.running = false;
                                } else {
                                    self.active_tab = Tab::Dashboard;
                                }
                            }
                            _ => match self.active_tab {
                                Tab::Dashboard => crate::tui::dashboard::handle_event(self, key),
                                Tab::Wallet => crate::tui::wallet::handle_event(self, key),
                                Tab::Transfer => crate::tui::transfer::handle_event(self, key),
                                Tab::Contracts => crate::tui::contracts::handle_event(self, key),
                                Tab::Deploy => crate::tui::deploy::handle_event(self, key),
                                Tab::AddressBook => {
                                    crate::tui::address_book::handle_event(self, key)
                                }
                                Tab::AbiWizard => crate::tui::abi_wizard::handle_event(self, key),
                                Tab::Peers => crate::tui::peers::handle_event(self, key),
                                Tab::Search => crate::tui::search::handle_event(self, key),
                            },
                        }
                    }
                }
            }
        }

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        Ok(())
    }

    fn draw(&mut self, f: &mut ratatui::Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(f.area());

        let titles: Vec<_> = Tab::ALL.iter().map(|t| Line::from(t.title())).collect();
        let tabs = Tabs::new(titles)
            .block(Block::default().borders(Borders::ALL).title(" coin-node "))
            .select(self.active_tab as usize)
            .style(Style::default().fg(Color::Cyan))
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .bg(Color::Cyan)
                    .fg(Color::Black),
            );

        f.render_widget(tabs, chunks[0]);

        match self.active_tab {
            Tab::Dashboard => crate::tui::dashboard::draw(self, f, chunks[1]),
            Tab::Wallet => crate::tui::wallet::draw(self, f, chunks[1]),
            Tab::Transfer => crate::tui::transfer::draw(self, f, chunks[1]),
            Tab::Contracts => crate::tui::contracts::draw(self, f, chunks[1]),
            Tab::Deploy => crate::tui::deploy::draw(self, f, chunks[1]),
            Tab::AddressBook => crate::tui::address_book::draw(self, f, chunks[1]),
            Tab::AbiWizard => crate::tui::abi_wizard::draw(self, f, chunks[1]),
            Tab::Peers => crate::tui::peers::draw(self, f, chunks[1]),
            Tab::Search => crate::tui::search::draw(self, f, chunks[1]),
        }
    }

    fn text_input_focused(&self) -> bool {
        match self.active_tab {
            Tab::Dashboard => false,
            Tab::Wallet => self.wallet_state.prompt == WalletPrompt::ConfirmOverwrite,
            Tab::Transfer => self.transfer_state.focus < 6,
            Tab::Contracts => {
                let arg_count = self.contracts_state.arg_inputs.len().max(1);
                self.contracts_state.focus != arg_count + 7
            }
            Tab::Deploy => self.deploy_state.focus < 6,
            Tab::AddressBook => self.address_book_state.editing,
            Tab::AbiWizard => self.abi_wizard_state.focus < 6,
            Tab::Peers => self.peers_state.focus == 0,
            Tab::Search => true,
        }
    }
}
