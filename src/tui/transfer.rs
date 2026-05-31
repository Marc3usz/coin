use crate::crypto::decode_hash;
use crate::node::gossip_tx;
use crate::tui::app::App;
use crate::tui::tx_options::{parse_u128, parse_u64, sign_with_optional_grind};
use crate::types::Transaction;
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
    match key.code {
        KeyCode::Down => {
            app.transfer_state.focus = (app.transfer_state.focus + 1) % 7;
        }
        KeyCode::Up => {
            app.transfer_state.focus = (app.transfer_state.focus + 6) % 7;
        }
        KeyCode::Enter => {
            if app.transfer_state.focus == 6 {
                submit_transfer(app);
            } else {
                app.transfer_state.focus = (app.transfer_state.focus + 1) % 7;
            }
        }
        _ => match app.transfer_state.focus {
            0 => {
                app.transfer_state
                    .to_input
                    .handle_event(&crossterm::event::Event::Key(key));
            }
            1 => {
                app.transfer_state
                    .amount_input
                    .handle_event(&crossterm::event::Event::Key(key));
            }
            2 => {
                app.transfer_state
                    .gas_limit_input
                    .handle_event(&crossterm::event::Event::Key(key));
            }
            3 => {
                app.transfer_state
                    .max_gas_price_input
                    .handle_event(&crossterm::event::Event::Key(key));
            }
            4 => {
                app.transfer_state
                    .fee_input
                    .handle_event(&crossterm::event::Event::Key(key));
            }
            5 => {
                app.transfer_state
                    .grind_input
                    .handle_event(&crossterm::event::Event::Key(key));
            }
            _ => {}
        },
    }
}

