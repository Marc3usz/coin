use crate::crypto::{decode_hash, hex_hash, Address, Hash};
use crate::tui::app::{App, SearchMode};
use crate::types::{Account, Block, BlockReceipts, Transaction};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block as UiBlock, Borders, Paragraph, Wrap},
    Frame,
};
use tui_input::backend::crossterm::EventHandler;

const MAX_RESULTS: usize = 80;
const MAX_RENDERED_LINES: usize = 500;

pub fn handle_event(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Tab => {
            app.search_state.mode = app.search_state.mode.next();
            app.search_state.status = format!("Mode: {}", app.search_state.mode.label());
        }
        KeyCode::Enter => run_search(app),
        KeyCode::Down => app.search_state.scroll = app.search_state.scroll.saturating_add(1),
        KeyCode::Up => app.search_state.scroll = app.search_state.scroll.saturating_sub(1),
        KeyCode::PageDown => app.search_state.scroll = app.search_state.scroll.saturating_add(10),
        KeyCode::PageUp => app.search_state.scroll = app.search_state.scroll.saturating_sub(10),
        KeyCode::Char('c')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) => {}
        KeyCode::Esc => {}
        _ => {
            app.search_state
                .query_input
                .handle_event(&crossterm::event::Event::Key(key));
        }
    }
}

fn run_search(app: &mut App) {
    let query = app.search_state.query_input.value().trim().to_string();
    app.search_state.scroll = 0;
    if query.is_empty() {
        app.search_state.results = vec!["Enter a search query first.".to_string()];
        app.search_state.status = "Empty query".to_string();
        return;
    }

    let started = std::time::Instant::now();
    let mode = app.search_state.mode;
    let result = {
        let node = app.node.lock().unwrap();
        let height = node.core.store.height().unwrap_or(0);
        let mut search = ChainSearch::new(&query, height);
        match mode {
            SearchMode::Auto => search.auto(&node),
            SearchMode::Address => search.address(&node),
            SearchMode::Contract => search.contract(&node),
            SearchMode::Transaction => search.transaction(&node),
            SearchMode::Block => search.block(&node),
            SearchMode::Logs => search.logs(&node),
        }
        search.finish()
    };

    app.search_state.results = if result.lines.is_empty() {
        vec![format!("No results for '{}'.", query)]
    } else {
        result.lines
    };
    app.search_state.status = format!(
        "{} mode | scanned {} blocks | {} result sections | {}ms",
        mode.label(),
        result.scanned_blocks,
        app.search_state.results.len(),
        started.elapsed().as_millis()
    );
}

struct SearchResult {
    lines: Vec<String>,
    scanned_blocks: u64,
}

struct ChainSearch<'a> {
    query: &'a str,
    query_lower: String,
    height: u64,
    lines: Vec<String>,
    scanned_blocks: u64,
}

impl<'a> ChainSearch<'a> {
    fn new(query: &'a str, height: u64) -> Self {
        Self {
            query,
            query_lower: query.to_ascii_lowercase(),
            height,
            lines: Vec::new(),
            scanned_blocks: 0,
        }
    }

    fn finish(self) -> SearchResult {
        SearchResult {
            lines: self.lines,
            scanned_blocks: self.scanned_blocks,
        }
    }

    fn auto(&mut self, node: &crate::node::NodeServer) {
        if self.query.parse::<u64>().is_ok() {
            self.block(node);
        }
        if decode_hash(self.query).is_ok() {
            self.block(node);
            self.transaction(node);
            self.address(node);
            self.contract(node);
        }
        self.logs(node);
        self.scan_chain(node, ScanKind::General);
        self.dedup();
    }

    fn address(&mut self, node: &crate::node::NodeServer) {
        let Ok(address) = decode_hash(self.query) else {
            self.push("Address", "Query is not a 32-byte hex address.");
            return;
        };
        let account = node.core.store.get_account(&address).unwrap_or_default();
        self.push("Address", &format_account(&address, &account));
        if let Some(code_hash) = account.code_hash {
            self.push(
                "Contract",
                &format!("Address has code hash {}", hex_hash(&code_hash)),
            );
        }
        self.scan_chain(node, ScanKind::Address(address));
        self.scan_mempool(node, Some(address));
    }

