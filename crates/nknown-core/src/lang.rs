//! Identifiants de langue.

use std::fmt;

/// Code de langue ISO 639-3 (`fra`, `eng`, ...) : trois lettres ASCII minuscules.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lang([u8; 3]);

impl Lang {
    pub const FRA: Lang = Lang(*b"fra");
    pub const ENG: Lang = Lang(*b"eng");

    pub fn from_code(code: &str) -> Option<Self> {
        match code.as_bytes() {
            &[a, b, c] if [a, b, c].iter().all(u8::is_ascii_lowercase) => Some(Lang([a, b, c])),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("ASCII garanti par la construction")
    }
}

impl fmt::Debug for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Lang({})", self.as_str())
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_codes() {
        assert_eq!(Lang::from_code("fra"), Some(Lang::FRA));
        assert_eq!(Lang::from_code("FRA"), None);
        assert_eq!(Lang::from_code("fr"), None);
        assert_eq!(Lang::ENG.to_string(), "eng");
    }
}
