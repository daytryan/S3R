//! Not retail (user 10-11): Skate 3 had no text chat. CHAT (kind 16) carries one typed line from its sender to every
//! member, peer to peer like movement, so the sender is the link it came from (`Session::receive`), never a name in the
//! text. Reliable and in order: `[base u32][UTF-8 text]` under the header's sequence, resent until CHAT_ACK (kind 17)
//! acknowledges it; `base` is the oldest line the sender still holds for that member, so a member whose link is new
//! skips the lines the old link already had.
use std::collections::VecDeque;

pub const CHAT: u8 = 16;
pub const CHAT_ACK: u8 = 17;
/// Characters a line holds at most, after trimming (user 10-29: 120 -> 150; at 4 bytes a character a line still fits
/// one datagram).
pub const MAX_CHARS: usize = 150;
/// Lines a sender may send per [`WINDOW_MS`]; more are dropped, on send and on receive.
pub const RATE: usize = 5;
pub const WINDOW_MS: u64 = 5000;

/// Combining marks (and emoji joiners / variation selectors) one character may carry; [`scrub`] drops the rest
/// ("zalgo").
pub const MAX_MARKS: usize = 3;

/// Characters that never draw as themselves: control and format characters (bidi overrides, zero-width spaces,
/// BOM, tags), fillers that look blank, private use (apt_scene draws U+E000.. as inline button glyphs),
/// noncharacters and the planes nothing is assigned in. The emoji joiner and variation selectors stay ([`mark`]s).
// ponytail: ranges, not the Unicode tables (no general-category crate in Cargo.lock); unassigned code points inside
// the assigned planes still pass, and draw as nothing (no font has them).
fn hidden(c: char) -> bool {
    let n = c as u32;
    c.is_control()
        || n & 0xFFFE == 0xFFFE
        || matches!(n,
            0xAD | 0x34F | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x115F..=0x1160
            | 0x17B4..=0x17B5 | 0x180B..=0x180F | 0x200B..=0x200C | 0x200E..=0x200F | 0x202A..=0x202E
            | 0x2060..=0x206F | 0x3164 | 0xE000..=0xF8FF | 0xFDD0..=0xFDEF | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFFC
            | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0x32400..)
}

/// A character drawn onto the one before it: combining marks, the emoji joiner, variation selectors.
// ponytail: the zalgo blocks plus Hebrew, Arabic, Devanagari, Thai and kana marks; other scripts' are uncapped.
fn mark(c: char) -> bool {
    matches!(c as u32,
        0x300..=0x36F | 0x483..=0x489 | 0x591..=0x5C7 | 0x610..=0x61A | 0x64B..=0x65F | 0x670 | 0x6D6..=0x6ED
        | 0x900..=0x903 | 0x93A..=0x94F | 0x951..=0x957 | 0x962..=0x963 | 0xE31 | 0xE34..=0xE3A | 0xE47..=0xE4E
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200D | 0x20D0..=0x20FF | 0x3099..=0x309A | 0xFE00..=0xFE0F
        | 0xFE20..=0xFE2F)
}

/// `text` without what [`hidden`] names and with at most [`MAX_MARKS`] marks per character: the allow-list chat and
/// names share (letters, numbers, punctuation, symbols, emoji, spaces).
pub fn scrub(text: &str) -> String {
    let mut marks = 0;
    text.chars()
        .filter(|&c| !hidden(c))
        .filter(|&c| {
            marks = if mark(c) { marks + 1 } else { 0 };
            marks <= MAX_MARKS
        })
        .collect()
}

/// Whether `text` shows anything: not only spaces, marks, joiners or blank braille cells.
pub fn visible(text: &str) -> bool {
    text.chars().any(|c| !c.is_whitespace() && !mark(c) && !hidden(c) && c != '\u{2800}')
}

/// The line as received: trimmed, 1..=[`MAX_CHARS`] characters, nothing [`scrub`] drops, something [`visible`];
/// None otherwise (an older sender's line holding such characters is not shown).
pub fn valid(text: &str) -> Option<&str> {
    let text = text.trim();
    (visible(text) && text.chars().count() <= MAX_CHARS && scrub(text) == text).then_some(text)
}

