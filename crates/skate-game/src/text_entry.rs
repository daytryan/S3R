//! The on-screen keyboard's model, shared by the boot team-name entry, Team Select and the Graphic Creator's text
//! layers (decision 9): the PC keyboard types, a controller uses the grid. Each user passes its own length cap.

use crate::boot_flow::Key;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextEntry {
    pub text: String,
    pub row: usize,
    pub column: usize,
    pub shift: bool,
    /// The most characters `text` holds.
    pub max: usize,
}
/// The on-screen keys: every character of ALL_VALID_ALPHANUMERIC_CHARACTERS, then the command row.
/// Not retail (user 10-14): QWERTY order, not the Guide keyboard's A-Z; the same 50 keys.
pub(crate) const GRID: [&str; 5] = ["1234567890", "qwertyuiop", "asdfghjkl'", "zxcvbnm,._", "-\"?!@#$*()"];
/// The last row: (key, columns it spans).
pub(crate) const COMMANDS: [(Command2, usize); 5] =
    [(Command2::Char('/'), 1), (Command2::Shift, 2), (Command2::Space, 3), (Command2::Backspace, 2), (Command2::Done, 2)];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command2 {
    Char(char),
    Shift,
    Space,
    Backspace,
    Done,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Typed {
    Done,
    Cancel,
}
impl TextEntry {
    /// `default` is shown whole, as the boot keyboard always has; typing past `max` is what stops.
    pub(crate) fn new(default: &str, max: usize) -> Self {
        Self { text: default.to_owned(), row: 0, column: 0, shift: false, max }
    }
    pub(crate) fn rows() -> usize {
        GRID.len() + 1
    }
    /// The key under the highlight.
    pub(crate) fn key(&self) -> Command2 {
        if let Some(row) = GRID.get(self.row) {
            let c = row.chars().nth(self.column).unwrap_or(' ');
            return Command2::Char(if self.shift { c.to_ascii_uppercase() } else { c });
        }
        let mut start = 0;
        for (key, span) in COMMANDS {
            if self.column < start + span {
                return key;
            }
            start += span;
        }
        Command2::Done
    }
    fn push(&mut self, c: char) {
        if self.text.chars().count() < self.max {
            self.text.push(c);
        }
    }
    pub(crate) fn type_text(&mut self, text: &str) {
        for c in text.chars().filter(|c| !c.is_control()) {
            self.push(c);
        }
    }
    pub(crate) fn backspace(&mut self) {
        self.text.pop();
    }
    pub(crate) fn input(&mut self, key: Key) -> Option<Typed> {
        let width = 10;
        match key {
            Key::Up => self.row = (self.row + Self::rows() - 1) % Self::rows(),
            Key::Down => self.row = (self.row + 1) % Self::rows(),
            Key::Left => self.column = (self.column + width - 1) % width,
            Key::Right => self.column = (self.column + 1) % width,
            Key::X => self.backspace(),
            Key::Y => self.push(' '),
            Key::Start => return Some(Typed::Done),
            Key::B => return Some(Typed::Cancel),
            Key::A => match self.key() {
                Command2::Char(c) => self.push(c),
                Command2::Shift => self.shift = !self.shift,
                Command2::Space => self.push(' '),
                Command2::Backspace => self.backspace(),
                Command2::Done => return Some(Typed::Done),
            },
            Key::StartReleased => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_is_the_callers() {
        let mut long = TextEntry::new("abcdefghij", 4);
        long.type_text("k");
        assert_eq!(long.text, "abcdefghij", "the default is shown whole; nothing more goes on");
        let mut k = TextEntry::new("", 2);
        k.type_text("xyz\u{7}");
        assert_eq!(k.text, "xy");
        k.input(Key::Y);
        assert_eq!(k.text, "xy");
    }
}
