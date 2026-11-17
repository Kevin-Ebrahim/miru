use super::{Source, hls, parser};
use crate::{
    model::{Anime, Episode, Mode, Stream},
    network::{Http, http_url},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use url::Url;

const MAX_PAGES: usize = 100;
const MAX_EPISODES: usize = 10_000;

#[derive(Clone)]
pub(super) struct Kaa {
    http: Http,
    base: Url,
}

impl Kaa {
    pub fn new(base: &str) -> Result<Self> {
        let base = http_url(base)?;
        Ok(Self {
            http: Http::new(&base)?,
            base,
        })
    }

    pub fn http(&self) -> Http {
        self.http.clone()
    }

    async fn json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.base.join(path)?;
        ensure!(
            url.origin() == self.base.origin(),
            "KAA catalogue URL changed origin"
        );
        let bytes = self
            .http
            .bytes(url.as_str(), Some(self.base.as_str()), parser::BODY_LIMIT)
            .await?;
        serde_json::from_slice(&bytes).context("Invalid KAA response")
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Anime>> {
        let url = self.base.join("/api/fsearch")?;
        let mut results = Vec::new();
        let mut seen = HashSet::new();
        let mut page = 1;
        loop {
            let bytes = self
                .http
                .post_json(
                    url.as_str(),
                    &serde_json::json!({"page": page, "query": query.trim()}),
                    parser::BODY_LIMIT,
                )
                .await?;
            let data: Search =
                serde_json::from_slice(&bytes).context("Invalid KAA search response")?;
            ensure!(
                data.max_page <= MAX_PAGES,
                "KAA search pagination exceeds limit"
            );
            for show in data.result {
                let anime = show.anime(&self.base)?;
                if seen.insert(anime.id.clone()) {
                    results.push(anime);
                }
            }
            ensure!(results.len() <= 500, "KAA search exceeds result limit");
            if page >= data.max_page {
                break;
            }
            page += 1;
        }
        Ok(results)
    }

    pub async fn synopsis(&self, anime: &Anime) -> Result<String> {
        segment(&anime.id)?;
        let show: Show = self.json(&format!("/api/show/{}", anime.id)).await?;
        ensure!(show.slug == anime.id, "KAA returned a different series");
        Ok(show.synopsis)
    }

    pub async fn episodes(&self, anime: &Anime, mode: Mode) -> Result<Vec<Episode>> {
        segment(&anime.id)?;
        let mut locale = locale(mode);
        let path = format!("/api/show/{}/episodes", anime.id);
        let mut first: EpisodePage = self.json(&format!("{path}?ep=1&lang={locale}")).await?;
        if mode == Mode::Sub && first.pages.is_empty() && first.current_page == 0 {
            let show: Show = self.json(&format!("/api/show/{}", anime.id)).await?;
            ensure!(show.slug == anime.id, "KAA returned a different series");
            if show.locales.iter().any(|locale| locale == "zh-CN") {
                locale = "zh-CN";
                first = self.json(&format!("{path}?ep=1&lang={locale}")).await?;
            }
        }
        ensure!(
            first.pages.len() <= MAX_PAGES,
            "KAA episode pagination exceeds limit"
        );
        if first.pages.is_empty() && first.current_page == 0 && first.result.is_none() {
            bail!(
                "KAA has no episodes in {}. Try switching audio language",
                mode.as_str()
            );
        }
        ensure!(
            first
                .pages
                .iter()
                .enumerate()
                .all(|(i, page)| page.number == i + 1),
            "KAA returned inconsistent episode pagination"
        );
        let starts = first
            .pages
            .iter()
            .map(|page| {
                let number = page
                    .eps
                    .first()
                    .context("KAA page has no starting episode")?;
                let text = match number {
                    PageEpisode::Number(number) => number.to_string(),
                    PageEpisode::Text(text) => text.clone(),
                };
                episode_number(&text)?;
                Ok(text)
            })
            .collect::<Result<Vec<_>>>()?;
        let start = starts
            .first()
            .context("KAA episode pagination has no first page")?;
        if first.current_page != 1 {
            first = self
                .json(&format!("{path}?ep={start}&lang={locale}"))
                .await?;
        }
        ensure!(
            first.current_page == 1,
            "KAA returned the wrong first episode page"
        );
        let mut rows = first.result.context("KAA episode page has no result")?;
        let mut visited = HashSet::from([start.clone()]);
        for (index, page) in starts.into_iter().skip(1).enumerate() {
            ensure!(visited.insert(page.clone()), "KAA repeated an episode page");
            let data: EpisodePage = self
                .json(&format!("{path}?ep={page}&lang={locale}"))
                .await?;
            ensure!(
                data.current_page == index + 2,
                "KAA returned the wrong episode page"
            );
            rows.extend(data.result.context("KAA episode page has no result")?);
            ensure!(
                rows.len() <= MAX_EPISODES,
                "KAA episode count exceeds limit"
            );
        }
        episodes(rows, &anime.id)
    }

    pub async fn resolve(&self, episode: &Episode, mode: Mode) -> Result<Stream> {
        let (show, route) = episode
            .id
            .split_once('/')
            .context("Invalid KAA episode identity")?;
        segment(show)?;
        segment(route)?;
        let mut data: Servers = self
            .json(&format!("/api/show/{show}/episode/{route}"))
            .await?;
        ensure!(
            data.show_slug == show
                && data.episode_string == episode.number
                && format!("ep-{}-{}", data.episode_string, data.slug) == route,
            "KAA returned a different episode"
        );
        if data.servers.is_empty() && mode == Mode::Dub && data.language == "en-US" {
            let anime = Anime {
                id: show.into(),
                provider: Source::Kaa.id(),
                ..Default::default()
            };
            let alternate = self
                .episodes(&anime, Mode::Sub)
                .await?
                .into_iter()
                .find(|item| item.number == episode.number)
                .context("KAA has no alternate source for this episode number")?;
            let (_, route) = alternate
                .id
                .split_once('/')
                .context("Invalid KAA episode identity")?;
            data = self
                .json(&format!("/api/show/{show}/episode/{route}"))
                .await?;
            ensure!(
                data.show_slug == show
                    && data.episode_string == episode.number
                    && format!("ep-{}-{}", data.episode_string, data.slug) == route,
                "KAA returned a different alternate episode"
            );
        }
        let language = match mode {
            Mode::Dub => hls::Language::English,
            Mode::Sub => match hls::language(&data.language) {
                Some(language @ (hls::Language::Japanese | hls::Language::Chinese)) => language,
                _ => bail!("KAA returned an unsupported original audio language"),
            },
        };
        let mut errors = Vec::new();
        for server in data.servers.into_iter().take(4) {
            let result = async {
                let player = player_url(&server.src, &self.base)?;
                let referer = format!("{}/", player.origin().ascii_serialization());
                let page = self
                    .http
                    .bytes(
                        player.as_str(),
                        Some(self.base.as_str()),
                        parser::BODY_LIMIT,
                    )
                    .await?;
                let config: PlayerConfig = parser::parse(parser::Request::KaaPlayer {
                    page: String::from_utf8(page).context("Invalid KAA player text")?,
                    base: player,
                })
                .await?;
                let master = config.manifest;
                let bytes = self
                    .http
                    .bytes(master.as_str(), Some(&referer), parser::BODY_LIMIT)
                    .await?;
                let text = std::str::from_utf8(&bytes).context("Invalid KAA playlist text")?;
                let variants = hls::parse_playlist_language(text, &master, Some(language))?;
                Ok::<_, anyhow::Error>(Stream {
                    request: Some(crate::model::MediaRequest {
                        origin: Some(referer.trim_end_matches('/').into()),
                        user_agent: Some(crate::network::USER_AGENT.into()),
                    }),
                    variants,
                    referer,
                    subtitle: config.subtitle.map(Into::into),
                })
            }
            .await;
            match result {
                Ok(stream) => return Ok(stream),
                Err(error) => errors.push(format!("{error:#}")),
            }
        }
        bail!(
            "KAA has no usable {} source{}",
            mode.as_str(),
            if errors.is_empty() {
                String::new()
            } else {
                format!(": {}", errors.join("; "))
            }
        )
    }
}

fn locale(mode: Mode) -> &'static str {
    match mode {
        Mode::Sub => "ja-JP",
        Mode::Dub => "en-US",
    }
}

