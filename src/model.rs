use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderId(String);

impl Default for ProviderId {
    fn default() -> Self {
        Self("hianime".into())
    }
}

impl TryFrom<String> for ProviderId {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        ensure!(
            !value.is_empty()
                && value.len() <= 64
                && value.bytes().all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || b".-".contains(&byte)),
            "Invalid provider identity"
        );
        Ok(Self(value))
    }
}

impl From<ProviderId> for String {
    fn from(provider: ProviderId) -> Self {
        provider.0
    }
}
impl std::fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceId {
    pub provider: ProviderId,
    pub id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Anime {
    pub id: String,
    #[serde(default)]
    pub provider: ProviderId,
    pub title: String,
    pub cover: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub info: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Episode {
    pub id: String,
    #[serde(default)]
    pub provider: ProviderId,
    pub number: String,
    pub title: String,
}

impl Anime {
    pub fn source_id(&self) -> SourceId {
        SourceId {
            provider: self.provider.clone(),
            id: self.id.clone(),
        }
    }
    pub fn same_source(&self, other: &Self) -> bool {
        self.provider == other.provider && self.id == other.id
    }
}

impl Episode {
    pub fn source_id(&self) -> SourceId {
        SourceId {
            provider: self.provider.clone(),
            id: self.id.clone(),
        }
    }
    pub fn same_source(&self, other: &Self) -> bool {
        self.provider == other.provider && self.id == other.id
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Sub,
    Dub,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sub => "sub",
            Self::Dub => "dub",
        }
    }
    pub fn toggle(&mut self) {
        *self = match self {
            Self::Sub => Self::Dub,
            Self::Dub => Self::Sub,
        };
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Variant {
    pub height: u32,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stream {
    pub variants: Vec<Variant>,
    pub referer: String,
    pub subtitle: Option<String>,
    pub audio: Option<String>,
}

impl Stream {
    pub fn select(&self, quality: &str) -> (&Variant, bool) {
        let best = &self.variants[0]; // provider guarantees at least one, sorted descending
        match quality {
            "best" => (best, false),
            "worst" => (self.variants.last().unwrap_or(best), false),
            other => other
                .trim_end_matches('p')
                .parse::<u32>()
                .ok()
                .and_then(|h| self.variants.iter().find(|v| v.height == h))
                .map(|v| (v, false))
                .unwrap_or((best, true)),
        }
    }
}

pub fn normalized(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// External titles must never inject terminal control sequences.
pub fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Resolve against actual provider order, including decimal specials. 0 = first, -1 = last.
pub fn episode_range(input: &str, episodes: &[Episode]) -> Result<Vec<usize>> {
    if episodes.is_empty() {
        bail!("No episodes available");
    }
    let locate = |value: &str| -> Result<usize> {
        match value {
            "0" => Ok(0),
            "-1" => Ok(episodes.len() - 1),
            _ => episodes
                .iter()
                .position(|e| e.number == value)
                .ok_or_else(|| anyhow::anyhow!("Episode {value} is not available")),
        }
    };
    let input = input.trim();
    if input == "-1" {
        return Ok(vec![episodes.len() - 1]);
    }
    let parts: Vec<_> = input
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        bail!("Enter an episode or range, for example 1-3");
    }
    let mut indices = Vec::new();
    for part in parts {
        if let Some((a, b)) = part.split_once('-').filter(|(a, _)| !a.is_empty()) {
            let (start, end) = (locate(a)?, locate(b)?);
            if start > end {
                bail!("Range must go forward");
            }
            indices.extend(start..=end);
        } else {
            indices.push(locate(part)?);
        }
    }
    indices.sort_unstable();
    indices.dedup();
    Ok(indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn episodes() -> Vec<Episode> {
        ["1", "1.5", "2", "3"]
            .iter()
            .map(|n| Episode {
                provider: Default::default(),
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect()
    }
    #[test]
    fn ranges_follow_provider_order() {
        assert_eq!(episode_range("1-2", &episodes()).unwrap(), [0, 1, 2]);
        assert_eq!(episode_range("0--1", &episodes()).unwrap(), [0, 1, 2, 3]);
        assert_eq!(episode_range("-1", &episodes()).unwrap(), [3]);
        assert_eq!(episode_range("3, 1.5 3", &episodes()).unwrap(), [1, 3]);
    }
    #[test]
    fn invalid_ranges_do_not_partially_play() {
        for input in ["3-1", "5", "", "1-", "1,99"] {
            assert!(episode_range(input, &episodes()).is_err());
        }
    }
    #[test]
    fn normalization_and_control_characters() {
        assert_eq!(normalized("  FRIEREN\t Beyond  "), "frieren beyond");
        assert!(
            !clean("hello\x1b\x07\r\nworld")
                .chars()
                .any(char::is_control)
        );
    }
}
