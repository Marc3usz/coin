use crate::crypto::hex_hash;
use crate::tui::app::App;
use crate::types::{RETARGET_BLOCKS, TARGET_BLOCK_SECONDS};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame,
};

pub fn handle_event(app: &mut App, key: KeyEvent) {
    if key.code == KeyCode::Char('m') {
        let mut node = app.node.lock().unwrap();
        node.core.cfg.mine = !node.core.cfg.mine;
        node.mining.enabled = node.core.cfg.mine;
        let state = if node.core.cfg.mine {
            "enabled"
        } else {
            "paused"
        };
        node.mining.push_log(format!("mining {state} by TUI"));
    }
}

pub fn draw(app: &mut App, f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),
            Constraint::Length(3),
            Constraint::Min(6),
        ])
        .split(area);

    let (
        height,
        head_hash,
        nbits,
        mempool_size,
        peers,
        mining,
        in_progress,
        mined_blocks,
        last_hash,
        last_error,
        logs,
        listen_addr,
        miner_address,
    ) = {
        let node = app.node.lock().unwrap();
        let head = node.core.head().unwrap();
        (
            head.header.height,
            hex_hash(&head.hash().unwrap()),
            head.header.nbits,
            node.core.mempool.all().len(),
            node.peers.len(),
            node.core.cfg.mine,
            node.mining.in_progress,
            node.mining.mined_blocks,
            node.mining
                .last_hash
                .clone()
                .unwrap_or_else(|| "none yet".to_string()),
            node.mining
                .last_error
                .clone()
                .unwrap_or_else(|| "none".to_string()),
            node.mining.logs.clone(),
            node.core.cfg.listen_addr.clone(),
            node.core
                .cfg
                .miner_address
                .map(|a| hex_hash(&a))
                .unwrap_or_else(|| "not set".to_string()),
        )
    };

    let status = if mining {
        if in_progress {
            "hashing candidate"
        } else if mempool_size == 0 {
            "idle/no tx; mining empty blocks on cadence"
        } else {
            "idle; txs queued for next block"
        }
    } else {
        "paused"
    };
    let next_retarget_in = (RETARGET_BLOCKS - ((height + 1) % RETARGET_BLOCKS)) % RETARGET_BLOCKS;
    let retarget_window = (RETARGET_BLOCKS - 1).max(1) * TARGET_BLOCK_SECONDS;
    let text = format!(
        "Node: {} | Peers: {} | Mempool: {} txs\nChain Height: {} | nbits: 0x{:08x}\nHead: {}\nMiner: {}\nMining: {} (press m to toggle) | Blocks mined this session: {}\nRetarget: every {} blocks, observed span ~{}s; next in {} blocks\nLast block: {}\nLast error: {}",
        listen_addr, peers, mempool_size, height, nbits, head_hash, miner_address, status, mined_blocks, RETARGET_BLOCKS, retarget_window, next_retarget_in, last_hash, last_error
    );

    f.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(" Dashboard "))
            .style(Style::default().fg(Color::White)),
        chunks[0],
    );

    let ratio = if mining && in_progress {
        0.70
    } else if mining {
        0.18
    } else {
        0.0
    };
    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Mining Activity "),
        )
        .gauge_style(if mining {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .ratio(ratio)
        .label(status);
    f.render_widget(gauge, chunks[1]);

    let log_text = if logs.is_empty() {
        "no mining logs yet".to_string()
    } else {
        logs.join("\n")
    };
    f.render_widget(
        Paragraph::new(log_text)
            .block(Block::default().borders(Borders::ALL).title(" Mining Log "))
            .style(Style::default().fg(Color::Cyan)),
        chunks[2],
    );
}
