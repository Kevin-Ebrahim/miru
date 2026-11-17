use crate::model::{Anime, Episode, Mode, ProviderId, Stream, clean, normalized};
use crate::network::{Http, http_url};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use lru::LruCache;
use regex::Regex;
use scraper::Selector;
use serde::Deserialize;
use std::{
    collections::HashSet,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use url::Url;

use super::{hls::parse_playlist, parser};

type SearchCache = Arc<Mutex<LruCache<String, (Instant, Vec<Anime>)>>>;

#[derive(Clone)]
pub(super) struct HiAnime {
    http: Http,
    base: Url,
    searches: SearchCache,
}

impl HiAnime {
    pub fn new(base: &str) -> Result<Self> {
        let base = http_url(base)?;
        Ok(Self {
            http: Http::new(&base)?,
            base,
            searches: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(32).unwrap()))),
        })
    }

    pub fn http(&self) -> Http {
        self.http.clone()
    }

    async fn text(&self, url: &str, referer: Option<&str>) -> Result<String> {
        let bytes = self.http.bytes(url, referer, parser::BODY_LIMIT).await?;
        let text = String::from_utf8(bytes).context("Provider returned invalid text")?;
        ensure!(
            !text.contains("<title>Just a moment"),
            "Provider challenge blocked this request. Check the provider in your browser and retry later"
        );
        Ok(text)
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Anime>> {
        let key = normalized(query);
        if let Some((at, results)) = self.searches.lock().await.get(&key)
            && at.elapsed() < Duration::from_secs(600)
        {
            return Ok(results.clone());
        }
        let mut url = self.base.join("search")?;
        url.query_pairs_mut().append_pair("keyword", query.trim());
        let page = self.text(url.as_str(), None).await?;
        let results: Vec<Anime> = parser::parse(parser::Request::Search {
            page,
            base: self.base.clone(),
        })
        .await?;
        self.searches
            .lock()
            .await
            .put(key, (Instant::now(), results.clone()));
        Ok(results)
    }

    pub async fn synopsis(&self, anime: &Anime) -> Result<String> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Invalid provider base URL"))?
            .pop_if_empty()
            .push(&anime.id);
        parser::parse(parser::Request::Synopsis {
            page: self.text(url.as_str(), None).await?,
        })
        .await
    }

    pub async fn episodes(&self, anime: &Anime) -> Result<Vec<Episode>> {
        let id = anime.id.rsplit('-').next().context("Missing anime ID")?;
        ensure!(
            id.chars().all(|c| c.is_ascii_digit()),
            "Unsupported anime ID; search for this series again"
        );
        let url = self.base.join(&format!("api/theme/episode/list/{id}"))?;
        let response = self.text(url.as_str(), None).await?;
        parser::parse(parser::Request::Episodes {
            page: html_response(&response)?,
            slug: anime.id.clone(),
        })
        .await
    }

    pub async fn resolve(&self, episode: &Episode, mode: Mode) -> Result<Stream> {
        let mut url = self.base.join("api/theme/episode/servers")?;
        url.query_pairs_mut().append_pair("episodeId", &episode.id);
        let response = self.text(url.as_str(), None).await?;
        let embed: Url = parser::parse(parser::Request::Server {
            page: html_response(&response)?,
            mode,
        })
        .await?;
        let referer = format!("{}/", embed.origin().ascii_serialization());
        let page = self.text(embed.as_str(), None).await?;
        let config = parse_embed(&page)?;
        let master = embed.join(&config.src)?;
        let playlist = self.text(master.as_str(), Some(&referer)).await?;
        let variants = parse_playlist(&playlist, &master, None)?;
        let subtitle = config
            .subtitles
            .iter()
            .find(|s| s.default)
            .or_else(|| config.subtitles.iter().find(|s| s.lang == "en"))
            .map(|s| {
                embed
                    .join(&s.src)
                    .map_err(anyhow::Error::from)
                    .and_then(|u| http_url(u.as_str()))
                    .map(String::from)
            })
            .transpose()?;
        Ok(Stream {
            request: None,
            variants,
            referer,
            subtitle,
        })
    }
}

fn selector(s: &str) -> Selector {
    Selector::parse(s).expect("static CSS selector")
}

