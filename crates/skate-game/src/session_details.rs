//! Not retail (user 10-25): what a host tells the server browser about its session (Host Actions): whether it is
//! listed there (skate_net::directory::Details, which multiplayer publishes to each lobby it owns), its name and its
//! description, and the on-screen keyboard (text_entry) they are typed on, drawn on the online layer in the spectate
//! legend's look. Names and descriptions are filtered where shown (server_browser). The same keyboard types Settings
//! search's words (user 10-26; core_menu opens their page).
use crate::{apt_scene::Draw, apt_text::Font, text_entry::TextEntry};
pub(crate) use skate_net::directory::Details;

/// The most characters a session's name and description hold (the relay bounds every lobby's the same way).
pub(crate) const NAME_MAX: usize = skate_net::directory::NAME.0;
pub(crate) const DESCRIPTION_MAX: usize = skate_net::directory::DESCRIPTION.0;
/// A description's line on the server browser's details screen.
pub(crate) const LINE: usize = 30;
/// The most characters a Settings search holds (the search page's tab name shows them).
pub(crate) const SEARCH_MAX: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    Name,
    Description,
    /// Settings search: Done hands its words to core_menu (online_menu's `searched`), not to the session.
    Search,
    /// The server browser's search (user 10-27): Done hands its words to server_browser::Browser.
    Servers,
}

/// The keyboard, open on one field. One Host Actions row edits both (the picker lists 8 at most): Done on the name goes
/// on to the description.
pub(crate) struct Keyboard {
    pub field: Field,
    pub entry: TextEntry,
}
impl Keyboard {
    pub fn open(field: Field, details: &Details) -> Self {
        let (text, max) = match field {
            Field::Name => (details.name.as_str(), NAME_MAX),
            Field::Description => (details.description.as_str(), DESCRIPTION_MAX),
            Field::Search => ("", SEARCH_MAX),
            Field::Servers => ("", NAME_MAX),
        };
        Self { field, entry: TextEntry::new(text, max) }
    }
    /// Settings search, starting from the page's `words`.
    pub fn search(words: &str) -> Self {
        Self { field: Field::Search, entry: TextEntry::new(words, SEARCH_MAX) }
    }
    /// The server browser's search, starting from its `words` (as long as a session's name can be).
    pub fn servers(words: &str) -> Self {
        Self { field: Field::Servers, entry: TextEntry::new(words, NAME_MAX) }
    }
    /// Done: the typed text, trimmed, into its field; after the name, the description's keyboard.
    pub fn apply(&self, details: &mut Details) -> Option<Keyboard> {
        let text = self.entry.text.trim().to_owned();
        match self.field {
            Field::Name => details.name = text,
            Field::Description => details.description = text,
            Field::Search | Field::Servers => {}
        }
        (self.field == Field::Name).then(|| Self::open(Field::Description, details))
    }
    pub fn title(&self) -> &'static str {
        match self.field {
            Field::Name => "Server Name",
            Field::Description => "Server Description",
            Field::Search => "Search Settings",
            Field::Servers => "Search Servers",
        }
    }
}

/// `text` in lines of at most `width` characters, broken between words (a longer word is cut).
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            if let Some(line) = lines.last_mut().filter(|l| !l.is_empty() && l.chars().count() + 1 + word.len() <= width) {
                line.push(' ');
                line.extend(word.iter());
                break;
            }
            let take = word.len().min(width);
            lines.push(word.drain(..take).collect());
            if word.is_empty() {
                break;
            }
        }
    }
    lines
}