fn segment(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            && value != "."
            && value != "..",
        "Invalid KAA source ID"
    );
    Ok(())
}

fn episode_number(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 32
            && value.split('.').count() <= 2
            && value
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())),
        "Invalid KAA episode number"
    );
    Ok(())
}

fn player_url(src: &str, base: &Url) -> Result<Url> {
    let player = http_url(src)?;
    ensure!(
        (player.scheme() == "https"
            && player.host_str() == Some("krussdomi.com")
            && player.port().is_none()
            || player.origin() == base.origin())
            && player.path() == "/cat-player/player",
        "Unsupported KAA player"
    );
    Ok(player)
}

#[derive(Serialize, Deserialize)]
pub(super) struct PlayerConfig {
    manifest: Url,
    subtitle: Option<Url>,
}

pub(super) fn parse_player(page: &str, base: &Url) -> Result<PlayerConfig> {
    let document = parser::document(page)?;
    let selector = scraper::Selector::parse("astro-island[props]").expect("static selector");
    for island in document.select(&selector) {
        let props: serde_json::Value = serde_json::from_str(island.attr("props").unwrap())
            .context("Invalid KAA player properties")?;
        let Some(manifest) = props
            .get("manifest")
            .and_then(|value| value.get(1))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let manifest = http_url(base.join(manifest)?.as_str())?;
        let mut subtitle = None;
        if let Some(tracks) = props
            .get("subtitles")
            .and_then(|v| v.get(1))
            .and_then(|v| v.as_array())
        {
            for track in tracks {
                let track = &track[1];
                let language = track["language"][1].as_str().unwrap_or("");
                if matches!(language, "en" | "en-US" | "eng" | "English") {
                    let src = track["src"][1]
                        .as_str()
                        .context("KAA subtitle has no URL")?;
                    subtitle = Some(http_url(base.join(src)?.as_str())?);
                    break;
                }
            }
        }
        return Ok(PlayerConfig { manifest, subtitle });
    }
    bail!("KAA player has no HLS manifest")
}

