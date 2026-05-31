use crate::tui::address_book::AbiEntry;
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
        KeyCode::Down | KeyCode::Tab => {
            app.abi_wizard_state.focus = (app.abi_wizard_state.focus + 1) % 6
        }
        KeyCode::Up => app.abi_wizard_state.focus = (app.abi_wizard_state.focus + 5) % 6,
        KeyCode::Enter => {
            if app.abi_wizard_state.focus == 5 {
                save_abi(app);
            } else {
                app.abi_wizard_state.focus = (app.abi_wizard_state.focus + 1) % 6;
            }
        }
        _ => match app.abi_wizard_state.focus {
            0 => input(&mut app.abi_wizard_state.entry_input, key),
            1 => input(&mut app.abi_wizard_state.method_id_input, key),
            2 => input(&mut app.abi_wizard_state.name_input, key),
            3 => input(&mut app.abi_wizard_state.args_input, key),
            4 => input(&mut app.abi_wizard_state.rets_input, key),
            _ => {}
        },
    }
}

fn input(input: &mut tui_input::Input, key: KeyEvent) {
    input.handle_event(&crossterm::event::Event::Key(key));
}

fn save_abi(app: &mut App) {
    let entry_name = app.abi_wizard_state.entry_input.value().trim().to_string();
    if entry_name.is_empty() {
        app.abi_wizard_state.result_msg = "address book entry name is required".to_string();
        return;
    }
    let method_id = match app
        .abi_wizard_state
        .method_id_input
        .value()
        .trim()
        .parse::<u16>()
    {
        Ok(v) => v,
        Err(_) => {
            app.abi_wizard_state.result_msg = "method id must be a u16".to_string();
            return;
        }
    };
    let name = app.abi_wizard_state.name_input.value().trim().to_string();
    if name.is_empty() {
        app.abi_wizard_state.result_msg = "method name is required".to_string();
        return;
    }
    let args = match app.abi_wizard_state.args_input.value().trim().parse::<u8>() {
        Ok(v) => v,
        Err(_) => {
            app.abi_wizard_state.result_msg = "args must be 0-255".to_string();
            return;
        }
    };
    let rets = match app.abi_wizard_state.rets_input.value().trim().parse::<u8>() {
        Ok(v) => v,
        Err(_) => {
            app.abi_wizard_state.result_msg = "rets must be 0-255".to_string();
            return;
        }
    };

    let Some(entry) = app.address_book.entries.get_mut(&entry_name) else {
        app.abi_wizard_state.result_msg =
            "entry does not exist; add it in Address Book first".to_string();
        return;
    };
    entry.abis.insert(
        method_id.to_string(),
        AbiEntry {
            name: name.clone(),
            args,
            rets,
        },
    );
    let path = app
        .node
        .lock()
        .unwrap()
        .core
        .cfg
        .config_dir
        .join("address_book.toml");
    match app.address_book.save(path) {
        Ok(()) => {
            app.abi_wizard_state.result_msg =
                format!("saved ABI {method_id}: {name} on {entry_name}");
            app.abi_wizard_state.method_id_input.reset();
            app.abi_wizard_state.name_input.reset();
            app.abi_wizard_state.focus = 1;
        }
        Err(err) => app.abi_wizard_state.result_msg = format!("save failed: {err}"),
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
            Constraint::Min(5),
        ])
        .split(area);

    let focused = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(Color::White);
    let style = |idx| {
        if app.abi_wizard_state.focus == idx {
            focused
        } else {
            normal
        }
    };

    render_input(
        f,
        chunks[0],
        " Address Book Entry Name ",
        app.abi_wizard_state.entry_input.value(),
        style(0),
    );
    render_input(
        f,
        chunks[1],
        " Method ID (u16) ",
        app.abi_wizard_state.method_id_input.value(),
        style(1),
    );
    render_input(
        f,
        chunks[2],
        " Method Name ",
        app.abi_wizard_state.name_input.value(),
        style(2),
    );
    render_input(
        f,
        chunks[3],
        " Arg Count ",
        app.abi_wizard_state.args_input.value(),
        style(3),
    );
    render_input(
        f,
        chunks[4],
        " Return Count ",
        app.abi_wizard_state.rets_input.value(),
        style(4),
    );

    let btn_style = if app.abi_wizard_state.focus == 5 {
        Style::default()
            .bg(Color::Green)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    };
    f.render_widget(
        Paragraph::new(" [ SAVE ABI MAPPING ] ")
            .block(Block::default().borders(Borders::ALL))
            .style(btn_style),
        chunks[5],
    );

    let mut help = String::from("ABI mappings let Contract Calls use method names instead of raw IDs.\nAdd the contract address in tab 6 first, then save mappings here.\n\n");
    if !app.address_book.entries.is_empty() {
        help.push_str("Known entries: ");
        let mut names = app.address_book.entries.keys().cloned().collect::<Vec<_>>();
        names.sort();
        help.push_str(&names.join(", "));
        help.push_str("\n");
    }
    help.push_str(&app.abi_wizard_state.result_msg);
    f.render_widget(
        Paragraph::new(help)
            .block(Block::default().borders(Borders::ALL).title(" ABI Wizard "))
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