/// The keyboard over the screen: its title, the text so far with its count, the keys (the highlighted one lit, as the
/// Graphic Creator's) and the pad's legend; the PC keyboard types, Enter is Done and Esc cancels.
pub(crate) fn draws(font: &Font, keyboard: &Keyboard) -> Vec<Draw> {
    use crate::graphic_board::{button, colour, label, quad};
    use crate::text_entry::{COMMANDS, Command2, GRID};
    const WHITE: &str = "runtime/white.rgba";
    const PANEL: [f32; 4] = [380., 160., 900., 548.];
    /// A key's pitch, and the gap between keys.
    const KEY: [f32; 2] = [48., 38.];
    const GAP: f32 = 4.;
    const INK: u32 = 0xEBF2F8;
    let entry = &keyboard.entry;
    let (x0, y0) = (PANEL[0] + 20., PANEL[1] + 16.);
    let mut out = vec![
        quad(WHITE, [1, 1], [0., 0., 1280., 720.], [0., 0., 0., 0.55]),
        quad(WHITE, [1, 1], PANEL, [0.024, 0.04, 0.055, 0.94]),
    ];
    out.extend(label(font, keyboard.title(), [x0, y0], 22., colour(0x6ED0FF, 1.)));
    let field = [x0, y0 + 36., PANEL[2] - 20., y0 + 70.];
    out.push(quad(WHITE, [1, 1], field, [1., 1., 1., 0.07]));
    out.extend(label(font, &format!("{}_", entry.text), [field[0] + 8., field[1] + 7.], 20., colour(INK, 1.)));
    let count = format!("{}/{}", entry.text.chars().count(), entry.max);
    out.extend(label(font, &count, [field[2] - 8. - font.width(&count, 16.), field[1] + 9.], 16., colour(0x96AFC3, 1.)));
    let top = field[3] + 14.;
    let key = |out: &mut Vec<Draw>, row: usize, column: usize, span: usize, name: &str| {
        let (x, y) = (x0 + column as f32 * KEY[0], top + row as f32 * KEY[1]);
        let rect = [x, y, x + span as f32 * KEY[0] - GAP, y + KEY[1] - GAP];
        let lit = entry.row == row && (column..column + span).contains(&entry.column);
        out.push(quad(WHITE, [1, 1], rect, if lit { colour(0x00CBFF, 0.9) } else { [1., 1., 1., 0.07] }));
        let x = (rect[0] + rect[2] - font.width(name, 18.)) / 2.;
        out.extend(label(font, name, [x, rect[1] + 8.], 18., colour(if lit { 0x02121E } else { INK }, 1.)));
    };
    for (row, keys) in GRID.iter().enumerate() {
        for (column, c) in keys.chars().enumerate() {
            key(&mut out, row, column, 1, &(if entry.shift { c.to_ascii_uppercase() } else { c }).to_string());
        }
    }
    let mut column = 0;
    for (command, span) in COMMANDS {
        let name = match command {
            Command2::Char(c) => c.to_string(),
            Command2::Shift => "Shift".into(),
            Command2::Space => "Space".into(),
            Command2::Backspace => "Delete".into(),
            Command2::Done => "Done".into(),
        };
        key(&mut out, GRID.len(), column, span, &name);
        column += span;
    }
    let mut x = x0;
    let y = top + (GRID.len() + 1) as f32 * KEY[1] + 8.;
    let done = if keyboard.field == Field::Name { "Next" } else { "Done" };
    for (glyph, words) in [("Start", done), ("X", "Delete"), ("Y", "Space"), ("B", "Cancel")] {
        out.push(button(glyph, [x, y - 1.], 17.));
        x += 17. * 4. / 3. + 6.;
        out.extend(label(font, words, [x, y], 16., colour(INK, 1.)));
        x += font.width(words, 16.) + 22.;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptions_wrap_between_words() {
        assert_eq!(wrap("Chill freeskate at the park, all welcome", LINE), ["Chill freeskate at the park,", "all welcome"]);
        assert_eq!(wrap("  ", LINE), Vec::<String>::new());
        assert_eq!(wrap("abcdefgh ij", 4), ["abcd", "efgh", "ij"], "a word longer than a line is cut");
    }
    /// User 10-26: Settings search types on this keyboard, with its own title and length; Done changes no session detail.
    #[test]
    fn settings_search_types_on_the_same_keyboard() {
        let keyboard = Keyboard::search("fps");
        assert_eq!(
            (keyboard.field, keyboard.title(), keyboard.entry.text.as_str(), keyboard.entry.max),
            (Field::Search, "Search Settings", "fps", SEARCH_MAX)
        );
        let mut details = Details { name: "Park".into(), ..Details::default() };
        assert!(keyboard.apply(&mut details).is_none());
        assert_eq!(details.name, "Park");
    }
    #[test]
    fn done_trims_into_its_field_then_goes_on_to_the_description() {
        let mut details = Details { description: "old".into(), ..Details::default() };
        let mut keyboard = Keyboard::open(Field::Name, &details);
        keyboard.entry.type_text("  Ryan's Park  ");
        let mut next = keyboard.apply(&mut details).expect("the description's keyboard");
        assert_eq!((details.name.as_str(), details.listed), ("Ryan's Park", true));
        assert_eq!((next.field, next.entry.text.as_str(), next.entry.max), (Field::Description, "old", DESCRIPTION_MAX));
        next.entry.backspace();
        assert!(next.apply(&mut details).is_none(), "the last field");
        assert_eq!(details.description, "ol");
    }
}