    fn contract(&mut self, node: &crate::node::NodeServer) {
        let Ok(address) = decode_hash(self.query) else {
            self.push("Contract", "Query is not a 32-byte contract address.");
            return;
        };
        let account = node.core.store.get_account(&address).unwrap_or_default();
        match account.code_hash {
            Some(code_hash) => {
                let code_size = node
                    .core
                    .store
                    .code(&code_hash)
                    .ok()
                    .flatten()
                    .map(|c| c.len())
                    .unwrap_or(0);
                self.push(
                    "Contract",
                    &format!(
                        "address: {}\ncode hash: {}\ncode bytes: {}\nbalance: {}\naccount index: {}",
                        hex_hash(&address),
                        hex_hash(&code_hash),
                        code_size,
                        account.balance,
                        account.account_index
                    ),
                );
                self.scan_chain(node, ScanKind::Address(address));
            }
            None => self.push("Contract", "Account exists as non-contract or is empty."),
        }
    }

    fn transaction(&mut self, node: &crate::node::NodeServer) {
        let Ok(hash) = decode_hash(self.query) else {
            self.push("Transaction", "Query is not a 32-byte tx hash.");
            return;
        };
        for tx in node.core.mempool.all() {
            if tx.hash().ok().as_ref() == Some(&hash) {
                self.push("Mempool Transaction", &format_tx(&tx));
            }
        }
        self.scan_chain(node, ScanKind::Tx(hash));
    }

    fn block(&mut self, node: &crate::node::NodeServer) {
        if let Ok(height) = self.query.parse::<u64>() {
            match node.core.store.get_block_by_height(height).ok().flatten() {
                Some(block) => self.push_block(node, &block),
                None => self.push("Block", &format!("No block at height {height}")),
            }
            return;
        }
        let Ok(hash) = decode_hash(self.query) else {
            self.push(
                "Block",
                "Query is neither a height nor a 32-byte block hash.",
            );
            return;
        };
        match node.core.store.get_block_by_hash(&hash).ok().flatten() {
            Some(block) => self.push_block(node, &block),
            None => self.push("Block", &format!("No block with hash {}", hex_hash(&hash))),
        }
    }

    fn logs(&mut self, node: &crate::node::NodeServer) {
        self.scan_chain(node, ScanKind::Logs);
    }

    fn scan_mempool(&mut self, node: &crate::node::NodeServer, address: Option<Address>) {
        for tx in node.core.mempool.all() {
            if address.is_none_or(|a| tx.from == a || tx.to == Some(a)) {
                self.push("Mempool", &format_tx(&tx));
            }
        }
    }

    fn scan_chain(&mut self, node: &crate::node::NodeServer, kind: ScanKind) {
        for height in 0..=self.height {
            if self.lines.len() >= MAX_RESULTS {
                self.push(
                    "Search",
                    "Result cap reached; narrow the query for more precision.",
                );
                return;
            }
            let Some(block) = node.core.store.get_block_by_height(height).ok().flatten() else {
                continue;
            };
            self.scanned_blocks += 1;
            let block_hash = block.hash().unwrap_or([0; 32]);
            let receipts = node.core.store.get_receipts(&block_hash).ok().flatten();
            match kind {
                ScanKind::Address(address) => {
                    self.match_address_block(&block, receipts.as_ref(), address)
                }
                ScanKind::Tx(hash) => self.match_tx_block(&block, receipts.as_ref(), hash),
                ScanKind::Logs => self.match_logs(&block, receipts.as_ref()),
                ScanKind::General => self.match_general(&block, receipts.as_ref()),
            }
        }
    }