pub(super) fn parse_search(page: &str, base: &Url) -> Result<Vec<Anime>> {
    // The sidebar repeats cards. Exclude it just as the reference implementation does.
    let page = page.split("id=\"main-sidebar\"").next().unwrap_or(page);
    let document = parser::document(page)?;

    let mut seen = HashSet::new();
    let mut results = Vec::new();

    for item in document.select(&selector(".flw-item")) {
        let Some(link) = item.select(&selector(".film-name a")).next() else {
            continue;
        };
        let Some(href) = link.attr("href") else {
            continue;
        };

        let url = base.join(href)?;
        let id = url
            .path()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default();
        if id.is_empty() || !seen.insert(id.to_owned()) {
            continue;
        }

        let title = clean(
            link.attr("title")
                .unwrap_or(&link.text().collect::<String>()),
        );
        let cover = item
            .select(&selector(".film-poster img"))
            .next()
            .and_then(|img| img.attr("data-src").or_else(|| img.attr("src")))
            .and_then(|url| base.join(url).ok())
            .map(String::from);
        let description = item
            .select(&selector(".description"))
            .next()
            .map(|e| clean(&e.text().collect::<String>()))
            .unwrap_or_default();

        let mut info = item
            .select(&selector(".fdi-item"))
            .map(|e| clean(&e.text().collect::<String>()))
            .collect::<Vec<_>>();
        for (css, label) in [(".tick-sub", "SUB"), (".tick-dub", "DUB")] {
            if let Some(e) = item.select(&selector(css)).next() {
                info.push(format!("{label} {}", clean(&e.text().collect::<String>())));
            }
        }

        results.push(Anime {
            provider: ProviderId::default(),
            id: id.into(),
            title,
            cover,
            description,
            info: info.join(" / "),
        });
    }

    ensure!(
        !results.is_empty()
            || page.contains("film_list-wrap")
            || document
                .select(&selector(".film_list p"))
                .any(|p| { normalized(&p.text().collect::<String>()) == "no animes found." })
            || page.contains("No results")
            || page.contains("no-results"),
        "Search markup changed. Check the provider or update the parser"
    );

    Ok(results)
}

