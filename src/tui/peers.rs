use crate::node::normalize_peer_url;
use crate::tui::app::App;
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
        KeyCode::Down | KeyCode::Tab => app.peers_state.focus = (app.peers_state.focus + 1) % 2,
        KeyCode::Up => app.peers_state.focus = (app.peers_state.focus + 1) % 2,
        KeyCode::Enter => {
            if app.peers_state.focus == 1 {
                add_peer(app);
            } else {
                app.peers_state.focus = 1;
            }
        }
        _ if app.peers_state.focus == 0 => {
            app.peers_state
                .peer_input
                .handle_event(&crossterm::event::Event::Key(key));
        }
        _ => {}
    }
}

fn add_peer(app: &mut App) {
    let text = app.peers_state.peer_input.value().trim();
    if text.is_empty() {
        app.peers_state.result_msg = "enter a peer URL or host:port".to_string();
        app.peers_state.focus = 0;
        return;
    }
    let peer = normalize_peer_url(text);
    if !peer.starts_with("http://") && !peer.starts_with("https://") {
        app.peers_state.result_msg = "peer must be http(s) or host:port".to_string();
        return;
    }

    let save_result = {
        let mut node = app.node.lock().unwrap();
        node.peers.insert(peer.clone());
        if !node.core.cfg.peers.contains(&peer) {
            node.core.cfg.peers.push(peer.clone());
        }
        node.core.cfg.save(&app.config_path)
    };

    match save_result {
        Ok(()) => {
            app.peers_state.result_msg =
                format!("added peer {peer}; sync loop will connect within a few seconds");
            app.peers_state.peer_input.reset();
            app.peers_state.focus = 1;
        }
        Err(err) => {
            app.peers_state.result_msg = format!("added in memory, config save failed: {err}");
        }
    }
}

pub fn draw(app: &mut App, f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(4),
        ])
        .split(area);

    let (listen_addr, genesis, peers, discovered, statuses, logs) = {
        let node = app.node.lock().unwrap();
        let mut peers = node.peers.iter().cloned().collect::<Vec<_>>();
        peers.sort();
        let mut discovered = node.discovered_peers.iter().cloned().collect::<Vec<_>>();
        discovered.sort();
        let genesis = node
            .core
            .store
            .get_block_by_height(0)
            .ok()
            .flatten()
            .and_then(|block| block.hash().ok())
            .map(|hash| crate::crypto::hex_hash(&hash))
            .unwrap_or_else(|| "unknown".to_string());
        (
            node.core.cfg.listen_addr.clone(),
            genesis,
            peers,
            discovered,
            node.peer_status.clone(),
            node.discovery_logs.clone(),
        )
    };

    let mut text = format!(
        "Listen: {listen_addr}\nGenesis: {genesis}\nLAN discovery: UDP broadcast + ARP/subnet scan on HTTP port 12367\n\nPeers:\n"
    );
    if peers.is_empty() {
        text.push_str("  none yet\n");
    } else {
        for peer in &peers {
            let tag = if discovered.contains(peer) {
                "discovered"
            } else {
                "manual"
            };
            let status = statuses
                .get(peer)
                .map(|status| {
                    if status.ok {
                        format!(
                            "online height={} seen={}s ago head={} genesis={}",
                            status.height.unwrap_or(0),
                            status.last_seen.map(|t| t.elapsed().as_secs()).unwrap_or(0),
                            short_hash(status.head.as_deref()),
                            short_hash(status.genesis.as_deref())
                        )
                    } else {
                        format!(
                            "offline: {}",
                            status.last_error.as_deref().unwrap_or("unknown error")
                        )
                    }
                })
                .unwrap_or_else(|| "pending connect".to_string());
            text.push_str(&format!("  {peer} [{tag}] {status}\n"));
        }
    }
    text.push_str("\nDiscovery log:\n");
    text.push_str(&logs.join("\n"));

    f.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(" Peers "))
            .style(Style::default().fg(Color::White)),
        chunks[0],
    );

    let input_style = if app.peers_state.focus == 0 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    f.render_widget(
        Paragraph::new(app.peers_state.peer_input.value())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Remote Peer URL or host:port "),
            )
            .style(input_style),
        chunks[1],
    );

    let btn_style = if app.peers_state.focus == 1 {
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    };
    f.render_widget(
        Paragraph::new(" [ ADD REMOTE PEER ] ")
            .block(Block::default().borders(Borders::ALL))
            .style(btn_style),
        chunks[2],
    );

    f.render_widget(
        Paragraph::new(app.peers_state.result_msg.clone())
            .block(Block::default().borders(Borders::ALL).title(" Status "))
            .style(Style::default().fg(Color::Green)),
        chunks[3],
    );
}

fn short_hash(hash: Option<&str>) -> String {
    hash.map(|hash| hash.chars().take(12).collect())
        .unwrap_or_else(|| "unknown".to_string())
}
