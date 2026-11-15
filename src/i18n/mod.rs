//! Languages of the user interface.
//!
//! Code reads its text through `tr()`, which returns the current language's `Strings`.
//! The language can change while the app runs, so text is looked up when it is drawn rather
//! than kept; a table that must keep a label keeps a `Text`, which looks it up each time.
//!
//! To add a language: copy `en.rs` to a new file and translate it, then add a variant to
//! `Lang` and fill in its arms below (key, name, strings, fonts and the system languages
//! that pick it).

mod en;
mod ja;
mod ko;
pub mod strings;
mod zh_hans;

pub use strings::Strings;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Lang {
    En,
    Ja,
    ZhHans,
    Ko,
}

/// A font face and its weight (FW_*).
pub type FontFace = (&'static str, i32);

const FW_NORMAL: i32 = 400;
const FW_BOLD: i32 = 700;

impl Lang {
    /// In the order of the language picker.
    pub const ALL: [Lang; 4] = [Lang::En, Lang::Ja, Lang::ZhHans, Lang::Ko];

    /// The key in the settings file, a BCP 47 tag.
    pub fn key(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ja => "ja",
            Lang::ZhHans => "zh-Hans",
            Lang::Ko => "ko",
        }
    }

    pub fn from_key(key: &str) -> Option<Lang> {
        Self::ALL.into_iter().find(|l| l.key().eq_ignore_ascii_case(key))
    }

    /// The language's name in itself, for the language picker.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ja => "日本語",
            Lang::ZhHans => "简体中文",
            Lang::Ko => "한국어",
        }
    }

    pub fn strings(self) -> &'static Strings {
        match self {
            Lang::En => &en::STRINGS,
            Lang::Ja => &ja::STRINGS,
            Lang::ZhHans => &zh_hans::STRINGS,
            Lang::Ko => &ko::STRINGS,
        }
    }

    /// The regular and semibold faces for text that is not plain ASCII. ASCII text uses
    /// Segoe UI in every language.
    pub fn fonts(self) -> [FontFace; 2] {
        match self {
            Lang::Ja => [("Yu Gothic UI", FW_NORMAL), ("Yu Gothic UI Semibold", FW_NORMAL)],
            Lang::ZhHans => [("Microsoft YaHei UI", FW_NORMAL), ("Microsoft YaHei UI", FW_BOLD)],
            Lang::Ko => [("Malgun Gothic", FW_NORMAL), ("Malgun Gothic", FW_BOLD)],
            // Process names and paths can be in the system's language, so follow its fonts.
            Lang::En => match system() {
                Some(l) if l != Lang::En => l.fonts(),
                _ => [("Segoe UI", FW_NORMAL), ("Segoe UI Semibold", FW_NORMAL)],
            },
        }
    }

    /// The supported language of a Windows LANGID, if any.
    fn from_langid(id: u16) -> Option<Lang> {
        let (primary, sub) = (id & 0x3FF, id >> 10);
        match primary {
            0x09 => Some(Lang::En),
            0x11 => Some(Lang::Ja),
            0x12 => Some(Lang::Ko),
            // Chinese (PRC) and Chinese (Singapore) write simplified characters.
            0x04 if sub == 0x02 || sub == 0x04 => Some(Lang::ZhHans),
            _ => None,
        }
    }
}

/// The Windows display language, when it is one of ours.
pub fn system() -> Option<Lang> {
    let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    Lang::from_langid(id)
}

/// The language a setting stands for: the chosen one, or else the system's, or else English.
pub fn resolve(setting: Option<Lang>) -> Lang {
    setting.or_else(system).unwrap_or(Lang::En)
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    let v = CURRENT.load(Ordering::Relaxed);
    Lang::ALL.into_iter().find(|l| *l as u8 == v).unwrap_or(Lang::En)
}

/// The current language's text.
pub fn tr() -> &'static Strings {
    current().strings()
}

/// A label that is looked up when it is shown, for tables built once and kept.
pub type Text = fn(&'static Strings) -> &'static str;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn langids() {
        assert_eq!(Lang::from_langid(0x0409), Some(Lang::En));
        assert_eq!(Lang::from_langid(0x0809), Some(Lang::En));
        assert_eq!(Lang::from_langid(0x0411), Some(Lang::Ja));
        assert_eq!(Lang::from_langid(0x0804), Some(Lang::ZhHans));
        assert_eq!(Lang::from_langid(0x1004), Some(Lang::ZhHans));
        assert_eq!(Lang::from_langid(0x0404), None);
        assert_eq!(Lang::from_langid(0x0412), Some(Lang::Ko));
        assert_eq!(Lang::from_langid(0x040C), None);
    }

    #[test]
    fn keys_round_trip() {
        for l in Lang::ALL {
            assert_eq!(Lang::from_key(l.key()), Some(l));
        }
    }

    #[test]
    fn formatted_strings() {
        assert_eq!((Lang::En.strings().details.n_processes)("Edge", 3), "Edge (3 processes)");
        assert_eq!((Lang::Ja.strings().details.n_processes)("Edge", 3), "Edge (3 個のプロセス)");
    }
}
