//! Conversion helpers for terminal input events.

#[cfg(feature = "ghostty")]
use fleet_proto::terminal::{
    Key, KeyAction, KeyEvent, Modifiers, MouseButton, MouseEvent, MouseEventKind,
};
#[cfg(feature = "ghostty")]
use libghostty_vt::{key, mouse};

/// Converts Fleet modifier flags into Ghostty modifier flags.
#[cfg(feature = "ghostty")]
#[must_use]
pub fn ghostty_modifiers(modifiers: Modifiers) -> key::Mods {
    let mut result = key::Mods::empty();
    if modifiers.contains(Modifiers::SHIFT) {
        result |= key::Mods::SHIFT;
    }
    if modifiers.contains(Modifiers::CTRL) {
        result |= key::Mods::CTRL;
    }
    if modifiers.contains(Modifiers::ALT) {
        result |= key::Mods::ALT;
    }
    if modifiers.contains(Modifiers::SUPER) {
        result |= key::Mods::SUPER;
    }
    result
}

/// Builds a Ghostty key event from the wire-level Fleet event.
#[cfg(feature = "ghostty")]
pub fn ghostty_key_event(event: &KeyEvent) -> Result<key::Event<'static>, libghostty_vt::Error> {
    let mut result = key::Event::new()?;
    let text = event.text.clone().or_else(|| match event.key {
        Key::Char(character) => Some(character.to_string()),
        _ => None,
    });
    result
        .set_action(match event.action {
            KeyAction::Press => key::Action::Press,
            KeyAction::Repeat => key::Action::Repeat,
            KeyAction::Release => key::Action::Release,
        })
        .set_key(ghostty_key(event.key))
        .set_mods(ghostty_modifiers(event.mods))
        .set_utf8(text);
    if let Key::Char(character) = event.key {
        result.set_unshifted_codepoint(character.to_ascii_lowercase());
    }
    Ok(result)
}

/// Builds a Ghostty mouse event using one-pixel cells as the geometry unit.
#[cfg(feature = "ghostty")]
pub fn ghostty_mouse_event(
    event: &MouseEvent,
) -> Result<mouse::Event<'static>, libghostty_vt::Error> {
    let mut result = mouse::Event::new()?;
    result
        .set_action(match event.kind {
            MouseEventKind::Press => mouse::Action::Press,
            MouseEventKind::Release => mouse::Action::Release,
            MouseEventKind::Move | MouseEventKind::Drag => mouse::Action::Motion,
        })
        .set_button(match event.kind {
            MouseEventKind::Move => None,
            MouseEventKind::Press | MouseEventKind::Release | MouseEventKind::Drag => {
                Some(ghostty_mouse_button(event.button))
            }
        })
        .set_mods(ghostty_modifiers(event.mods))
        .set_position(mouse::Position {
            x: f32::from(event.col) + 0.5,
            y: f32::from(event.row) + 0.5,
        });
    Ok(result)
}

#[cfg(feature = "ghostty")]
fn ghostty_key(key: Key) -> key::Key {
    match key {
        Key::Enter => key::Key::Enter,
        Key::Escape => key::Key::Escape,
        Key::Backspace => key::Key::Backspace,
        Key::Tab => key::Key::Tab,
        Key::Up => key::Key::ArrowUp,
        Key::Down => key::Key::ArrowDown,
        Key::Left => key::Key::ArrowLeft,
        Key::Right => key::Key::ArrowRight,
        Key::Home => key::Key::Home,
        Key::End => key::Key::End,
        Key::PageUp => key::Key::PageUp,
        Key::PageDown => key::Key::PageDown,
        Key::Insert => key::Key::Insert,
        Key::Delete => key::Key::Delete,
        Key::F1 => key::Key::F1,
        Key::F2 => key::Key::F2,
        Key::F3 => key::Key::F3,
        Key::F4 => key::Key::F4,
        Key::F5 => key::Key::F5,
        Key::F6 => key::Key::F6,
        Key::F7 => key::Key::F7,
        Key::F8 => key::Key::F8,
        Key::F9 => key::Key::F9,
        Key::F10 => key::Key::F10,
        Key::F11 => key::Key::F11,
        Key::F12 => key::Key::F12,
        Key::Char(character) => ascii_key(character),
    }
}

#[cfg(feature = "ghostty")]
fn ascii_key(character: char) -> key::Key {
    match character.to_ascii_lowercase() {
        'a' => key::Key::A,
        'b' => key::Key::B,
        'c' => key::Key::C,
        'd' => key::Key::D,
        'e' => key::Key::E,
        'f' => key::Key::F,
        'g' => key::Key::G,
        'h' => key::Key::H,
        'i' => key::Key::I,
        'j' => key::Key::J,
        'k' => key::Key::K,
        'l' => key::Key::L,
        'm' => key::Key::M,
        'n' => key::Key::N,
        'o' => key::Key::O,
        'p' => key::Key::P,
        'q' => key::Key::Q,
        'r' => key::Key::R,
        's' => key::Key::S,
        't' => key::Key::T,
        'u' => key::Key::U,
        'v' => key::Key::V,
        'w' => key::Key::W,
        'x' => key::Key::X,
        'y' => key::Key::Y,
        'z' => key::Key::Z,
        '0' => key::Key::Digit0,
        '1' => key::Key::Digit1,
        '2' => key::Key::Digit2,
        '3' => key::Key::Digit3,
        '4' => key::Key::Digit4,
        '5' => key::Key::Digit5,
        '6' => key::Key::Digit6,
        '7' => key::Key::Digit7,
        '8' => key::Key::Digit8,
        '9' => key::Key::Digit9,
        ' ' => key::Key::Space,
        '-' => key::Key::Minus,
        '=' => key::Key::Equal,
        '[' => key::Key::BracketLeft,
        ']' => key::Key::BracketRight,
        '\\' => key::Key::Backslash,
        ';' => key::Key::Semicolon,
        '\'' => key::Key::Quote,
        ',' => key::Key::Comma,
        '.' => key::Key::Period,
        '/' => key::Key::Slash,
        '`' => key::Key::Backquote,
        _ => key::Key::Unidentified,
    }
}

#[cfg(feature = "ghostty")]
fn ghostty_mouse_button(button: MouseButton) -> mouse::Button {
    match button {
        MouseButton::Left => mouse::Button::Left,
        MouseButton::Middle => mouse::Button::Middle,
        MouseButton::Right => mouse::Button::Right,
        MouseButton::WheelUp => mouse::Button::Four,
        MouseButton::WheelDown => mouse::Button::Five,
        MouseButton::Other(4) => mouse::Button::Four,
        MouseButton::Other(5) => mouse::Button::Five,
        MouseButton::Other(6) => mouse::Button::Six,
        MouseButton::Other(7) => mouse::Button::Seven,
        MouseButton::Other(8) => mouse::Button::Eight,
        MouseButton::Other(9) => mouse::Button::Nine,
        MouseButton::Other(10) => mouse::Button::Ten,
        MouseButton::Other(11) => mouse::Button::Eleven,
        MouseButton::Other(_) => mouse::Button::Unknown,
    }
}