pub(super) fn parse_synopsis(page: &str) -> Result<String> {
    let document = parser::document(page)?;
    let description = document
        .select(&selector(".film-description .text"))
        .next()
        .map(|element| {
            let mut text = String::new();
            let mut pending: Vec<_> = element.children().rev().map(|node| (node, false)).collect();
            while let Some((node, closing)) = pending.pop() {
                if closing {
                    text.push('\n');
                } else if let Some(value) = node.value().as_text() {
                    text.extend(
                        value
                            .chars()
                            .map(|c| if c.is_whitespace() { ' ' } else { c }),
                    );
                } else if let Some(child) = scraper::ElementRef::wrap(node) {
                    if matches!(child.value().name(), "p" | "div" | "br") {
                        text.push('\n');
                        pending.push((node, true));
                    }
                    pending.extend(node.children().rev().map(|node| (node, false)));
                }
            }
            text.lines()
                .map(clean)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .filter(|text| !text.is_empty())
        .context("Full synopsis unavailable from this provider")?;
    Ok(description)
}

fn html_response(response: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Response {
        html: String,
    }
    Ok(serde_json::from_str::<Response>(response)
        .context("Provider returned an invalid episode response")?
        .html)
}

pub(super) fn parse_episodes(html: &str, slug: &str) -> Result<Vec<Episode>> {
    let doc = parser::fragment(html)?;
    let mut episodes = Vec::new();
    let mut seen = HashSet::new();
    for e in doc.select(&selector(".ep-item")) {
        let (Some(id), Some(number), Some(href)) =
            (e.attr("data-id"), e.attr("data-number"), e.attr("href"))
        else {
            continue;
        };
        // Old ani-cli providers reused numeric IDs. Never play a different series by accident.
        if !href
            .split('?')
            .next()
            .unwrap_or_default()
            .ends_with(&format!("/watch/{slug}"))
            || !seen.insert(id.to_owned())
        {
            continue;
        }
        episodes.push(Episode {
            provider: Default::default(),
            id: id.into(),
            number: clean(number),
            title: clean(e.attr("title").unwrap_or("")),
        });
    }

    ensure!(
        !episodes.is_empty(),
        "No episodes found for this series. Try searching for it again"
    );
    Ok(episodes)
}

pub(super) fn parse_server(html: &str, mode: Mode) -> Result<Url> {
    let doc = parser::fragment(html)?;
    for e in doc.select(&selector(".server-item")) {
        if e.attr("data-type") == Some(mode.as_str())
            && e.attr("data-server-name") == Some("ZokoAnime")
        {
            let bytes = STANDARD
                .decode(e.attr("data-hash").unwrap_or_default())
                .context("Invalid server URL encoding")?;
            return http_url(&String::from_utf8(bytes)?);
        }
    }
    bail!(
        "No {} source on ZokoAnime. Try the other language or another episode",
        mode.as_str().to_uppercase()
    )
}

#[derive(Deserialize)]
pub struct EmbedConfig {
    src: String,
    #[serde(default)]
    subtitles: Vec<Subtitle>,
}
#[derive(Deserialize)]
struct Subtitle {
    src: String,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    lang: String,
}

pub(super) fn parse_embed(page: &str) -> Result<EmbedConfig> {
    let re = Regex::new(r#"window\.__P\s*=\s*["']([^"']+)["']"#)?;
    let blob = re
        .captures(page)
        .context("Player configuration changed; stream could not be resolved")?;
    let key = b"otaku-embed-v1";
    let mut bytes = STANDARD
        .decode(&blob[1])
        .context("Invalid player encoding")?;
    for (i, b) in bytes.iter_mut().enumerate() {
        *b ^= key[i % key.len()];
    }
    serde_json::from_slice(&bytes).context("Invalid player configuration")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_synopsis_keeps_paragraphs_and_inline_text_without_marketing_metadata() {
        let text = parse_synopsis(
            r#"<meta name="description" content="Watch now!">
            <div class="film-description"><div class="text"><p>A <em>long</em> journey &amp;
            an unexpected friend.</p><p>A second chapter<br>ends here.</p></div></div>"#,
        )
        .unwrap();
        assert_eq!(
            text,
            "A long journey & an unexpected friend.\n\nA second chapter\n\nends here."
        );
        assert!(parse_synopsis("<meta name=description content='Watch now!'>").is_err());
        assert!(
            parse_synopsis("<div class=film-description><div class=text> </div></div>").is_err()
        );
    }

    #[test]
    fn search_handles_entities_missing_art_and_sidebar() {
        let results = parse_search(
            include_str!("../../tests/fixtures/search.html"),
            &Url::parse("https://example.org").unwrap(),
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Frieren: Beyond Journey's End");
        assert_eq!(
            results[0].cover.as_deref(),
            Some("https://example.org/covers/one.jpg")
        );
        assert_eq!(results[1].title, "日本語 & English");
        assert!(results[1].cover.is_none());
    }

    #[test]
    fn empty_live_search_is_not_a_markup_error() {
        assert!(
            parse_search(
                include_str!("../../tests/fixtures/empty-search.html"),
                &Url::parse("https://example.org").unwrap()
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn changed_search_markup_is_an_error() {
        assert!(
            parse_search(
                "<html>Service unavailable</html>",
                &Url::parse("https://example.org").unwrap()
            )
            .is_err()
        );
        assert!(
            parse_search(
                "<div class=\"film_list-wrap\"></div>",
                &Url::parse("https://example.org").unwrap()
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn episodes_reject_another_series_with_reused_id() {
        let episodes = parse_episodes(
            include_str!("../../tests/fixtures/episodes.html"),
            "frieren-481",
        )
        .unwrap();
        assert_eq!(episodes.len(), 3);
        assert_eq!(episodes[0].title, "The Journey's End");
        assert_eq!(episodes[1].number, "1.5");
        assert!(
            parse_episodes(
                include_str!("../../tests/fixtures/episodes.html"),
                "old-481"
            )
            .is_err()
        );
    }

    #[test]
    fn server_selects_language_and_supported_host() {
        let hash = STANDARD.encode("https://example.org/stream/sub");
        let html = format!(
            r#"<div data-hash="{hash}" class="server-item" data-server-name="ZokoAnime" data-type="sub"></div>"#
        );
        assert_eq!(
            parse_server(&html, Mode::Sub).unwrap().path(),
            "/stream/sub"
        );
        assert!(parse_server(&html, Mode::Dub).is_err());
    }

    #[test]
    fn player_config_decodes_without_executing_javascript() {
        let data = br#"{"src":"https://example.org/master.m3u8","subtitles":[]}"#;
        let key = b"otaku-embed-v1";
        let bytes: Vec<_> = data
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ key[i % key.len()])
            .collect();
        let html = format!("window.__P = '{}';", STANDARD.encode(bytes));
        assert_eq!(
            parse_embed(&html).unwrap().src,
            "https://example.org/master.m3u8"
        );
        assert!(parse_embed("window.__P='bad'").is_err());
    }
}
