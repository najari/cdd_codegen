//! Turning CDD qualifiers and labels into Rust identifiers.
//!
//! The rules, in order: letters with accents become their base letter, everything that is not an
//! ASCII letter or digit separates words, a word list that is empty or starts with a digit gets a
//! prefix, Rust keywords get a trailing underscore, and a name already taken in its scope gets
//! `_2`, `_3`. The same CDD always gives the same names: later entries take the suffix, so adding
//! a service does not rename the ones before it.

use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use std::collections::HashSet;

/// Latin letters with diacritics and a few ligatures, mapped to ASCII.
fn fold_char(c: char) -> Option<&'static str> {
    Some(match c {
        'ä' | 'à' | 'á' | 'â' | 'ã' | 'å' | 'ā' => "a",
        'Ä' | 'À' | 'Á' | 'Â' | 'Ã' | 'Å' | 'Ā' => "A",
        'ö' | 'ò' | 'ó' | 'ô' | 'õ' | 'ø' | 'ō' => "o",
        'Ö' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ø' | 'Ō' => "O",
        'ü' | 'ù' | 'ú' | 'û' | 'ū' => "u",
        'Ü' | 'Ù' | 'Ú' | 'Û' | 'Ū' => "U",
        'é' | 'è' | 'ê' | 'ë' | 'ē' => "e",
        'É' | 'È' | 'Ê' | 'Ë' | 'Ē' => "E",
        'í' | 'ì' | 'î' | 'ï' | 'ī' => "i",
        'Í' | 'Ì' | 'Î' | 'Ï' | 'Ī' => "I",
        'ç' | 'ć' | 'č' => "c",
        'Ç' | 'Ć' | 'Č' => "C",
        'ñ' | 'ń' => "n",
        'Ñ' | 'Ń' => "N",
        'š' | 'ś' => "s",
        'Š' | 'Ś' => "S",
        'ž' | 'ź' | 'ż' => "z",
        'Ž' | 'Ź' | 'Ż' => "Z",
        'ý' | 'ÿ' => "y",
        'ß' => "ss",
        'æ' => "ae",
        'Æ' => "AE",
        'œ' => "oe",
        'Œ' => "OE",
        _ => return None,
    })
}

/// ASCII letters and digits; every other character is a word break (a space).
pub fn ascii_words(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if let Some(f) = fold_char(c) {
            out.push_str(f);
        } else {
            out.push(' ');
        }
    }
    out
}

const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
];

fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

fn finish(mut s: String, prefix: &str) -> String {
    if s.is_empty() {
        s = prefix.to_string();
    } else if s.starts_with(|c: char| c.is_ascii_digit()) {
        s = format!("{prefix}{s}");
    }
    if is_keyword(&s) {
        s.push('_');
    }
    s
}

pub fn snake(s: &str) -> String {
    let words = ascii_words(s);
    finish(words.to_snake_case(), "x")
}

pub fn camel(s: &str) -> String {
    let words = ascii_words(s);
    finish(words.to_upper_camel_case(), "X")
}

pub fn shouty(s: &str) -> String {
    let words = ascii_words(s);
    finish(words.to_shouty_snake_case(), "X")
}

/// A label shortened to something that reads as an identifier.
pub fn camel_label(s: &str, max: usize) -> String {
    let mut c = camel(s);
    if c.len() > max {
        c.truncate(max);
        // Never cut a keyword escape in half.
        while !c.is_char_boundary(c.len()) {
            c.pop();
        }
    }
    c
}

/// Names already used in one scope.
#[derive(Debug)]
pub struct Scope {
    used: HashSet<String>,
    sep: &'static str,
}

impl Default for Scope {
    /// For snake_case and SHOUTY names: `name`, `name_2`.
    fn default() -> Self {
        Scope {
            used: HashSet::new(),
            sep: "_",
        }
    }
}

impl Scope {
    /// For UpperCamelCase names, where an underscore is not allowed: `Name`, `Name2`.
    pub fn camel() -> Scope {
        Scope {
            used: HashSet::new(),
            sep: "",
        }
    }

    /// `base` if it is free, else `base_2`, `base_3`, ...
    pub fn take(&mut self, base: String) -> String {
        if self.used.insert(base.clone()) {
            return base;
        }
        let mut n = 2;
        loop {
            let candidate = format!("{base}{}{n}", self.sep);
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }

    pub fn reserve(&mut self, name: &str) {
        self.used.insert(name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdd_qualifiers_become_idiomatic_names() {
        assert_eq!(snake("Variant_Coding_Read"), "variant_coding_read");
        assert_eq!(snake("SeedLevel_0x01_Request"), "seed_level_0x01_request");
        assert_eq!(snake("DEFAULT_SESSION_Start"), "default_session_start");
        // A digit starts a word that heck keeps in lower case.
        assert_eq!(camel("offOn_1Byte"), "OffOn1byte");
        assert_eq!(shouty("General_reject"), "GENERAL_REJECT");
    }

    #[test]
    fn keywords_digits_and_empty_names_are_made_legal() {
        assert_eq!(snake("type"), "type_");
        assert_eq!(snake("Match"), "match_");
        assert_eq!(snake("0x10"), "x0x10");
        assert_eq!(camel("1Byte"), "X1byte");
        assert_eq!(snake("***"), "x");
    }

    #[test]
    fn accents_fold_and_other_scripts_separate() {
        assert_eq!(snake("Prüfung_läuft"), "prufung_lauft");
        assert_eq!(camel("Straße"), "Strasse");
        assert_eq!(camel("テスト Door"), "Door");
    }

    #[test]
    fn a_taken_name_gets_a_suffix_and_the_first_keeps_its_name() {
        let mut s = Scope::default();
        assert_eq!(s.take("data".into()), "data");
        assert_eq!(s.take("data".into()), "data_2");
        assert_eq!(s.take("data".into()), "data_3");
        assert_eq!(s.take("data_2".into()), "data_2_2");
    }
}
