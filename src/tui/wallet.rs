use crate::crypto::hex_hash;
use crate::tui::app::{push_log, App, WalletPrompt};
use crate::wallet::WalletFile;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_input::backend::crossterm::EventHandler;

pub fn handle_event(app: &mut App, key: KeyEvent) {
    match app.wallet_state.prompt {
        WalletPrompt::CreateMissing => match key.code {
            KeyCode::Char('y') | KeyCode::Enter => create_wallet(app, false),
            KeyCode::Char('n') | KeyCode::Esc => {
                app.wallet_state.prompt = WalletPrompt::None;
                app.wallet_state.missing_prompt_dismissed = true;
                app.wallet_state.result_msg = "wallet creation cancelled".to_string();
                push_log(&mut app.wallet_state.logs, "wallet creation cancelled");
            }
            _ => {}
        },
        WalletPrompt::ConfirmOverwrite => match key.code {
            KeyCode::Enter => {
                if app.wallet_state.confirm_input.value() == "OVERWRITE" {
                    create_wallet(app, true);
                } else {
                    app.wallet_state.result_msg =
                        "type OVERWRITE exactly before pressing Enter".to_string();
                    push_log(
                        &mut app.wallet_state.logs,
                        "overwrite confirmation rejected",
                    );
                }
            }
            KeyCode::Esc => {
                app.wallet_state.prompt = WalletPrompt::None;
                app.wallet_state.confirm_input.reset();
                app.wallet_state.result_msg = "overwrite cancelled".to_string();
                push_log(&mut app.wallet_state.logs, "wallet overwrite cancelled");
            }
            _ => {
                app.wallet_state
                    .confirm_input
                    .handle_event(&crossterm::event::Event::Key(key));
            }
        },
        WalletPrompt::None => match key.code {
            KeyCode::Char('c') => {
                app.wallet_state.missing_prompt_dismissed = false;
                let wallet_path = app.node.lock().unwrap().core.cfg.wallet_path.clone();
                if wallet_path.exists() {
                    app.wallet_state.prompt = WalletPrompt::ConfirmOverwrite;
                    app.wallet_state.confirm_input.reset();
                    app.wallet_state.result_msg =
                        "wallet exists; type OVERWRITE and press Enter to replace it".to_string();
                } else {
                    app.wallet_state.prompt = WalletPrompt::CreateMissing;
                    app.wallet_state.result_msg = "create a new wallet? y/n".to_string();
                }
            }
            _ => {}
        },
    }
}

fn create_wallet(app: &mut App, overwrite: bool) {
    let wallet_path = app.node.lock().unwrap().core.cfg.wallet_path.clone();
    if wallet_path.exists() && !overwrite {
        app.wallet_state.result_msg = "wallet already exists; refusing to overwrite".to_string();
        push_log(
            &mut app.wallet_state.logs,
            "refused to overwrite existing wallet",
        );
        app.wallet_state.prompt = WalletPrompt::None;
        return;
    }

    let wallet = WalletFile::generate();
    match wallet.save(&wallet_path) {
        Ok(()) => {
            let mut node = app.node.lock().unwrap();
            if let Ok(addr) = wallet.address() {
                node.core.cfg.miner_address = Some(addr);
                node.mining.push_log("miner address updated from wallet");
            }
            app.wallet_state.result_msg = format!("wallet saved: {}", wallet_path.display());
            push_log(
                &mut app.wallet_state.logs,
                format!("wallet saved at {}", wallet_path.display()),
            );
            app.wallet_state.prompt = WalletPrompt::None;
            app.wallet_state.missing_prompt_dismissed = false;
            app.wallet_state.confirm_input.reset();
        }
        Err(err) => {
            app.wallet_state.result_msg = format!("failed to save wallet: {err}");
            push_log(
                &mut app.wallet_state.logs,
                format!("wallet save failed: {err}"),
            );
        }
    }
}

