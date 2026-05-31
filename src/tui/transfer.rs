use crate::crypto::decode_hash;
use crate::tui::app::App;
use crate::types::Transaction;
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
        KeyCode::Down => {
            app.transfer_state.focus = (app.transfer_state.focus + 1) % 4;
        }
        KeyCode::Up => {
            app.transfer_state.focus = (app.transfer_state.focus + 3) % 4;
        }
        KeyCode::Enter => {
            if app.transfer_state.focus == 3 {
                submit_transfer(app);
            } else {
                app.transfer_state.focus = (app.transfer_state.focus + 1) % 4;
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
                    .fee_input
                    .handle_event(&crossterm::event::Event::Key(key));
            }
            _ => {}
        },
    }
}

fn submit_transfer(app: &mut App) {
    let to_str = app.transfer_state.to_input.value().trim();
    let amount_str = app.transfer_state.amount_input.value().trim();
    let fee_str = app.transfer_state.fee_input.value().trim();

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

    let amount: u128 = match amount_str.parse() {
        Ok(v) => v,
        Err(_) => {
            app.transfer_state.result_msg = "Invalid amount".to_string();
            return;
        }
    };

    let fee: u128 = match fee_str.parse() {
        Ok(v) => v,
        Err(_) => {
            app.transfer_state.result_msg = "Invalid fee".to_string();
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
        gas_limit: 100_000,
        max_gas_price: 1000,
        mining_tip: fee,
        expiration_height: None,
        payload: vec![],
        account_index: account.account_index,
        nonce: 0,
        public_key: vec![],
        signature: vec![],
    };

    let tx = match sign_tx(tx, node.core.cfg.chain_id, &wallet) {
        Ok(t) => t,
        Err(e) => {
            app.transfer_state.result_msg = format!("Sign error: {}", e);
            return;
        }
    };

    let peers_len = node.peers.len();
    let result = node.core.submit_tx(tx, peers_len);
    if result.accepted {
        app.transfer_state.result_msg = format!(
            "Success! Tx Hash: {}",
            crate::crypto::hex_hash(&result.tx_hash.unwrap())
        );
        app.transfer_state.to_input.reset();
        app.transfer_state.amount_input.reset();
        app.transfer_state.focus = 3;
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

    // Fee
    let fee_style = if app.transfer_state.focus == 2 {
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
    f.render_widget(fee_widget, chunks[2]);

    // Submit Button
    let btn_style = if app.transfer_state.focus == 3 {
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
    f.render_widget(btn_widget, chunks[3]);

    // Result Message
    let res_widget = Paragraph::new(app.transfer_state.result_msg.clone())
        .block(Block::default().borders(Borders::ALL).title(" Result "))
        .style(Style::default().fg(Color::Green));
    f.render_widget(res_widget, chunks[4]);
}

fn field_style(style: Style, valid: bool) -> Style {
    if valid {
        style
    } else {
        style.fg(Color::Red)
    }
}