fn submit_transfer(app: &mut App) {
    let to_str = app.transfer_state.to_input.value().trim();
    let amount_str = app.transfer_state.amount_input.value().trim();

    // Check address book first
    let to_addr_str = app
        .address_book
        .entries
        .get(to_str)
        .map(|e| e.address.as_str())
        .unwrap_or(to_str);

    let to = match decode_hash(to_addr_str) {
        Ok(addr) => Some(addr),
        Err(_) => {
            if to_str.is_empty() {
                None // Contract deployment
            } else {
                app.transfer_state.result_msg = "Invalid To address".to_string();
                return;
            }
        }
    };

    let amount = match parse_u128(amount_str, "amount") {
        Ok(v) => v,
        Err(e) => {
            app.transfer_state.result_msg = e;
            return;
        }
    };
    let gas_limit = match parse_u64(app.transfer_state.gas_limit_input.value(), "gas limit") {
        Ok(v) if v > 0 => v,
        _ => {
            app.transfer_state.result_msg = "invalid gas limit".to_string();
            return;
        }
    };
    let max_gas_price = match parse_u128(
        app.transfer_state.max_gas_price_input.value(),
        "max gas price",
    ) {
        Ok(v) => v,
        Err(e) => {
            app.transfer_state.result_msg = e;
            return;
        }
    };
    let fee = match parse_u128(app.transfer_state.fee_input.value(), "mining tip") {
        Ok(v) => v,
        Err(e) => {
            app.transfer_state.result_msg = e;
            return;
        }
    };
    let grind = match parse_u64(app.transfer_state.grind_input.value(), "grind iterations") {
        Ok(v) => v.min(100_000),
        Err(e) => {
            app.transfer_state.result_msg = e;
            return;
        }
    };

    let mut node = app.node.lock().unwrap();
    let wallet_path = node.core.cfg.wallet_path.clone();

    let wallet = match WalletFile::load(&wallet_path) {
        Ok(w) => w,
        Err(e) => {
            app.transfer_state.result_msg = format!("Failed to load wallet: {}", e);
            return;
        }
    };

    let from_addr = match wallet.address() {
        Ok(a) => a,
        Err(e) => {
            app.transfer_state.result_msg = format!("Wallet address error: {}", e);
            return;
        }
    };

    let account = node.core.store.get_account(&from_addr).unwrap_or_default();

    let tx = Transaction {
        from: from_addr,
        to,
        value: amount,
        gas_limit,
        max_gas_price,
        mining_tip: fee,
        expiration_height: None,
        payload: vec![],
        account_index: account.account_index,
        nonce: 0,
        public_key: vec![],
        signature: vec![],
    };

    let tx = match sign_with_optional_grind(tx, node.core.cfg.chain_id, &wallet, grind) {
        Ok(t) => t,
        Err(e) => {
            app.transfer_state.result_msg = format!("Sign error: {}", e);
            return;
        }
    };

    let peers = node.peers.iter().cloned().collect::<Vec<_>>();
    let result = node.core.submit_tx(tx.clone(), peers.len());
    if result.accepted {
        tokio::spawn(gossip_tx(peers, tx));
        app.transfer_state.result_msg = format!(
            "Success! Tx Hash: {}",
            crate::crypto::hex_hash(&result.tx_hash.unwrap())
        );
        app.transfer_state.to_input.reset();
        app.transfer_state.amount_input.reset();
        app.transfer_state.focus = 6;
    } else {
        app.transfer_state.result_msg = format!(
            "Failed: {}",
            result.error.unwrap_or_else(|| "Unknown error".to_string())
        );
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
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(3),
        ])
        .split(area);

    let focused_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let normal_style = Style::default().fg(Color::White);

    // To Address
    let to_style = if app.transfer_state.focus == 0 {
        focused_style
    } else {
        normal_style
    };
    let to_widget = Paragraph::new(app.transfer_state.to_input.value())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" To (Address or Address Book Name) "),
        )
        .style(to_style);
    f.render_widget(to_widget, chunks[0]);

    // Amount
    let amount_style = if app.transfer_state.focus == 1 {
        focused_style
    } else {
        normal_style
    };
    let amount_widget = Paragraph::new(app.transfer_state.amount_input.value())
        .block(Block::default().borders(Borders::ALL).title(" Amount "))
        .style(field_style(
            amount_style,
            app.transfer_state
                .amount_input
                .value()
                .trim()
                .parse::<u128>()
                .is_ok(),
        ));
    f.render_widget(amount_widget, chunks[1]);

    render_num(
        f,
        chunks[2],
        " Gas Limit ",
        app.transfer_state.gas_limit_input.value(),
        app.transfer_state.focus == 2,
        false,
    );
    render_num(
        f,
        chunks[3],
        " Max Gas Price ",
        app.transfer_state.max_gas_price_input.value(),
        app.transfer_state.focus == 3,
        false,
    );

    // Fee
    let fee_style = if app.transfer_state.focus == 4 {
        focused_style
    } else {
        normal_style
    };
    let fee_widget = Paragraph::new(app.transfer_state.fee_input.value())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Mining Tip (Fee) "),
        )
        .style(field_style(
            fee_style,
            app.transfer_state
                .fee_input
                .value()
                .trim()
                .parse::<u128>()
                .is_ok(),
        ));
    f.render_widget(fee_widget, chunks[4]);

    render_num(
        f,
        chunks[5],
        " Grind Nonces (0-100000) ",
        app.transfer_state.grind_input.value(),
        app.transfer_state.focus == 5,
        true,
    );

    // Submit Button
    let btn_style = if app.transfer_state.focus == 6 {
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    };
    let btn_widget = Paragraph::new(" [ SUBMIT TRANSFER ] ")
        .block(Block::default().borders(Borders::ALL))
        .style(btn_style);
    f.render_widget(btn_widget, chunks[6]);

    // Result Message
    let res_widget = Paragraph::new(app.transfer_state.result_msg.clone())
        .block(Block::default().borders(Borders::ALL).title(" Result "))
        .style(Style::default().fg(Color::Green));
    f.render_widget(res_widget, chunks[7]);
}

fn render_num(
    f: &mut Frame,
    area: Rect,
    title: &str,
    value: &str,
    focused: bool,
    allow_zero: bool,
) {
    let style = if focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let valid = value
        .trim()
        .parse::<u128>()
        .is_ok_and(|v| allow_zero || v > 0);
    f.render_widget(
        Paragraph::new(value.to_string())
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(field_style(style, valid)),
        area,
    );
}

fn field_style(style: Style, valid: bool) -> Style {
    if valid {
        style
    } else {
        style.fg(Color::Red)
    }
}
