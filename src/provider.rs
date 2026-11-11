use crate::model::{Anime, Audio, Episode, Mode, ProviderId, Stream, Variant, clean, normalized};
use crate::network::{Http, http_url};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use lru::LruCache;
use regex::Regex;
use scraper::Selector;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use url::Url;

mod parser;
pub use parser::run_helper as run_parser_helper;

type SearchCache = Arc<Mutex<LruCache<String, (Instant, Vec<Anime>)>>>;

#[derive(Clone)]
pub struct Provider {
    http: Http,
    id: ProviderId,
    base: Url,
    searches: SearchCache,
}

impl Provider {
    pub fn new(base: &str) -> Result<Self> {
        let base = http_url(base)?;
        Ok(Self {
            http: Http::new(&base)?,
            id: ProviderId::default(),
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
        ensure!(
            anime.provider == self.id,
            "This item belongs to provider {}; the active provider is {}",
            anime.provider,
            self.id
        );
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
        ensure!(
            anime.provider == self.id,
            "This item belongs to provider {}; the active provider is {}",
            anime.provider,
            self.id
        );
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
        ensure!(
            episode.provider == self.id,
            "This item belongs to provider {}; the active provider is {}",
            episode.provider,
            self.id
        );
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
        let variants = parse_playlist(&playlist, &master)?;
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
        for variant in &variants {
            self.http.validate(&variant.url)?;
            if let Some(audio) = variant.audio_url() {
                self.http.validate(audio)?;
            }
        }
        if let Some(url) = &subtitle {
            self.http.validate(url)?;
        }
        Ok(Stream {
            variants,
            referer,
            subtitle,
        })
    }
}

fn selector(s: &str) -> Selector {
    Selector::parse(s).expect("static CSS selector")
}
pub fn parse_search(page: &str, base: &Url) -> Result<Vec<Anime>> {
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

pub fn parse_synopsis(page: &str) -> Result<String> {
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

pub fn parse_episodes(html: &str, slug: &str) -> Result<Vec<Episode>> {
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

pub fn parse_server(html: &str, mode: Mode) -> Result<Url> {
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

pub fn parse_embed(page: &str) -> Result<EmbedConfig> {
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

fn hls_attributes(mut list: &str) -> Result<HashMap<&str, &str>> {
    let mut attributes = HashMap::new();
    while !list.is_empty() {
        let (name, value) = list
            .split_once('=')
            .context("Malformed HLS attribute list")?;
        let name = name.trim();
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-'),
            "Invalid HLS attribute name"
        );
        let value = value.trim_start();
        let (value, rest) = if let Some(quoted) = value.strip_prefix('"') {
            let (value, rest) = quoted
                .split_once('"')
                .context("Unterminated HLS quoted attribute")?;
            let rest = rest.trim_start();
            let rest = if rest.is_empty() {
                rest
            } else {
                rest.strip_prefix(',')
                    .context("Malformed HLS attribute separator")?
            };
            (value, rest)
        } else {
            let (value, rest) = value.split_once(',').unwrap_or((value, ""));
            (value.trim(), rest)
        };
        ensure!(
            attributes.insert(name, value).is_none(),
            "Duplicate HLS attribute {name:?}"
        );
        list = rest.trim_start();
    }
    Ok(attributes)
}

pub fn parse_playlist(text: &str, master: &Url) -> Result<Vec<Variant>> {
    ensure!(
        text.trim_start().starts_with("#EXTM3U"),
        "Stream host did not return an HLS playlist"
    );
    struct Rendition {
        is_default: bool,
        autoselect: bool,
        url: Option<String>,
    }
    // Shared renditions and relative URLs can expand far beyond the input text.
    let mut remaining = parser::BODY_LIMIT;
    let mut reserve = |bytes: usize| -> Result<()> {
        remaining = remaining
            .checked_sub(bytes)
            .context("Resolved HLS rendition data exceeds size limit")?;
        Ok(())
    };
    let mut groups: HashMap<&str, Rendition> = HashMap::new();
    for line in text.lines().map(str::trim) {
        let Some(list) = line.strip_prefix("#EXT-X-MEDIA:") else {
            continue;
        };
        let attributes = hls_attributes(list)?;
        if attributes.get("TYPE") != Some(&"AUDIO") {
            continue;
        }
        let group = attributes
            .get("GROUP-ID")
            .copied()
            .filter(|group| !group.is_empty())
            .context("HLS audio rendition has no group identity")?;
        let rendition = Rendition {
            is_default: attributes.get("DEFAULT") == Some(&"YES"),
            autoselect: attributes.get("AUTOSELECT") == Some(&"YES"),
            url: attributes
                .get("URI")
                .map(|uri| -> Result<String> {
                    let url = http_url(master.join(uri)?.as_str())?.to_string();
                    reserve(url.len())?;
                    Ok(url)
                })
                .transpose()?,
        };
        if let Some(current) = groups.get_mut(group) {
            ensure!(
                !(current.is_default && rendition.is_default),
                "Multiple default audio renditions in HLS group {group:?}"
            );
            if (rendition.is_default, rendition.autoselect)
                > (current.is_default, current.autoselect)
            {
                *current = rendition;
            }
        } else {
            groups.insert(group, rendition);
        }
    }
    let mut variants = Vec::new();
    let mut pending = None;
    for line in text.lines().map(str::trim) {
        if let Some(list) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let attributes = hls_attributes(list)?;
            let height = attributes
                .get("RESOLUTION")
                .and_then(|resolution| resolution.split_once('x'))
                .and_then(|(width, height)| {
                    width.parse::<u32>().ok()?;
                    height.parse().ok()
                })
                .unwrap_or(0);
            let audio = attributes
                .get("AUDIO")
                .map(|&group| -> Result<Audio> {
                    let rendition = groups.get(group).with_context(|| {
                        format!("HLS variant references missing audio group {group:?}")
                    })?;
                    reserve(group.len())?;
                    if let Some(url) = &rendition.url {
                        reserve(url.len())?;
                    }
                    Ok(Audio {
                        group: group.into(),
                        url: rendition.url.clone(),
                    })
                })
                .transpose()?;
            pending = Some((height, audio));
        } else if !line.is_empty()
            && !line.starts_with('#')
            && let Some((height, audio)) = pending.take()
        {
            let url = http_url(master.join(line)?.as_str())?.to_string();
            reserve(url.len())?;
            variants.push(Variant { height, url, audio });
        }
    }
    if variants.is_empty() && text.contains("#EXTINF:") {
        reserve(master.as_str().len())?;
        variants.push(Variant {
            height: 0,
            url: master.to_string(),
            audio: None,
        });
    }
    ensure!(
        !variants.is_empty(),
        "No playable streams in the HLS playlist"
    );
    variants.sort_by_key(|v| std::cmp::Reverse(v.height));
    Ok(variants)
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
            include_str!("../tests/fixtures/search.html"),
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
                include_str!("../tests/fixtures/empty-search.html"),
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
            include_str!("../tests/fixtures/episodes.html"),
            "frieren-481",
        )
        .unwrap();
        assert_eq!(episodes.len(), 3);
        assert_eq!(episodes[0].title, "The Journey's End");
        assert_eq!(episodes[1].number, "1.5");
        assert!(
            parse_episodes(include_str!("../tests/fixtures/episodes.html"), "old-481").is_err()
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
    #[test]
    fn hls_resolves_relative_urls_audio_and_quality() {
        let master = Url::parse("https://cdn.example/path/master.m3u8?token=test").unwrap();
        let variants =
            parse_playlist(include_str!("../tests/fixtures/master.m3u8"), &master).unwrap();
        assert_eq!(
            variants.iter().map(|v| v.height).collect::<Vec<_>>(),
            [1080, 720, 360]
        );
        assert_eq!(variants[1].url, "https://cdn.example/path/720/index.m3u8");
        assert_eq!(
            variants[0].audio_url(),
            Some("https://cdn.example/audio/index.m3u8")
        );
        assert_eq!(variants[1].audio_url(), variants[0].audio_url());
        assert!(variants[2].audio.is_none());
        let stream = Stream {
            variants,
            subtitle: None,
            referer: String::new(),
        };
        assert_eq!(stream.select("worst").0.height, 360);
        assert_eq!(stream.select("720p").0.height, 720);
        assert!(stream.select("480p").1);
    }
    #[test]
    fn media_playlist_and_invalid_responses() {
        let url = Url::parse("https://example.org/a.m3u8").unwrap();
        assert_eq!(
            parse_playlist("#EXTM3U\n#EXTINF:2.0,\nseg.ts", &url)
                .unwrap()
                .len(),
            1
        );
        assert!(parse_playlist("<html>error</html>", &url).is_err());
        assert!(http_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn selected_video_keeps_its_audio_group_in_player_arguments() {
        let master = Url::parse("https://cdn.example/path/master.m3u8").unwrap();
        let variants =
            parse_playlist(include_str!("../tests/fixtures/audio-groups.m3u8"), &master).unwrap();
        let stream = Stream {
            variants,
            subtitle: Some("https://cdn.example/subtitle.vtt".into()),
            referer: String::new(),
        };
        let exported = serde_json::to_value(&stream).unwrap();
        assert_eq!(exported["audio"], "https://cdn.example/path/first.m3u8");
        assert_eq!(exported["referer"], "");
        assert_eq!(exported["subtitle"], "https://cdn.example/subtitle.vtt");
        assert_eq!(exported["variants"][0]["height"], 1080);
        assert_eq!(
            exported["variants"][0]["url"],
            "https://cdn.example/path/video-high.m3u8"
        );
        assert_eq!(exported["variants"][0]["audio"]["group"], "a1");
        assert_eq!(exported["variants"][0]["audio"]["url"], exported["audio"]);
        assert_eq!(exported["variants"][1]["audio"]["group"], "a2");
        for (quality, video, audio) in [
            ("best", "video-high.m3u8", "first.m3u8"),
            ("1080p", "video-high.m3u8", "first.m3u8"),
            ("worst", "video-low.m3u8", "second.m3u8"),
            ("480", "video-low.m3u8", "second.m3u8"),
            ("720p", "video-high.m3u8", "first.m3u8"),
        ] {
            for player in ["mpv", "vlc"] {
                let arguments = crate::player::player_args(player, &stream, quality, "Probe");
                let audio = format!("https://cdn.example/path/{audio}");
                let expected = if player == "vlc" {
                    format!(":input-slave={audio}#https://cdn.example/subtitle.vtt")
                } else {
                    format!("--audio-file={audio}")
                };
                assert!(
                    arguments.contains(&expected),
                    "{player} {quality}: {arguments:?}"
                );
                assert!(arguments.contains(&format!("https://cdn.example/path/{video}")));
            }
        }
    }

    #[test]
    fn hls_audio_groups_can_follow_video_and_contain_quoted_commas() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let variants = parse_playlist(
            r#"#EXTM3U
            #EXT-X-STREAM-INF:RESOLUTION=1920x1080,AUDIO="late,group"
            video.m3u8
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="late,group",NAME="DEFAULT=YES, stereo",URI="other.m3u8"
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="late,group",NAME="Chosen",DEFAULT=YES,URI="chosen.m3u8?token=x,y"
            "#,
            &master,
        )
        .unwrap();
        assert_eq!(
            variants[0].audio_url(),
            Some("https://cdn.example/chosen.m3u8?token=x,y")
        );
        assert_eq!(variants[0].audio.as_ref().unwrap().group, "late,group");
    }

    #[test]
    fn hls_optional_default_keeps_the_available_audio_rendition() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let expected = "https://cdn.example/audio.m3u8";
        for default in ["DEFAULT=YES,", "", "DEFAULT=NO,"] {
            let text = format!(
                "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Main\",AUTOSELECT=YES,{default}URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:RESOLUTION=128x96,AUDIO=\"a\"\nvideo.m3u8\n"
            );
            let stream = Stream {
                variants: parse_playlist(&text, &master).unwrap(),
                subtitle: None,
                referer: String::new(),
            };
            assert_eq!(stream.select("best").0.audio_url(), Some(expected));
            assert_eq!(serde_json::to_value(&stream).unwrap()["audio"], expected);
            for player in ["mpv", "vlc"] {
                let arguments = crate::player::player_args(player, &stream, "best", "Probe");
                let audio = if player == "vlc" {
                    format!(":input-slave={expected}")
                } else {
                    format!("--audio-file={expected}")
                };
                assert!(arguments.contains(&audio), "{default:?}: {arguments:?}");
            }
        }
    }

    #[test]
    fn hls_audio_priority_is_default_then_autoselect_then_first() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        for renditions in [
            [
                ("AUTOSELECT=NO", "other.m3u8"),
                ("AUTOSELECT=YES", "audio.m3u8"),
            ],
            [
                ("AUTOSELECT=YES", "other.m3u8"),
                ("DEFAULT=YES,AUTOSELECT=YES", "audio.m3u8"),
            ],
            [
                ("AUTOSELECT=YES", "audio.m3u8"),
                ("AUTOSELECT=YES", "other.m3u8"),
            ],
        ] {
            let mut text = "#EXTM3U\n".to_owned();
            for (index, (flags, uri)) in renditions.into_iter().enumerate() {
                text.push_str(&format!(
                    "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Track {index}\",{flags},URI=\"{uri}\"\n"
                ));
            }
            text.push_str("#EXT-X-STREAM-INF:RESOLUTION=128x96,AUDIO=\"a\"\nvideo.m3u8\n");
            let variants = parse_playlist(&text, &master).unwrap();
            assert_eq!(
                variants[0].audio_url(),
                Some("https://cdn.example/audio.m3u8")
            );
        }
    }

    #[test]
    fn hls_missing_defaults_and_embedded_audio_stay_within_their_groups() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let variants = parse_playlist(
            r#"#EXTM3U
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="first",NAME="First",URI="first.m3u8"
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="first",NAME="Second",URI="second.m3u8"
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="embedded",NAME="Alternate",URI="other.m3u8"
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="embedded",NAME="Embedded",DEFAULT=YES
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="unused",NAME="Unrelated",DEFAULT=YES,URI="unrelated.m3u8"
            #EXT-X-STREAM-INF:RESOLUTION=1920x1080,AUDIO="first"
            high.m3u8
            #EXT-X-STREAM-INF:RESOLUTION=1280x720,AUDIO="embedded"
            medium.m3u8
            #EXT-X-STREAM-INF:RESOLUTION=640x360
            low.m3u8
            "#,
            &master,
        )
        .unwrap();
        assert_eq!(
            variants[0].audio_url(),
            Some("https://cdn.example/first.m3u8")
        );
        assert_eq!(variants[1].audio.as_ref().unwrap().group, "embedded");
        assert!(variants[1].audio_url().is_none());
        assert!(variants[2].audio.is_none());
        let stream = Stream {
            variants,
            subtitle: None,
            referer: String::new(),
        };
        for quality in ["720p", "worst"] {
            for player in ["mpv", "vlc"] {
                let arguments = crate::player::player_args(player, &stream, quality, "Probe");
                assert!(arguments.iter().all(|argument| {
                    !argument.starts_with("--audio-file=") && !argument.starts_with(":input-slave=")
                }));
            }
        }
        let embedded = Stream {
            variants: vec![stream.variants[1].clone()],
            subtitle: None,
            referer: String::new(),
        };
        let exported = serde_json::to_value(embedded).unwrap();
        assert!(exported["audio"].is_null());
        assert_eq!(exported["variants"][0]["audio"]["group"], "embedded");
        assert!(exported["variants"][0]["audio"]["url"].is_null());
    }

    #[test]
    fn hls_rejects_ambiguous_or_invalid_audio_group_definitions() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let fixture = include_str!("../tests/fixtures/audio-groups.m3u8");
        for (text, reason) in [
            (
                fixture.replace("AUDIO=\"a1\"", "AUDIO=\"missing\""),
                "missing audio group",
            ),
            (
                fixture.replace("GROUP-ID=\"a2\"", "GROUP-ID=\"a1\""),
                "Multiple default audio",
            ),
            (fixture.replace("GROUP-ID=\"a1\",", ""), "no group identity"),
            (
                fixture.replace("AUDIO=\"a1\"", "AUDIO=\"a1\",AUDIO=\"a2\""),
                "Duplicate HLS attribute",
            ),
            (
                "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"unterminated".into(),
                "Unterminated HLS",
            ),
        ] {
            let error = parse_playlist(&text, &master).unwrap_err();
            assert!(error.to_string().contains(reason), "{error:#}");
        }
    }

    #[test]
    fn hls_shared_renditions_cannot_expand_beyond_the_metadata_budget() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let mut playlist = format!(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"shared\",NAME=\"Large\",DEFAULT=YES,URI=\"{}.m3u8\"\n",
            "a".repeat(128 * 1024)
        );
        for index in 0..40 {
            playlist.push_str(&format!(
                "#EXT-X-STREAM-INF:RESOLUTION=1920x1080,AUDIO=\"shared\"\nvideo-{index}.m3u8\n"
            ));
        }
        assert!(playlist.len() < parser::BODY_LIMIT);
        let result = parse_playlist(&playlist, &master);
        assert!(
            result.is_err(),
            "Shared audio expanded beyond the metadata budget"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("rendition data exceeds")
        );
    }
    #[tokio::test]
    async fn foreign_provider_items_are_rejected_before_network_access() {
        let provider = Provider::new("http://127.0.0.1:9/").unwrap();
        let foreign: ProviderId = "other".to_owned().try_into().unwrap();
        let anime = Anime {
            provider: foreign.clone(),
            id: "series-1".into(),
            ..Default::default()
        };
        let episode = Episode {
            provider: foreign,
            id: "1".into(),
            number: "1".into(),
            title: String::new(),
        };
        assert!(
            provider
                .episodes(&anime)
                .await
                .unwrap_err()
                .to_string()
                .contains("active provider")
        );
        assert!(
            provider
                .synopsis(&anime)
                .await
                .unwrap_err()
                .to_string()
                .contains("active provider")
        );
        assert!(
            provider
                .resolve(&episode, Mode::Sub)
                .await
                .unwrap_err()
                .to_string()
                .contains("active provider")
        );
    }
}