/// What the typing bar sends: [`scrub`]bed, then [`valid`].
pub fn clean(text: &str) -> Option<String> {
    valid(&scrub(text)).map(str::to_owned)
}

/// At most [`RATE`] lines per [`WINDOW_MS`].
#[derive(Default)]
pub struct Limiter(VecDeque<u64>);
impl Limiter {
    /// Counts the line at `now` (ms) if it is within the rate.
    pub fn allow(&mut self, now: u64) -> bool {
        self.allow_up_to(RATE, now)
    }
    /// Audit 10-24: a receiver allows twice the rate: lines said within it arrive bunched up (jitter, a resend), and one
    /// dropped there is lost for good (it was acknowledged).
    pub fn allow_up_to(&mut self, rate: usize, now: u64) -> bool {
        while self.0.front().is_some_and(|&t| now.saturating_sub(t) >= WINDOW_MS) {
            self.0.pop_front();
        }
        if self.0.len() >= rate {
            return false;
        }
        self.0.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_trimmed_printable_and_at_most_150_chars() {
        assert_eq!(valid("  hi there "), Some("hi there"));
        assert_eq!(valid("   "), None, "empty after trimming");
        assert_eq!(valid("a\u{7}b"), None, "a control character");
        assert_eq!(valid("a\nb"), None, "a line break");
        let wide = "é".repeat(MAX_CHARS);
        assert_eq!(valid(&wide), Some(wide.as_str()), "150 characters, 300 bytes");
        assert_eq!(valid(&format!("{wide}x")), None, "151 characters");
        assert_eq!(clean(" a\u{1b}[31mb\r\n"), Some("a[31mb".into()), "the bar strips control characters");
        assert_eq!(clean("\r\n\t"), None);
        assert!(crate::packed::HEADER + 4 + 4 * MAX_CHARS <= crate::packed::MTU, "the longest line fits a datagram");
    }

    /// C5: an allow-list, on send and on receive. Bidi overrides, zero-width and other format characters, private
    /// use (the HUD's inline button glyphs), tags and noncharacters are dropped by the bar and refused on arrival;
    /// marks are capped per character; emoji keep their joiner, variation selector and skin tone.
    #[test]
    fn lines_hold_only_characters_that_draw_as_themselves() {
        for bad in ["a\u{202E}b", "a\u{200B}b", "a\u{2066}b", "a\u{AD}b", "a\u{FEFF}", "a\u{E000}", "a\u{F8FF}", "a\u{E0041}", "a\u{FFFF}", "a\u{10FFFD}"] {
            assert_eq!(valid(bad), None, "{bad:?} refused on arrival");
            let kept = if bad.ends_with('b') { "ab" } else { "a" };
            assert_eq!(clean(bad).as_deref(), Some(kept), "{bad:?} scrubbed by the bar");
        }
        let zalgo = format!("a{}b", "\u{301}".repeat(50));
        assert_eq!(valid(&zalgo), None);
        assert_eq!(clean(&zalgo), Some(format!("a{}b", "\u{301}".repeat(MAX_MARKS))));
        for blank in ["\u{200B}\u{200B}", "\u{3164}", "\u{2800}\u{2800}", "\u{301}\u{301}", "\u{115F}", "\u{202E} "] {
            assert_eq!(clean(blank), None, "{blank:?} shows nothing");
        }
        for ok in ["ti\u{1EBF}ng Vi\u{1EC7}t", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", "\u{2764}\u{FE0F}", "\u{1F44D}\u{1F3FD}", "\u{1F1EF}\u{1F1F5}", "\u{5E9}\u{5C1}\u{5BC}", "e\u{301}\u{302}", "\u{2800}\u{2801}", "\u{D55C}\u{AD6D}\u{C5B4} \u{4E2D}\u{6587}", "<3 :) $5"] {
            assert_eq!(valid(ok), Some(ok), "{ok:?} kept");
        }
    }

    #[test]
    fn five_lines_per_five_seconds() {
        let mut limit = Limiter::default();
        assert!((0..5).all(|n| limit.allow(n * 100)));
        assert!(!limit.allow(4_999), "a sixth inside the window");
        assert!(limit.allow(5_000), "the first left the window");
        assert!(!limit.allow(5_001));
    }
}
