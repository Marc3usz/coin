use crate::chain::contract_address;
use crate::crypto::hex_hash;
use crate::tui::app::{push_log, App};
use crate::types::Transaction;
use crate::vm::decode_contract_blob;
use crate::wallet::{sign_tx, WalletFile};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::path::Path;
use tui_input::backend::crossterm::EventHandler;

pub fn handle_event(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Down => app.deploy_state.focus = (app.deploy_state.focus + 1) % 5,
        KeyCode::Up => app.deploy_state.focus = (app.deploy_state.focus + 4) % 5,
        KeyCode::Enter => {
            if app.deploy_state.focus == 4 {
                submit_contract_deploy(app);
            } else {
                app.deploy_state.focus = (app.deploy_state.focus + 1) % 5;
            }
        }
        _ => match app.deploy_state.focus {
            0 => input(&mut app.deploy_state.deploy_path_input, key),
            1 => input(&mut app.deploy_state.value_input, key),
            2 => input(&mut app.deploy_state.deploy_gas_input, key),
            3 => input(&mut app.deploy_state.fee_input, key),
            _ => {}
        },
    }
}

fn input(input: &mut tui_input::Input, key: KeyEvent) {
    input.handle_event(&crossterm::event::Event::Key(key));
}

fn submit_contract_deploy(app: &mut App) {
    let path = app.deploy_state.deploy_path_input.value().trim();
    push_log(&mut app.deploy_state.logs, "deploy requested");
    if path.is_empty() {
        app.deploy_state.result_msg = "enter a .litevm or .bin.litevm path".to_string();
        push_log(
            &mut app.deploy_state.logs,
            "deploy blocked: missing file path",
        );
        return;
    }

    push_log(
        &mut app.deploy_state.logs,
        format!("reading bytecode: {path}"),
    );
    let payload = match load_contract_file(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            app.deploy_state.result_msg = err;
            push_log(
                &mut app.deploy_state.logs,
                format!("read failed: {}", app.deploy_state.result_msg),
            );
            return;
        }
    };
    push_log(
        &mut app.deploy_state.logs,
        format!("loaded {} bytes", payload.len()),
    );
    if let Err(err) = decode_contract_blob(&payload) {
        app.deploy_state.result_msg = format!("invalid contract blob: {err}");
        push_log(
            &mut app.deploy_state.logs,
            format!("validation failed: {err}"),
        );
        return;
    }
    push_log(&mut app.deploy_state.logs, "LVM1 blob validated");

    let value = match parse_u128(app.deploy_state.value_input.value().trim(), "value") {
        Ok(v) => v,
        Err(err) => {
            app.deploy_state.result_msg = err;
            push_log(
                &mut app.deploy_state.logs,
                format!("invalid value: {}", app.deploy_state.result_msg),
            );
            return;
        }
    };
    let gas_limit = match app
        .deploy_state
        .deploy_gas_input
        .value()
        .trim()
        .parse::<u64>()
    {
        Ok(v) if v > 0 => v,
        _ => {
            app.deploy_state.result_msg = "invalid gas limit".to_string();
            push_log(&mut app.deploy_state.logs, "invalid gas limit");
            return;
        }
    };
    let fee = match parse_u128(app.deploy_state.fee_input.value().trim(), "fee") {
        Ok(v) => v,
        Err(err) => {
            app.deploy_state.result_msg = err;
            push_log(
                &mut app.deploy_state.logs,
                format!("invalid fee: {}", app.deploy_state.result_msg),
            );
            return;
        }
    };

    push_log(&mut app.deploy_state.logs, "loading signing wallet");
    let mut node = app.node.lock().unwrap();
    let wallet_path = node.core.cfg.wallet_path.clone();
    let wallet = match WalletFile::load(&wallet_path) {
        Ok(w) => w,
        Err(e) => {
            app.deploy_state.result_msg = format!("failed to load wallet: {e}");
            push_log(
                &mut app.deploy_state.logs,
                format!("wallet load failed: {e}"),
            );
            return;
        }
    };
    let from_addr = match wallet.address() {
        Ok(a) => a,
        Err(e) => {
            app.deploy_state.result_msg = format!("wallet address error: {e}");
            push_log(
                &mut app.deploy_state.logs,
                format!("wallet address error: {e}"),
            );
            return;
        }
    };
    let account = node.core.store.get_account(&from_addr).unwrap_or_default();
    let expected_contract = contract_address(&from_addr, account.account_index);
    push_log(
        &mut app.deploy_state.logs,
        format!(
            "expected contract address: {}",
            hex_hash(&expected_contract)
        ),
    );
    let tx = Transaction {
        from: from_addr,
        to: None,
        value,
        gas_limit,
        max_gas_price: 1000,
        mining_tip: fee,
        expiration_height: None,
        payload,
        account_index: account.account_index,
        nonce: 0,
        public_key: vec![],
        signature: vec![],
    };
    let tx = match sign_tx(tx, node.core.cfg.chain_id, &wallet) {
        Ok(t) => t,
        Err(e) => {
            app.deploy_state.result_msg = format!("sign error: {e}");
            push_log(&mut app.deploy_state.logs, format!("signing failed: {e}"));
            return;
        }
    };
    let tx_hash = tx.hash().unwrap_or([0; 32]);
    push_log(
        &mut app.deploy_state.logs,
        format!("signed tx: {}", hex_hash(&tx_hash)),
    );
    let peers_len = node.peers.len();
    let result = node.core.submit_tx(tx, peers_len);
    if result.accepted {
        let submitted_hash = result.tx_hash.unwrap_or(tx_hash);
        app.deploy_state.result_msg = format!(
            "deploy submitted: {}\nexpected contract address after mining: {}",
            hex_hash(&submitted_hash),
            hex_hash(&expected_contract)
        );
        push_log(
            &mut app.deploy_state.logs,
            format!("accepted into mempool: {}", hex_hash(&submitted_hash)),
        );
        push_log(
            &mut app.deploy_state.logs,
            "waiting for miner to include tx in a block",
        );
        app.deploy_state.deploy_path_input.reset();
        app.deploy_state.focus = 0;
    } else {
        let error = result.error.unwrap_or_else(|| "unknown error".to_string());
        app.deploy_state.result_msg = format!("failed: {}", error);
        push_log(
            &mut app.deploy_state.logs,
            format!("submit rejected: {error}"),
        );
    }
}