    fn match_address_block(
        &mut self,
        block: &Block,
        receipts: Option<&BlockReceipts>,
        address: Address,
    ) {
        if block.header.miner_address == address {
            self.push("Miner Match", &format_block_summary(block));
        }
        for (idx, tx) in block.transactions.iter().enumerate() {
            if tx.from == address || tx.to == Some(address) {
                self.push(
                    "Transaction Match",
                    &format!("block {} tx #{idx}\n{}", block.header.height, format_tx(tx)),
                );
            }
        }
        if let Some(receipts) = receipts {
            for receipt in &receipts.receipts {
                if receipt_contains(&receipt.events, &hex_hash(&address)) {
                    self.push(
                        "Event Match",
                        &format!(
                            "block {} receipt tx={}\n{}",
                            block.header.height,
                            hex_hash(&receipt.tx_hash),
                            format_events(&receipt.events)
                        ),
                    );
                }
            }
        }
    }

    fn match_tx_block(&mut self, block: &Block, receipts: Option<&BlockReceipts>, hash: Hash) {
        for (idx, tx) in block.transactions.iter().enumerate() {
            if tx.hash().ok().as_ref() == Some(&hash) {
                self.push(
                    "Transaction",
                    &format!(
                        "block {} tx #{idx}\nblock hash: {}\n{}",
                        block.header.height,
                        hex_hash(&block.hash().unwrap_or([0; 32])),
                        format_tx(tx)
                    ),
                );
            }
        }
        if let Some(receipts) = receipts {
            for receipt in &receipts.receipts {
                if receipt.tx_hash == hash {
                    self.push("Receipt", &format_receipt(receipt));
                }
            }
        }
    }

    fn match_logs(&mut self, block: &Block, receipts: Option<&BlockReceipts>) {
        let Some(receipts) = receipts else { return };
        for receipt in &receipts.receipts {
            let formatted = format_events(&receipt.events);
            if formatted.to_ascii_lowercase().contains(&self.query_lower)
                || hex_hash(&receipt.tx_hash).contains(&self.query_lower)
            {
                self.push(
                    "Log Match",
                    &format!(
                        "block {} tx={} success={}\n{}",
                        block.header.height,
                        hex_hash(&receipt.tx_hash),
                        receipt.success,
                        formatted
                    ),
                );
            }
        }
    }

    fn match_general(&mut self, block: &Block, receipts: Option<&BlockReceipts>) {
        let block_hash = hex_hash(&block.hash().unwrap_or([0; 32]));
        if block_hash.contains(&self.query_lower)
            || block.header.height.to_string() == self.query
            || hex_hash(&block.header.miner_address).contains(&self.query_lower)
        {
            self.push("Block Match", &format_block_summary(block));
        }
        for tx in &block.transactions {
            let tx_text = format_tx(tx).to_ascii_lowercase();
            if tx_text.contains(&self.query_lower) {
                self.push(
                    "Transaction Text Match",
                    &format!("block {}\n{}", block.header.height, format_tx(tx)),
                );
            }
        }
        if let Some(receipts) = receipts {
            for receipt in &receipts.receipts {
                let text = format_receipt(receipt).to_ascii_lowercase();
                if text.contains(&self.query_lower) {
                    self.push(
                        "Receipt Text Match",
                        &format!("block {}\n{}", block.header.height, format_receipt(receipt)),
                    );
                }
            }
        }
    }

    fn push_block(&mut self, node: &crate::node::NodeServer, block: &Block) {
        let block_hash = block.hash().unwrap_or([0; 32]);
        let receipts = node.core.store.get_receipts(&block_hash).ok().flatten();
        let mut text = format_block_summary(block);
        text.push_str(&format!("\ntransactions: {}", block.transactions.len()));
        if !block.transactions.is_empty() {
            text.push_str("\n\nTransactions:");
            for (idx, tx) in block.transactions.iter().enumerate().take(12) {
                text.push_str(&format!(
                    "\n#{idx} {}",
                    hex_hash(&tx.hash().unwrap_or([0; 32]))
                ));
            }
        }
        if let Some(receipts) = receipts {
            text.push_str(&format!("\nreceipts: {}", receipts.receipts.len()));
            for receipt in receipts.receipts.iter().take(12) {
                text.push_str(&format!(
                    "\nreceipt tx={} success={} gas={} events={}",
                    hex_hash(&receipt.tx_hash),
                    receipt.success,
                    receipt.gas_used,
                    receipt.events.len()
                ));
            }
        }
        self.push("Block", &text);
    }

