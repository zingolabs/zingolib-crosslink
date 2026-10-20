//! Maps terminal events onto reducer actions.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{Action, Key};

pub fn map_event(event: Event) -> Option<Action> {
    match event {
        Event::Key(key) => map_key(key).map(Action::Key),
        Event::Paste(text) => Some(Action::Paste(text)),
        Event::Resize(w, h) => Some(Action::Resize(w, h)),
        _ => None,
    }
}

fn map_key(key: KeyEvent) -> Option<Key> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char('c') if ctrl => Key::CtrlC,
        KeyCode::Char(c) if ctrl => match c {
            'u' => Key::PageUp,
            'd' => Key::PageDown,
            _ => Key::Other,
        },
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        _ => Key::Other,
    })
}
