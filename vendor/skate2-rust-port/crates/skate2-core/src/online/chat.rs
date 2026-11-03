//! Lobby chat (PC port addition): T opens a prompt, Enter sends its line to the server, which
//! relays it to the game's members, and every member's event log shows it as
//! `[name]: line`, wrapped into lines of at most 50 characters.

use crate::ui::extras::TextInput;
use skate2_net::protocol::{CHAT_CHARACTERS, valid_chat};

/// The widest line the event log shows a chat line in.
pub const LINE_CHARACTERS: usize = 50;

/// The open chat prompt: the line typed so far and the cursor's place in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatPrompt {
    pub text: String,
    pub cursor: usize,
}

/// What a key did to the prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatOutcome {
    Open,
    Closed,
    /// Enter on a line worth sending: the line, trimmed.
    Send(String),
}

impl ChatPrompt {
    /// A key: printable ASCII and spaces up to the line's limit, editing, Enter and Esc.
    pub fn input(&mut self, input: TextInput) -> ChatOutcome {
        match input {
            TextInput::Insert(typed) => {
                for character in typed.chars().filter(|character| character.is_ascii_graphic() || *character == ' ') {
                    if self.text.len() == CHAT_CHARACTERS {
                        break;
                    }
                    self.text.insert(self.cursor, character);
                    self.cursor += 1;
                }
            }
            TextInput::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.cursor);
            }
            TextInput::Delete if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            TextInput::Left => self.cursor = self.cursor.saturating_sub(1),
            TextInput::Right => self.cursor = (self.cursor + 1).min(self.text.len()),
            TextInput::Home => self.cursor = 0,
            TextInput::End => self.cursor = self.text.len(),
            TextInput::Accept => {
                let line = self.text.trim();
                return if valid_chat(line) { ChatOutcome::Send(line.to_owned()) } else { ChatOutcome::Closed };
            }
            TextInput::Cancel => return ChatOutcome::Closed,
            TextInput::Backspace | TextInput::Delete => {}
        }
        ChatOutcome::Open
    }
}

/// The event log's lines for a member's chat line: `[name]: line` (both ASCII, as the
/// nickname and chat checks make them), broken at spaces into lines of at most
/// [`LINE_CHARACTERS`] characters; a word wider than a line fills the line it starts on and
/// goes on in the next.
#[must_use]
pub fn lines(name: &str, text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for mut word in format!("[{name}]: {text}").split(' ').filter(|word| !word.is_empty()) {
        let gap = usize::from(!current.is_empty());
        let append = |current: &mut String, part: &str| {
            if gap == 1 {
                current.push(' ');
            }
            current.push_str(part);
        };
        if current.len() + gap + word.len() <= LINE_CHARACTERS {
            append(&mut current, word);
            continue;
        }
        if word.len() > LINE_CHARACTERS {
            let room = LINE_CHARACTERS.saturating_sub(current.len() + gap);
            if room > 0 {
                append(&mut current, &word[..room]);
                word = &word[room..];
            }
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        while word.len() > LINE_CHARACTERS {
            let (head, rest) = word.split_at(LINE_CHARACTERS);
            lines.push(head.to_owned());
            word = rest;
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> ChatPrompt {
        let mut prompt = ChatPrompt::default();
        prompt.input(TextInput::Insert(text.to_owned()));
        prompt
    }

    #[test]
    fn the_prompt_takes_printable_ascii_up_to_the_limit_and_sends_it_trimmed() {
        let mut prompt = typed("  hey\tthere\u{e9}!  ");
        assert_eq!(prompt.text, "  heythere!  ");
        assert_eq!(prompt.input(TextInput::Accept), ChatOutcome::Send("heythere!".into()));
        let mut prompt = typed(&"a".repeat(CHAT_CHARACTERS + 20));
        assert_eq!(prompt.text.len(), CHAT_CHARACTERS);
        prompt.input(TextInput::Home);
        prompt.input(TextInput::Delete);
        prompt.input(TextInput::Insert("bc".into()));
        assert!(prompt.text.starts_with('b') && prompt.text.len() == CHAT_CHARACTERS);
        assert_eq!(typed("   ").input(TextInput::Accept), ChatOutcome::Closed);
        assert_eq!(typed("hi").input(TextInput::Cancel), ChatOutcome::Closed);
    }

    #[test]
    fn a_chat_line_wraps_at_fifty_characters_behind_its_name() {
        assert_eq!(lines("Rob", "hi all"), ["[Rob]: hi all"]);
        let wrapped = lines("Rob", &"word ".repeat(40).trim_end().to_owned());
        assert!(wrapped.iter().all(|line| line.len() <= LINE_CHARACTERS));
        assert!(wrapped[0].starts_with("[Rob]: word word"));
        assert_eq!(wrapped.join(" "), format!("[Rob]: {}", "word ".repeat(40).trim_end()));
        let long = lines("Rob", &"x".repeat(120));
        assert_eq!(long, [format!("[Rob]: {}", "x".repeat(43)), "x".repeat(50), "x".repeat(27)]);
    }
}
