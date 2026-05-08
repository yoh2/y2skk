use bitflags::bitflags;

/// Platform-independent key identifier.
/// Maps X11 keysym / Qt::Key / GDK_KEY_* values to this enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Space,
    Return,
    BackSpace,
    Delete,
    Escape,
    Tab,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    F(u8),
    /// Unknown keysym (for passthrough)
    Other(u32),
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Modifiers: u8 {
        const SHIFT = 0b0001;
        const CTRL  = 0b0010;
        const ALT   = 0b0100;
        const META  = 0b1000;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
    pub is_press: bool,
}

impl KeyEvent {
    pub fn press(key: Key, modifiers: Modifiers) -> Self {
        Self {
            key,
            modifiers,
            is_press: true,
        }
    }

    pub fn release(key: Key, modifiers: Modifiers) -> Self {
        Self {
            key,
            modifiers,
            is_press: false,
        }
    }

    /// Returns the printable character for this event, if any (without Shift consideration).
    pub fn printable_char(&self) -> Option<char> {
        match &self.key {
            Key::Char(c) => Some(*c),
            Key::Space => Some(' '),
            _ => None,
        }
    }
}
