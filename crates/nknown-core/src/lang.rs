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

    /// Langue d'une étiquette BCP 47 / ISO 639-1 (`fr`, `fr-FR`, `en_US`) ou ISO 639-3 (`fra`).
    pub fn from_tag(tag: &str) -> Option<Self> {
        let primary = tag.split(['-', '_']).next()?.to_ascii_lowercase();
        if primary.len() == 3 {
            return Self::from_code(&primary);
        }
        let code = match primary.as_str() {
            "fr" => "fra",
            "en" => "eng",
            "de" => "deu",
            "es" => "spa",
            "it" => "ita",
            "pt" => "por",
            "nl" => "nld",
            "ca" => "cat",
            "br" => "bre",
            "eu" => "eus",
            "oc" => "oci",
            "pl" => "pol",
            "ru" => "rus",
            "uk" => "ukr",
            "cs" => "ces",
            "sv" => "swe",
            "da" => "dan",
            "no" | "nb" => "nob",
            "fi" => "fin",
            "el" => "ell",
            "tr" => "tur",
            "ar" => "ara",
            "he" => "heb",
            "fa" => "fas",
            "hi" => "hin",
            "zh" => "zho",
            "ja" => "jpn",
            "ko" => "kor",
            "vi" => "vie",
            "id" => "ind",
            "ro" => "ron",
            "hu" => "hun",
            _ => return None,
        };
        Self::from_code(code)
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
        assert_eq!(Lang::from_tag("fr-FR"), Some(Lang::FRA));
        assert_eq!(Lang::from_tag("EN_us"), Some(Lang::ENG));
        assert_eq!(Lang::from_tag("fra"), Some(Lang::FRA));
        assert_eq!(Lang::from_tag("xx"), None);
    }
}
