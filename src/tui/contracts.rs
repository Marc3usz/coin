use crate::crypto::{decode_hash, hex_hash};
use crate::tui::app::{push_log, App};
use crate::types::Transaction;
use crate::vm::{encode_contract_call, ContractCallKind, ContractCallPayload, Value};
use crate::wallet::{sign_tx, WalletFile};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_input::backend::crossterm::EventHandler;

pub fn handle_event(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Down => app.contracts_state.focus = (app.contracts_state.focus + 1) % 6,
        KeyCode::Up => app.contracts_state.focus = (app.contracts_state.focus + 5) % 6,
        KeyCode::Enter => {
            if app.contracts_state.focus == 5 {
                submit_contract_call(app);
            } else {
                app.contracts_state.focus = (app.contracts_state.focus + 1) % 6;
            }
        }
        _ => match app.contracts_state.focus {
            0 => input(&mut app.contracts_state.address_input, key),
            1 => input(&mut app.contracts_state.method_input, key),
            2 => input(&mut app.contracts_state.args_input, key),
            3 => input(&mut app.contracts_state.value_input, key),
            4 => input(&mut app.contracts_state.fee_input, key),
            _ => {}
        },
    }
}

fn input(input: &mut tui_input::Input, key: KeyEvent) {
    input.handle_event(&crossterm::event::Event::Key(key));
}

fn submit_contract_call(app: &mut App) {
    let addr_str = app.contracts_state.address_input.value().trim();
    let method_str = app.contracts_state.method_input.value().trim();
    let args_str = app.contracts_state.args_input.value().trim();
    let value_str = app.contracts_state.value_input.value().trim();
    let fee_str = app.contracts_state.fee_input.value().trim();
    push_log(&mut app.contracts_state.logs, "contract call requested");

    let mut resolved_addr_str = addr_str;
    let mut resolved_method_idx = None;
    if let Some(entry) = app.address_book.entries.get(addr_str) {
        resolved_addr_str = &entry.address;
        resolved_method_idx = entry.method_id_for_name(method_str);
        push_log(
            &mut app.contracts_state.logs,
            format!("resolved address book entry '{addr_str}'"),
        );
    }

    let to = match decode_hash(resolved_addr_str) {
        Ok(addr) => Some(addr),
        Err(_) => {
            app.contracts_state.result_msg = "invalid contract address".to_string();
            push_log(&mut app.contracts_state.logs, "invalid contract address");
            return;
        }
    };
    let method_idx = match resolved_method_idx.or_else(|| method_str.parse::<u16>().ok()) {
        Some(idx) => idx,
        None => {
            app.contracts_state.result_msg = "method must be a u16 ID or ABI name".to_string();
            push_log(&mut app.contracts_state.logs, "method resolution failed");
            return;
        }
    };
    push_log(
        &mut app.contracts_state.logs,
        format!("method resolved to id {method_idx}"),
    );
    let args = match parse_args(args_str) {
        Ok(args) => args,
        Err(err) => {
            app.contracts_state.result_msg = err;
            push_log(
                &mut app.contracts_state.logs,
                format!("argument parse failed: {}", app.contracts_state.result_msg),
            );
            return;
        }
    };
    push_log(
        &mut app.contracts_state.logs,
        format!("encoded {} arguments", args.len()),
    );
    let value = match parse_u128(value_str, "value") {
        Ok(v) => v,
        Err(err) => {
            app.contracts_state.result_msg = err;
            push_log(
                &mut app.contracts_state.logs,
                format!("invalid call value: {}", app.contracts_state.result_msg),
            );
            return;
        }
    };
    let fee = match parse_u128(fee_str, "fee") {
        Ok(v) => v,
        Err(err) => {
            app.contracts_state.result_msg = err;
            push_log(
                &mut app.contracts_state.logs,
                format!("invalid mining tip: {}", app.contracts_state.result_msg),
            );
            return;
        }
    };
    let payload = match encode_contract_call(&ContractCallPayload {
        kind: ContractCallKind::Method,
        method_idx,
        args,
    }) {
        Ok(bytes) => bytes,
        Err(e) => {
            app.contracts_state.result_msg = format!("failed to encode payload: {e}");
            push_log(
                &mut app.contracts_state.logs,
                format!("payload encode failed: {e}"),
            );
            return;
        }
    };
    push_log(
        &mut app.contracts_state.logs,
        format!("LCALL payload: {} bytes", payload.len()),
    );

    push_log(&mut app.contracts_state.logs, "loading signing wallet");
    let mut node = app.node.lock().unwrap();
    let wallet_path = node.core.cfg.wallet_path.clone();
    let wallet = match WalletFile::load(&wallet_path) {
        Ok(w) => w,
        Err(e) => {
            app.contracts_state.result_msg = format!("failed to load wallet: {e}");
            push_log(
                &mut app.contracts_state.logs,
                format!("wallet load failed: {e}"),
            );
            return;
        }
    };
    let from_addr = match wallet.address() {
        Ok(a) => a,
        Err(e) => {
            app.contracts_state.result_msg = format!("wallet address error: {e}");
            push_log(
                &mut app.contracts_state.logs,
                format!("wallet address error: {e}"),
            );
            return;
        }
    };
    let account = node.core.store.get_account(&from_addr).unwrap_or_default();
    push_log(
        &mut app.contracts_state.logs,
        format!("sender account index: {}", account.account_index),
    );
    let tx = Transaction {
        from: from_addr,
        to,
        value,
        gas_limit: 10_000_000,
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
            app.contracts_state.result_msg = format!("sign error: {e}");
            push_log(
                &mut app.contracts_state.logs,
                format!("signing failed: {e}"),
            );
            return;
        }
    };
    let tx_hash = tx.hash().unwrap_or([0; 32]);
    push_log(
        &mut app.contracts_state.logs,
        format!("signed tx: {}", hex_hash(&tx_hash)),
    );
    let peers_len = node.peers.len();
    let result = node.core.submit_tx(tx, peers_len);
    if result.accepted {
        let submitted_hash = result.tx_hash.unwrap_or(tx_hash);
        app.contracts_state.result_msg = format!("call submitted: {}", hex_hash(&submitted_hash));
        push_log(
            &mut app.contracts_state.logs,
            format!("accepted into mempool: {}", hex_hash(&submitted_hash)),
        );
        push_log(&mut app.contracts_state.logs, "waiting for mining/receipt");
        app.contracts_state.address_input.reset();
        app.contracts_state.method_input.reset();
        app.contracts_state.args_input.reset();
        app.contracts_state.focus = 0;
    } else {
        let error = result.error.unwrap_or_else(|| "unknown error".to_string());
        app.contracts_state.result_msg = format!("failed: {}", error);
        push_log(
            &mut app.contracts_state.logs,
            format!("submit rejected: {error}"),
        );
    }
}

