use std::sync::OnceLock;

use windows::Win32::Globalization::GetUserDefaultUILanguage;

const PRIMARY_LANGUAGE_MASK: u16 = 0x3ff;
const LANG_POLISH: u16 = 0x15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    Polish,
}

static LANGUAGE: OnceLock<Language> = OnceLock::new();

impl Language {
    pub fn parse(code: &str) -> Option<Self> {
        match code.trim().to_ascii_lowercase().split(['-', '_']).next()? {
            "en" => Some(Self::English),
            "pl" => Some(Self::Polish),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::English => "",
            Self::Polish => "pl",
        }
    }

    fn detect() -> Self {
        if let Some(language) = std::env::var("CHROMAFREE_LANGUAGE")
            .ok()
            .as_deref()
            .and_then(Self::parse)
        {
            return language;
        }
        match unsafe { GetUserDefaultUILanguage() } & PRIMARY_LANGUAGE_MASK {
            LANG_POLISH => Self::Polish,
            _ => Self::English,
        }
    }

    pub fn current() -> Self {
        *LANGUAGE.get_or_init(Self::detect)
    }

    pub fn pick<T>(self, english: T, polish: T) -> T {
        match self {
            Self::English => english,
            Self::Polish => polish,
        }
    }
}

pub fn tr(english: &'static str, polish: &'static str) -> &'static str {
    Language::current().pick(english, polish)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_language_codes() {
        assert_eq!(Language::parse("pl-PL"), Some(Language::Polish));
        assert_eq!(Language::parse("EN_us"), Some(Language::English));
        assert_eq!(Language::parse("de"), None);
    }
}
