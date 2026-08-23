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
}