fn wallet_history(app: &App, addr: [u8; 32]) -> Vec<String> {
    let node = app.node.lock().unwrap();
    let mut rows = Vec::new();
    for tx in node.core.mempool.all() {
        if tx.from == addr || tx.to == Some(addr) {
            rows.push(format!(
                "mempool tx {} | {} -> {} | value {} | tip {}",
                hex_hash(&tx.hash().unwrap_or([0; 32])),
                short_addr(&tx.from),
                tx.to
                    .map(|addr| short_addr(&addr))
                    .unwrap_or_else(|| "deploy".to_string()),
                tx.value,
                tx.mining_tip
            ));
        }
    }
    let height = node.core.store.height().unwrap_or(0);
    for h in (0..=height).rev().take(200) {
        let Some(block) = node.core.store.get_block_by_height(h).ok().flatten() else {
            continue;
        };
        if block.header.miner_address == addr {
            rows.push(format!("block {h} mined reward | {}", short_addr(&addr)));
        }
        for tx in &block.transactions {
            if tx.from == addr || tx.to == Some(addr) {
                rows.push(format!(
                    "block {h} tx {} | {} -> {} | value {} | gas {} | tip {}",
                    hex_hash(&tx.hash().unwrap_or([0; 32])),
                    short_addr(&tx.from),
                    tx.to
                        .map(|addr| short_addr(&addr))
                        .unwrap_or_else(|| "deploy".to_string()),
                    tx.value,
                    tx.gas_limit,
                    tx.mining_tip
                ));
            }
        }
        if rows.len() >= 40 {
            break;
        }
    }
    if rows.is_empty() {
        rows.push("No wallet operations found in mempool or last 200 blocks.".to_string());
    }
    rows
}

fn short_addr(addr: &[u8; 32]) -> String {
    let hex = hex_hash(addr);
    format!("{}...{}", &hex[..8], &hex[hex.len() - 8..])
}

pub fn draw(app: &mut App, f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(11),
            Constraint::Length(7),
            Constraint::Min(8),
            Constraint::Length(8),
        ])
        .split(area);

    let wallet_path = app.node.lock().unwrap().core.cfg.wallet_path.clone();
    let wallet_info = WalletFile::load(&wallet_path)
        .and_then(|wallet| {
            let addr = wallet.address()?;
            let node = app.node.lock().unwrap();
            let account = node.core.store.get_account(&addr).unwrap_or_default();
            Ok((
                wallet.address_hex,
                addr,
                account.balance,
                account.account_index,
            ))
        })
        .ok();

    let text = if let Some((address, addr, balance, account_index)) = wallet_info.as_ref() {
        let short = hex_hash(&addr);
        format!(
            "Wallet File: {}\nAddress: {}\nShort: {}...{}\nBalance: {} coins\nAccount Index: {}\n\nKeys: c create/replace wallet with confirmation | Esc dashboard",
            wallet_path.display(),
            address,
            &short[..12],
            &short[short.len() - 12..],
            balance,
            account_index
        )
    } else {
        if app.wallet_state.prompt == WalletPrompt::None
            && !app.wallet_state.missing_prompt_dismissed
        {
            app.wallet_state.prompt = WalletPrompt::CreateMissing;
        }
        let action = if app.wallet_state.missing_prompt_dismissed {
            "Press c whenever you are ready to create one."
        } else {
            "Press y or Enter to create one now. Press n to leave this tab read-only."
        };
        format!(
            "No wallet found at {}\n\nThis node can run, but transfers, contract calls, and miner rewards need a wallet.\n\nPress y or Enter to create one now. Press n to leave this tab read-only.",
            wallet_path.display()
        )
        .replace("Press y or Enter to create one now. Press n to leave this tab read-only.", action)
    };

    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(" Wallet "))
        .style(Style::default().fg(Color::White));
    f.render_widget(p, chunks[0]);

    let prompt = match app.wallet_state.prompt {
        WalletPrompt::None => "No pending wallet action.".to_string(),
        WalletPrompt::CreateMissing => {
            "Create missing wallet? y/Enter = create, n/Esc = cancel".to_string()
        }
        WalletPrompt::ConfirmOverwrite => format!(
            "Overwrite protection active. Type OVERWRITE: {}",
            app.wallet_state.confirm_input.value()
        ),
    };
    let prompt_style = if app.wallet_state.prompt == WalletPrompt::ConfirmOverwrite {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Yellow)
    };
    f.render_widget(
        Paragraph::new(prompt)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Wallet Action "),
            )
            .style(prompt_style),
        chunks[1],
    );

    let history_text = wallet_info
        .as_ref()
        .map(|(_, addr, _, _)| wallet_history(app, *addr).join("\n"))
        .unwrap_or_else(|| "Wallet history unavailable until a wallet is loaded.".to_string());

    f.render_widget(
        Paragraph::new(app.wallet_state.result_msg.clone())
            .block(Block::default().borders(Borders::ALL).title(" Status "))
            .style(Style::default().fg(Color::Green)),
        chunks[2],
    );

    let logs = format!(
        "Operation History\n{}\n\nLocal Wallet Log\n{}",
        history_text,
        app.wallet_state.logs.join("\n")
    );
    f.render_widget(
        Paragraph::new(logs)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Wallet Activity "),
            )
            .style(Style::default().fg(Color::Cyan)),
        chunks[3],
    );
}
