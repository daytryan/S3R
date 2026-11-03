//! Not retail (user 10-25): the online chat filter. Slurs never show, whoever sends them and whatever the settings:
//! online_chat masks them before a line is sent and again as one arrives. Settings > Audio > Text Chat "Filtered" (the
//! default) masks swearing too; "Unfiltered" leaves it. A list word is found after folding case, accents, leetspeak and
//! look-alike letters, with any letter allowed to repeat, and across spaced-out letters or words; a match must cover
//! whole words, so "this hit" and "Scunthorpe" stay as written. A hit masks its whole word ("****").
use std::sync::OnceLock;

// ROT13, so the source doesn't spell them out. *_ANYWHERE: inside a word (plurals, compounds) or over whole words in a
// row; *_WORD: the whole word, with an "s" or "z" after.
const SLURS_ANYWHERE: &[&str] = &["avttre", "avttn", "snttbg", "genaal", "genaavr", "furznyr", "jrgonpx", "wvtnobb", "cbepuzbaxrl", "enturnq", "gbjryurnq", "mvccreurnq", "tbyyvjbt"];
const SLURS_WORD: &[&str] = &["snt", "pbba", "fcvp", "fcvpx", "tbbx", "xvxr", "cnxv", "qlxr", "puvax", "fnzob", "qnexvr", "qnexl", "ornare", "ergneq", "ergneqrq", "gneq"];
const SWEARS_ANYWHERE: &[&str] = &["shpx", "fuvg", "ovgpu", "onfgneq", "juber", "nffubyr", "jnax", "cvff", "fyhg", "obyybpxf", "qvpxurnq"];
const SWEARS_WORD: &[&str] = &["phag", "qvpx", "pbpx", "chffl", "gjng", "cevpx", "nff", "nefr", "gvg"];
/// Real words an ANYWHERE entry sits inside (folded, plain): never masked for it.
const KEEP: &[&str] = &["snigger", "sniggers", "sniggered", "sniggering", "niggard", "niggards", "niggardly", "shiitake"];

struct List {
    anywhere: Vec<Vec<char>>,
    word: Vec<Vec<char>>,
}

fn rot13(s: &str) -> Vec<char> {
    s.chars().map(|c| match c {
        'a'..='m' => (c as u8 + 13) as char,
        'n'..='z' => (c as u8 - 13) as char,
        _ => c,
    }).collect()
}

fn list(anywhere: &[&str], word: &[&str]) -> List {
    List { anywhere: anywhere.iter().map(|w| rot13(w)).collect(), word: word.iter().map(|w| rot13(w)).collect() }
}

fn slurs() -> &'static List {
    static LIST: OnceLock<List> = OnceLock::new();
    LIST.get_or_init(|| list(SLURS_ANYWHERE, SLURS_WORD))
}

fn swears() -> &'static List {
    static LIST: OnceLock<List> = OnceLock::new();
    LIST.get_or_init(|| list(SWEARS_ANYWHERE, SWEARS_WORD))
}

/// A word's letters as the lists spell them: lower case, accents and look-alikes (Cyrillic, Greek) folded, leetspeak
/// read as letters, anything else dropped. Edge punctuation is trimmed first, so a closing "!" isn't an "i".
fn fold(word: &str) -> Vec<char> {
    let word = word.trim_matches(|c: char| c.is_ascii_punctuation() && !"$@+|".contains(c));
    word.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| {
            Some(match c {
                'a'..='z' => c,
                '4' | '@' | 'à'..='å' | 'а' | 'α' => 'a',
                '8' => 'b',
                'ç' | 'с' => 'c',
                '3' | '€' | 'è'..='ë' | 'е' => 'e',
                '9' => 'g',
                '1' | '!' | '|' | 'ì'..='ï' | 'і' | 'ι' => 'i',
                'ñ' => 'n',
                '0' | 'ò'..='ö' | 'ø' | 'о' | 'ο' => 'o',
                'р' => 'p',
                '5' | '$' => 's',
                '7' | '+' => 't',
                'ù'..='ü' => 'u',
                'х' => 'x',
                'ý' | 'ÿ' | 'у' => 'y',
                _ => return None,
            })
        })
        .collect()
}

/// Where `entry` ends matched at `text[at..]`: each run of one letter in it met by at least as many, the whole run taken.
fn matches(text: &[char], at: usize, entry: &[char]) -> Option<usize> {
    let (mut t, mut e) = (at, 0);
    while e < entry.len() {
        let c = entry[e];
        let need = entry[e..].iter().take_while(|&&x| x == c).count();
        let have = text[t..].iter().take_while(|&&x| x == c).count();
        if have < need {
            return None;
        }
        (t, e) = (t + have, e + need);
    }
    Some(t)
}

/// `entry` as the whole word, with an "s" or "z" after.
fn whole(word: &[char], entry: &[char]) -> bool {
    matches(word, 0, entry).is_some_and(|end| matches!(word[end..], [] | ['s'] | ['z']))
}