#[derive(Deserialize)]
struct Search {
    #[serde(rename = "maxPage", default = "first_page")]
    max_page: usize,
    result: Vec<Show>,
}
fn first_page() -> usize {
    1
}

#[derive(Deserialize)]
struct Show {
    #[serde(default)]
    locales: Vec<String>,
    slug: String,
    title: String,
    #[serde(default)]
    title_en: Option<String>,
    #[serde(default)]
    synopsis: String,
    #[serde(default)]
    poster: Option<Poster>,
}
#[derive(Deserialize)]
struct Poster {
    hq: String,
}
impl Show {
    fn anime(self, base: &Url) -> Result<Anime> {
        segment(&self.slug)?;
        let title = self
            .title_en
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(self.title);
        ensure!(!title.trim().is_empty(), "KAA result has no title");
        let cover = self
            .poster
            .map(|poster| {
                segment(&poster.hq)?;
                Ok::<_, anyhow::Error>(
                    base.join(&format!("/image/poster/{}.webp", poster.hq))?
                        .into(),
                )
            })
            .transpose()?;
        Ok(Anime {
            provider: Source::Kaa.id(),
            id: self.slug,
            title,
            cover,
            description: self.synopsis,
            info: String::new(),
        })
    }
}
#[derive(Deserialize)]
struct EpisodePage {
    current_page: usize,
    pages: Vec<Page>,
    result: Option<Vec<Row>>,
}
#[derive(Deserialize)]
struct Page {
    number: usize,
    eps: Vec<PageEpisode>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum PageEpisode {
    Number(serde_json::Number),
    Text(String),
}

#[derive(Deserialize)]
struct Row {
    slug: String,
    episode_string: String,
    #[serde(default)]
    title: String,
}
fn episodes(rows: Vec<Row>, show: &str) -> Result<Vec<Episode>> {
    ensure!(
        rows.len() <= MAX_EPISODES,
        "KAA episode count exceeds limit"
    );
    let mut seen = HashSet::new();
    let mut numbers = HashSet::new();
    let mut result = Vec::new();
    for row in rows {
        segment(&row.slug)?;
        episode_number(&row.episode_string)?;
        let id = format!("{show}/ep-{}-{}", row.episode_string, row.slug);
        ensure!(
            seen.insert(id.clone()) && numbers.insert(row.episode_string.clone()),
            "KAA repeated an episode"
        );
        result.push(Episode {
            provider: Source::Kaa.id(),
            id,
            number: row.episode_string,
            title: row.title,
        });
    }
    ensure!(
        !result.is_empty(),
        "KAA has no episodes in the requested language"
    );
    Ok(result)
}
#[derive(Deserialize)]
struct Servers {
    show_slug: String,
    slug: String,
    episode_string: String,
    language: String,
    servers: Vec<Server>,
}
#[derive(Deserialize)]
struct Server {
    src: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_props_preserve_relative_urls_and_optional_subtitles() {
        let base = Url::parse("https://krussdomi.com/cat-player/player?id=probe").unwrap();
        let page = r#"<astro-island props='{&quot;manifest&quot;:[0,&quot;/master.m3u8&quot;],&quot;subtitles&quot;:[1,[[0,{&quot;language&quot;:[0,&quot;en&quot;],&quot;src&quot;:[0,&quot;/sub.vtt&quot;]}]]]}'></astro-island>"#;
        let config = parse_player(page, &base).unwrap();
        assert_eq!(
            config.manifest.as_str(),
            "https://krussdomi.com/master.m3u8"
        );
        assert_eq!(
            config.subtitle.unwrap().as_str(),
            "https://krussdomi.com/sub.vtt"
        );
        let config = parse_player(
            r#"<astro-island props='{"manifest":[0,"/master.m3u8"]}'></astro-island>"#,
            &base,
        )
        .unwrap();
        assert!(config.subtitle.is_none());
        for page in [
            "<html>unavailable</html>",
            "<astro-island props='broken'></astro-island>",
            r#"<astro-island props='{"manifest":[0,"file:///etc/passwd"]}'></astro-island>"#,
        ] {
            assert!(parse_player(page, &base).is_err());
        }
    }

