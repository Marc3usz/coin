use crate::crypto::decode_hash;
use crate::tui::app::App;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tui_input::backend::crossterm::EventHandler;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AddressBook {
    #[serde(default)]
    pub entries: HashMap<String, AddressEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddressEntry {
    pub address: String,
    #[serde(default)]
    pub abis: HashMap<String, AbiEntry>,
}

impl AddressEntry {
    pub fn method_id_for_name(&self, name: &str) -> Option<u16> {
        self.abis.iter().find_map(|(id, abi)| {
            if abi.name == name {
                id.parse::<u16>().ok()
            } else {
                None
            }
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbiEntry {
    pub name: String,
    pub args: u8,
    pub rets: u8,
    #[serde(default)]
    pub params: Vec<AbiParam>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbiParam {
    pub name: String,
    pub ty: String,
}

impl AddressBook {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

pub fn handle_event(app: &mut App, key: KeyEvent) {
    if app.address_book_state.editing {
        match key.code {
            KeyCode::Tab | KeyCode::Down | KeyCode::Up => {
                app.address_book_state.selected = (app.address_book_state.selected + 1) % 2;
            }
            KeyCode::Enter => save_entry(app),
            KeyCode::Esc => {
                app.address_book_state.editing = false;
                app.address_book_state.result_msg = "edit cancelled".to_string();
            }
            _ => {
                if app.address_book_state.selected == 0 {
                    app.address_book_state
                        .name_input
                        .handle_event(&crossterm::event::Event::Key(key));
                } else {
                    app.address_book_state
                        .address_input
                        .handle_event(&crossterm::event::Event::Key(key));
                }
            }
        }
        return;
    }

    match key.code {
        KeyCode::Down => {
            let len = app.address_book.entries.len().max(1);
            app.address_book_state.selected = (app.address_book_state.selected + 1) % len;
        }
        KeyCode::Up => {
            let len = app.address_book.entries.len().max(1);
            app.address_book_state.selected = (app.address_book_state.selected + len - 1) % len;
        }
        KeyCode::Char('a') => {
            app.address_book_state.editing = true;
            app.address_book_state.selected = 0;
            app.address_book_state.name_input.reset();
            app.address_book_state.address_input.reset();
            app.address_book_state.result_msg = "adding entry".to_string();
        }
        KeyCode::Char('e') => edit_selected(app),
        KeyCode::Char('x') | KeyCode::Delete => delete_selected(app),
        KeyCode::Char('r') => reload(app),
        _ => {}
    }
}

fn address_book_path(app: &App) -> std::path::PathBuf {
    app.node
        .lock()
        .unwrap()
        .core
        .cfg
        .config_dir
        .join("address_book.toml")
}

fn save_entry(app: &mut App) {
    let name = app.address_book_state.name_input.value().trim().to_string();
    let address = app
        .address_book_state
        .address_input
        .value()
        .trim()
        .to_string();
    if name.is_empty() {
        app.address_book_state.result_msg = "name cannot be empty".to_string();
        return;
    }
    if decode_hash(&address).is_err() {
        app.address_book_state.result_msg = "address must be a 32-byte hex address".to_string();
        return;
    }

    let old_abis = app
        .address_book
        .entries
        .get(&name)
        .map(|e| e.abis.clone())
        .unwrap_or_default();
    app.address_book.entries.insert(
        name.clone(),
        AddressEntry {
            address,
            abis: old_abis,
        },
    );
    match app.address_book.save(address_book_path(app)) {
        Ok(()) => {
            app.address_book_state.editing = false;
            app.address_book_state.result_msg = format!("saved entry {name}");
        }
        Err(err) => app.address_book_state.result_msg = format!("save failed: {err}"),
    }
}

fn sorted_names(app: &App) -> Vec<String> {
    let mut names = app.address_book.entries.keys().cloned().collect::<Vec<_>>();
    names.sort();
    names
}

fn edit_selected(app: &mut App) {
    let names = sorted_names(app);
    if names.is_empty() {
        app.address_book_state.result_msg = "no entry selected".to_string();
        return;
    }
    let name = names[app.address_book_state.selected.min(names.len() - 1)].clone();
    if let Some(entry) = app.address_book.entries.get(&name) {
        app.address_book_state.name_input = tui_input::Input::default().with_value(name);
        app.address_book_state.address_input =
            tui_input::Input::default().with_value(entry.address.clone());
        app.address_book_state.selected = 0;
        app.address_book_state.editing = true;
    }
}

fn delete_selected(app: &mut App) {
    let names = sorted_names(app);
    if names.is_empty() {
        app.address_book_state.result_msg = "no entry selected".to_string();
        return;
    }
    let name = names[app.address_book_state.selected.min(names.len() - 1)].clone();
    app.address_book.entries.remove(&name);
    app.address_book_state.selected = app.address_book_state.selected.saturating_sub(1);
    match app.address_book.save(address_book_path(app)) {
        Ok(()) => app.address_book_state.result_msg = format!("deleted entry {name}"),
        Err(err) => {
            app.address_book_state.result_msg =
                format!("delete saved in memory but file save failed: {err}")
        }
    }
}

fn reload(app: &mut App) {
    match AddressBook::load(address_book_path(app)) {
        Ok(book) => {
            app.address_book = book;
            app.address_book_state.result_msg = "reloaded address book".to_string();
        }
        Err(err) => app.address_book_state.result_msg = format!("reload failed: {err}"),
    }
}

pub fn draw(app: &mut App, f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(7),
            Constraint::Length(3),
        ])
        .split(area);

    let names = sorted_names(app);
    let mut text = String::new();
    if names.is_empty() {
        text.push_str("No entries. Press a to add one.\n");
    } else {
        for (idx, name) in names.iter().enumerate() {
            let marker = if idx == app.address_book_state.selected {
                ">"
            } else {
                " "
            };
            if let Some(entry) = app.address_book.entries.get(name) {
                text.push_str(&format!("{marker} {name}\n  {}\n", entry.address));
                for (id, abi) in &entry.abis {
                    let params = abi
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, p.ty))
                        .collect::<Vec<_>>()
                        .join(", ");
                    text.push_str(&format!(
                        "    ABI {id}: {}({}) args={} rets={}\n",
                        abi.name, params, abi.args, abi.rets
                    ));
                }
            }
        }
    }
    text.push_str("\nKeys: a add | e edit | x/delete remove | r reload | 7 ABI wizard");
    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Address Book "),
            )
            .style(Style::default().fg(Color::White)),
        chunks[0],
    );

    let edit_text = if app.address_book_state.editing {
        format!(
            "{} Name: {}\n{} Address: {}\nEnter saves, Tab changes field, Esc cancels",
            if app.address_book_state.selected == 0 {
                ">"
            } else {
                " "
            },
            app.address_book_state.name_input.value(),
            if app.address_book_state.selected == 1 {
                ">"
            } else {
                " "
            },
            app.address_book_state.address_input.value()
        )
    } else {
        "Not editing. Press a or e.".to_string()
    };
    f.render_widget(
        Paragraph::new(edit_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Entry Editor "),
            )
            .style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        chunks[1],
    );

    f.render_widget(
        Paragraph::new(app.address_book_state.result_msg.clone())
            .block(Block::default().borders(Borders::ALL).title(" Status "))
            .style(Style::default().fg(Color::Green)),
        chunks[2],
    );
}
