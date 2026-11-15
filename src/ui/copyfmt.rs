//! Clipboard text of a table row.
//!
//! A row is a list of fields (heading, property name, value). A format decides how one
//! field becomes a line; the rows are joined with CRLF. Values that are missing or
//! unknown ("—" or empty) are written as "-".
//!
//! To add a format or a scope: add a variant, then fill in `ALL`, `key`, `label`, and
//! the match arm. The compiler reports every match that needs the new variant.

/// One property of a row.
#[derive(Clone, Copy)]
pub struct Field<'a> {
    /// The header text, e.g. "CPU".
    pub heading: &'a str,
    /// The technical name, `source.field`, e.g. "Proc.cpu".
    pub property: &'a str,
    pub value: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Format {
    /// `{heading}:{value}`
    #[default]
    Standard,
    /// `{heading}({property}):{value}`
    Detailed,
}

impl Format {
    pub const ALL: [Format; 2] = [Format::Standard, Format::Detailed];

    /// The name stored in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Format::Standard => "standard",
            Format::Detailed => "detailed",
        }
    }

    /// The name shown in the settings page.
    pub fn label(self) -> &'static str {
        match self {
            Format::Standard => crate::i18n::tr().settings.std_format,
            Format::Detailed => crate::i18n::tr().settings.detailed,
        }
    }

    pub fn from_key(key: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.key() == key)
    }

    /// One line of the copied text.
    fn line(self, f: &Field) -> String {
        let value = if f.value.is_empty() || f.value == "—" { "-" } else { f.value };
        match self {
            Format::Standard => format!("{}:{}", f.heading, value),
            Format::Detailed => format!("{}({}):{}", f.heading, f.property, value),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Scope {
    /// Only the properties of the columns that are shown.
    #[default]
    Visible,
    /// Every property of the row, shown or not.
    All,
}

impl Scope {
    pub const ALL: [Scope; 2] = [Scope::Visible, Scope::All];

    pub fn key(self) -> &'static str {
        match self {
            Scope::Visible => "visible",
            Scope::All => "all",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Visible => crate::i18n::tr().settings.visible_cols,
            Scope::All => crate::i18n::tr().settings.all,
        }
    }

    pub fn from_key(key: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// Formats the fields of one row. Lines are separated by CRLF, as Windows text tools expect.
pub fn format_row<'a>(format: Format, fields: impl IntoIterator<Item = Field<'a>>) -> String {
    fields.into_iter().map(|f| format.line(&f)).collect::<Vec<_>>().join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> [Field<'static>; 2] {
        [
            Field { heading: "CPU", property: "Proc.cpu", value: "12" },
            Field { heading: "PID", property: "Proc.pid", value: "" },
        ]
    }

    #[test]
    fn standard_and_detailed() {
        assert_eq!(format_row(Format::Standard, fields()), "CPU:12\r\nPID:-");
        assert_eq!(format_row(Format::Detailed, fields()), "CPU(Proc.cpu):12\r\nPID(Proc.pid):-");
    }

    #[test]
    fn unknown_dash_is_replaced() {
        let f = [Field { heading: "メモリ", property: "Proc.private_ws", value: "—" }];
        assert_eq!(format_row(Format::Standard, f), "メモリ:-");
    }

    #[test]
    fn keys_round_trip() {
        for f in Format::ALL {
            assert_eq!(Format::from_key(f.key()), Some(f));
        }
        for s in Scope::ALL {
            assert_eq!(Scope::from_key(s.key()), Some(s));
        }
        assert_eq!(Format::from_key("unknown"), None);
    }
}