    #[test]
    fn catalogue_keeps_real_numbers_and_rejects_ambiguous_or_unsafe_ids() {
        let rows = vec![
            Row {
                slug: "one".into(),
                episode_string: "1".into(),
                title: "One".into(),
            },
            Row {
                slug: "special".into(),
                episode_string: "1.5".into(),
                title: "Special".into(),
            },
            Row {
                slug: "ten".into(),
                episode_string: "10".into(),
                title: "Ten".into(),
            },
        ];
        let result = episodes(rows, "show").unwrap();
        assert_eq!(result[1].id, "show/ep-1.5-special");
        assert_eq!(result[2].number, "10");
        for id in [
            "..",
            "../other",
            "https://evil.test",
            "a?other",
            "a#other",
            "a%2fother",
        ] {
            assert!(segment(id).is_err());
        }
        for number in ["", "1..5", "NaN", "1/2", "1?lang=en-US"] {
            assert!(episode_number(number).is_err());
        }
        let base = Url::parse("https://kaa.lt/").unwrap();
        assert!(player_url("https://krussdomi.com/cat-player/player?id=probe", &base).is_ok());
        for src in [
            "https://evil.test/cat-player/player",
            "https://krussdomi.com.evil.test/cat-player/player",
            "http://127.0.0.1/private",
        ] {
            assert!(player_url(src, &base).is_err());
        }
        let rows = (0..2)
            .map(|i| Row {
                slug: format!("copy-{i}"),
                episode_string: "1".into(),
                title: String::new(),
            })
            .collect();
        assert!(episodes(rows, "show").is_err());
    }
}