    fn push(&mut self, title: &str, body: &str) {
        if self.lines.len() < MAX_RESULTS {
            self.lines.push(format!("== {title} ==\n{body}"));
        }
    }

    fn dedup(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.lines.retain(|line| seen.insert(line.clone()));
    }
}

#[derive(Clone, Copy)]
enum ScanKind {
    Address(Address),
    Tx(Hash),
    Logs,
    General,
}

fn format_account(address: &Address, account: &Account) -> String {
    format!(
        "address: {}\nbalance: {}\naccount index: {}\ncode hash: {}",
        hex_hash(address),
        account.balance,
        account.account_index,
        account
            .code_hash
            .map(|h| hex_hash(&h))
            .unwrap_or_else(|| "none".to_string())
    )
}

fn format_block_summary(block: &Block) -> String {
    format!(
        "height: {}\nhash: {}\ntime: {}\nnbits: 0x{:08x}\nnonce: {}\ntxs: {}\ngas: {}/{}\nminer: {}\ntx root: {}\nreceipt root: {}",
        block.header.height,
        hex_hash(&block.hash().unwrap_or([0; 32])),
        block.header.timestamp,
        block.header.nbits,
        block.header.nonce,
        block.header.tx_count,
        block.header.gas_used,
        block.header.block_gas_limit,
        hex_hash(&block.header.miner_address),
        hex_hash(&block.header.tx_root),
        hex_hash(&block.header.receipt_root)
    )
}

fn format_tx(tx: &Transaction) -> String {
    format!(
        "hash: {}\nfrom: {}\nto: {}\nvalue: {}\ngas limit: {}\nmax gas price: {}\nmining tip: {}\naccount index: {}\nnonce: {}\npayload bytes: {}",
        hex_hash(&tx.hash().unwrap_or([0; 32])),
        hex_hash(&tx.from),
        tx.to.map(|a| hex_hash(&a)).unwrap_or_else(|| "contract deploy".to_string()),
        tx.value,
        tx.gas_limit,
        tx.max_gas_price,
        tx.mining_tip,
        tx.account_index,
        tx.nonce,
        tx.payload.len()
    )
}

fn format_receipt(receipt: &crate::types::Receipt) -> String {
    format!(
        "tx: {}\nsuccess: {}\ngas used: {}\ngas burned: {}\nmining tip paid: {}\nexit: {}\nevents:\n{}",
        hex_hash(&receipt.tx_hash),
        receipt.success,
        receipt.gas_used,
        receipt.gas_burned,
        receipt.mining_tip_paid,
        receipt.exit_reason,
        format_events(&receipt.events)
    )
}

fn format_events(events: &[(u16, Vec<String>)]) -> String {
    if events.is_empty() {
        return "  none".to_string();
    }
    events
        .iter()
        .map(|(idx, values)| format!("  event {idx}: {}", values.join(", ")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn receipt_contains(events: &[(u16, Vec<String>)], needle: &str) -> bool {
    events
        .iter()
        .any(|(_, values)| values.iter().any(|v| v.contains(needle)))
}

pub fn draw(app: &mut App, f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(8),
        ])
        .split(area);

    f.render_widget(
        Paragraph::new(app.search_state.query_input.value().to_string())
            .block(
                UiBlock::default()
                    .borders(Borders::ALL)
                    .title(" Search Query "),
            )
            .style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(format!(
            "Mode: {} | Enter search | Tab mode | Up/Down/Page scroll | Supports height, hashes, addresses, txs, blocks, receipts, logs",
            app.search_state.mode.label()
        ))
        .block(UiBlock::default().borders(Borders::ALL).title(" Controls "))
        .style(Style::default().fg(Color::Cyan)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(app.search_state.status.clone())
            .block(UiBlock::default().borders(Borders::ALL).title(" Status "))
            .style(Style::default().fg(Color::Green)),
        chunks[2],
    );

    let text = app
        .search_state
        .results
        .join("\n\n")
        .lines()
        .skip(app.search_state.scroll)
        .take(MAX_RENDERED_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    f.render_widget(
        Paragraph::new(text)
            .block(UiBlock::default().borders(Borders::ALL).title(" Results "))
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false }),
        chunks[3],
    );
}
