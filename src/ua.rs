/// User-Agent selection. One value is picked at process start and reused
/// for the lifetime of the process (no rotation, no pool).
const HONEST: &str = "reddit-rss-mcp/0.1 (+https://github.com/AdrianBinDC/redatom)";
const READER: &str = "NetNewsWire/6.1";
const FIREFOX: &str = "Mozilla/5.0 (X11; Linux aarch64; rv:140.0) Gecko/20100101 Firefox/140.0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ua {
    Honest,
    Reader,
    Firefox,
    Custom(String),
}

impl Ua {
    pub fn header(&self) -> String {
        match self {
            Ua::Honest => HONEST.to_string(),
            Ua::Reader => READER.to_string(),
            Ua::Firefox => FIREFOX.to_string(),
            Ua::Custom(s) => s.clone(),
        }
    }

    /// Build from the environment. `REDDIT_RSS_UA` (exact header override)
    /// takes priority, then `REDDIT_RSS_UA_SET` picks a preset:
    /// `honest` (default), `reader`, or `firefox`.
    pub fn from_env() -> Ua {
        if let Ok(custom) = std::env::var("REDDIT_RSS_UA") {
            let trimmed = custom.trim();
            if !trimmed.is_empty() {
                return Ua::Custom(trimmed.to_string());
            }
        }
        match std::env::var("REDDIT_RSS_UA_SET")
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("reader") => Ua::Reader,
            Some("firefox") => Ua::Firefox,
            _ => Ua::Honest,
        }
    }

    /// Return one distinct fallback UA identity, or `None` if this is
    /// already the last fallback to try.
    pub fn fallback(&self) -> Option<Ua> {
        match self {
            Ua::Honest => Some(Ua::Reader),
            Ua::Reader => Some(Ua::Firefox),
            Ua::Firefox | Ua::Custom(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honest_falls_back_to_reader() {
        assert_eq!(Ua::Honest.fallback(), Some(Ua::Reader));
    }

    #[test]
    fn reader_falls_back_to_firefox() {
        assert_eq!(Ua::Reader.fallback(), Some(Ua::Firefox));
    }

    #[test]
    fn firefox_has_no_fallback() {
        assert_eq!(Ua::Firefox.fallback(), None);
    }

    #[test]
    fn custom_has_no_fallback() {
        assert_eq!(Ua::Custom("my-ua/1.0".into()).fallback(), None);
    }
}