fn parse_u128(text: &str, label: &str) -> Result<u128, String> {
    text.parse::<u128>().map_err(|_| format!("invalid {label}"))
}

fn load_contract_file(path: &str) -> Result<Vec<u8>, String> {
    let path = Path::new(path);
    let bytes = std::fs::read(path).map_err(|err| format!("failed to read file: {err}"))?;
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".bin.litevm"))
    {
        return Ok(bytes);
    }

    let text =
        String::from_utf8(bytes).map_err(|err| format!("text .litevm is not UTF-8: {err}"))?;
    let compact = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or(""))
        .collect::<String>()
        .split_whitespace()
        .collect::<String>();
    let hex_text = compact.strip_prefix("0x").unwrap_or(&compact);
    if !hex_text.is_empty()
        && hex_text.len() % 2 == 0
        && hex_text.chars().all(|c| c.is_ascii_hexdigit())
    {
        hex::decode(hex_text).map_err(|err| format!("invalid hex bytecode: {err}"))
    } else {
        Ok(text.into_bytes())
    }
}

pub fn draw(app: &mut App, f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(7),
        ])
        .split(area);

    let focused = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(Color::White);
    let style = |idx| {
        if app.deploy_state.focus == idx {
            focused
        } else {
            normal
        }
    };

    render_input(
        f,
        chunks[0],
        " .litevm or .bin.litevm Path ",
        app.deploy_state.deploy_path_input.value(),
        style(0),
    );
    render_input(
        f,
        chunks[1],
        " Initial Coins to Send ",
        app.deploy_state.value_input.value(),
        style(1),
    );
    render_input(
        f,
        chunks[2],
        " Gas Limit ",
        app.deploy_state.deploy_gas_input.value(),
        style(2),
    );
    render_input(
        f,
        chunks[3],
        " Mining Tip ",
        app.deploy_state.fee_input.value(),
        style(3),
    );
    let btn_style = if app.deploy_state.focus == 4 {
        Style::default()
            .bg(Color::Magenta)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    };
    f.render_widget(
        Paragraph::new(" [ DEPLOY CONTRACT ] ")
            .block(Block::default().borders(Borders::ALL))
            .style(btn_style),
        chunks[4],
    );
    f.render_widget(
        Paragraph::new(app.deploy_state.result_msg.clone())
            .block(Block::default().borders(Borders::ALL).title(" Result "))
            .style(Style::default().fg(Color::Green)),
        chunks[5],
    );
    f.render_widget(
        Paragraph::new(app.deploy_state.logs.join("\n"))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Live Deploy Log "),
            )
            .style(Style::default().fg(Color::Cyan)),
        chunks[6],
    );
}

fn render_input(f: &mut Frame, area: Rect, title: &str, value: &str, style: Style) {
    f.render_widget(
        Paragraph::new(value.to_string())
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(style),
        area,
    );
}