fn mark(words: &[Vec<char>], list: &List, hide: &mut [bool]) {
    // Every word's letters end to end, `starts[i]` where word i begins.
    let all: Vec<char> = words.concat();
    let starts: Vec<usize> = words.iter().scan(0, |at, w| { let start = *at; *at += w.len(); Some(start) }).chain([all.len()]).collect();
    let owner = |p: usize| starts.partition_point(|&s| s <= p) - 1;
    for entry in &list.anywhere {
        for at in 0..all.len() {
            let Some(end) = matches(&all, at, entry) else { continue };
            let (first, last) = (owner(at), owner(end - 1));
            let kept = first == last && KEEP.iter().any(|k| words[first].iter().copied().eq(k.chars()));
            // Inside one word, or over whole words only ("this hit" is no match).
            if !kept && (first == last || starts[first] == at && starts[last + 1] == end) {
                hide[first..=last].fill(true);
            }
        }
    }
    for entry in &list.word {
        for (i, word) in words.iter().enumerate() {
            hide[i] |= whole(word, entry);
        }
        // Spaced-out letters ("a b c"): any run of one-letter words read as one.
        for first in 0..words.len() {
            for last in first + 1..words.len() {
                if words[last].len() != 1 || words[first].len() != 1 {
                    break;
                }
                if whole(&words[first..=last].concat(), entry) {
                    hide[first..=last].fill(true);
                }
            }
        }
    }
}

/// `text` with every word a list covers masked: the slurs always, the swears too when `swears`.
pub(crate) fn filter(text: &str, swears_too: bool) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let folded: Vec<Vec<char>> = words.iter().map(|w| fold(w)).collect();
    let mut hide = vec![false; words.len()];
    mark(&folded, slurs(), &mut hide);
    if swears_too {
        mark(&folded, swears(), &mut hide);
    }
    if !hide.contains(&true) {
        return text.to_owned();
    }
    let (mut out, mut last) = (String::with_capacity(text.len()), 0);
    for (word, hidden) in words.iter().zip(hide) {
        let at = word.as_ptr() as usize - text.as_ptr() as usize;
        out.push_str(&text[last..at]);
        if hidden {
            out.extend(std::iter::repeat_n('*', word.chars().count()));
        } else {
            out.push_str(word);
        }
        last = at + word.len();
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inputs are ROT13 too; digits and symbols pass through.
    fn rot(s: &str) -> String {
        s.chars().map(|c| match c {
            'a'..='m' | 'A'..='M' => (c as u8 + 13) as char,
            'n'..='z' | 'N'..='Z' => (c as u8 - 13) as char,
            _ => c,
        }).collect()
    }

    #[test]
    fn slurs_never_show_filtered_or_not() {
        for swears in [true, false] {
            assert_eq!(filter(&rot("lbh ner n avttre"), swears), "you are a ******");
            assert_eq!(filter(&rot("avttnf"), swears), "******", "plurals");
            assert_eq!(filter(&rot("snt!"), swears), "****", "a closing ! is no letter");
            assert_eq!(filter(&rot("SNTTT"), swears), "*****", "a whole-word entry, repeated letters");
        }
    }

    #[test]
    fn disguises_are_seen_through() {
        assert_eq!(filter(&rot("A1TT3E"), false), "******", "case and leetspeak");
        assert_eq!(filter(&rot("avvvvttttre"), false), "***********", "repeats");
        assert_eq!(filter(&rot("a.v.t.t.r.e"), false), "***********", "dots");
        assert_eq!(filter(&rot("a v t t r e"), false), "* * * * * *", "spaced out");
        assert_eq!(filter(&rot("s n t"), false), "* * *", "a spaced-out whole word");
        assert_eq!(filter(&rot("cbepu zbaxrl"), false), "***** ******", "over whole words");
        assert_eq!(filter(&format!("{}\u{435}{}", rot("avtt"), rot("e")), false), "******", "a Cyrillic e");
    }

    #[test]
    fn swears_are_masked_only_when_filtered() {
        assert_eq!(filter(&rot("jung gur shpx"), true), "what the ****");
        assert_eq!(filter(&rot("jung gur shpx"), false), rot("jung gur shpx"));
        assert_eq!(filter(&rot("zbgureshpxre"), true), "************", "inside a word");
        assert_eq!(filter(&rot("ng nff"), true), "at ***");
    }

    /// The Scunthorpe problem: words that merely contain a list entry, or meet one across a space, stay.
    #[test]
    fn innocent_words_stay() {
        for text in ["Scunthorpe", "this hit hard", "spicy line", "spiced", "cocktail", "assess the classic pass", "Niger and Nigeria",
            "he sniggered", "shiitake", "raccoon", "Dickens", "titanic"] {
            assert_eq!(filter(text, true), text);
        }
    }

    #[test]
    fn masking_keeps_the_spacing_and_settles() {
        let once = filter(&rot("  bx   avttre  tt "), true);
        assert_eq!(once, "  ok   ******  gg ");
        assert_eq!(filter(&once, true), once);
    }
}