fn parse_args(args_str: &str) -> Result<Vec<Value>, String> {
    let mut args = Vec::new();
    if !args_str.is_empty() {
        for arg in args_str.split(',') {
            let arg = arg.trim();
            if let Ok(v) = arg.parse::<u64>() {
                args.push(Value::U64(v));
            } else if arg.starts_with("0x") {
                args.push(Value::Address(
                    decode_hash(arg).map_err(|_| format!("invalid 32-byte address: {arg}"))?,
                ));
            } else {
                return Err(format!("invalid argument: {arg}"));
            }
        }
    }
    Ok(args)
}

fn parse_u128(text: &str, label: &str) -> Result<u128, String> {
    text.parse::<u128>().map_err(|_| format!("invalid {label}"))
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
        if app.contracts_state.focus == idx {
            focused
        } else {
            normal
        }
    };

    render_input(
        f,
        chunks[0],
        " Contract Address or Name ",
        app.contracts_state.address_input.value(),
        style(0),
    );
    render_input(
        f,
        chunks[1],
        " Method ID or ABI Name ",
        app.contracts_state.method_input.value(),
        style(1),
    );
    render_input(
        f,
        chunks[2],
        " Args: 100, 0x... ",
        app.contracts_state.args_input.value(),
        style(2),
    );
    render_input(
        f,
        chunks[3],
        " Coins to Send ",
        app.contracts_state.value_input.value(),
        style(3),
    );
    render_input(
        f,
        chunks[4],
        " Mining Tip ",
        app.contracts_state.fee_input.value(),
        style(4),
    );
    let btn_style = if app.contracts_state.focus == 5 {
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    };
    f.render_widget(
        Paragraph::new(" [ EXECUTE CONTRACT CALL ] ")
            .block(Block::default().borders(Borders::ALL))
            .style(btn_style),
        chunks[5],
    );
    f.render_widget(
        Paragraph::new(app.contracts_state.result_msg.clone())
            .block(Block::default().borders(Borders::ALL).title(" Result "))
            .style(Style::default().fg(Color::Green)),
        chunks[6],
    );
    f.render_widget(
        Paragraph::new(app.contracts_state.logs.join("\n"))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Live Contract Log "),
            )
            .style(Style::default().fg(Color::Cyan)),
        chunks[7],
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
