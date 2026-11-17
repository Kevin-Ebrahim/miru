use super::{Source, parser};
use crate::{
    model::{Anime, Episode, Mode, Stream, Variant, clean, normalized},
    network::{Http, http_url},
};
use anyhow::{Context, Result, ensure};
use lru::LruCache;
use regex::Regex;
use scraper::{ElementRef, Selector};
use std::{
    collections::HashSet,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use url::Url;

type Searches = Arc<Mutex<LruCache<String, (Instant, Vec<Anime>)>>>;

#[derive(Clone)]
pub(super) struct AnimeGg {
    http: Http,
    base: Url,
    searches: Searches,
}

impl AnimeGg {
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

    async fn text(&self, url: &Url) -> Result<String> {
        let bytes = self
            .http
            .bytes(url.as_str(), Some(self.base.as_str()), parser::BODY_LIMIT)
            .await?;
        let page = String::from_utf8(bytes).context("AnimeGG returned invalid text")?;
        ensure!(
            !page.contains("<title>Just a moment"),
            "AnimeGG blocked this request with a browser challenge"
        );
        Ok(page)
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Anime>> {
        let key = normalized(query);
        if let Some((at, results)) = self.searches.lock().await.get(&key)
            && at.elapsed() < Duration::from_secs(600)
        {
            return Ok(results.clone());
        }
        let mut url = self.base.join("/search/")?;
        url.query_pairs_mut().append_pair("q", query.trim());
        let results: Vec<Anime> = parser::parse(parser::Request::GgSearch {
            page: self.text(&url).await?,
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
        let url = page_url(&self.base, &format!("/series/{}", anime.id))?;
        parser::parse(parser::Request::GgSynopsis {
            page: self.text(&url).await?,
        })
        .await
    }

    pub async fn episodes(&self, anime: &Anime, mode: Mode) -> Result<Vec<Episode>> {
        let url = page_url(&self.base, &format!("/series/{}", anime.id))?;
        parser::parse(parser::Request::GgEpisodes {
            page: self.text(&url).await?,
            base: self.base.clone(),
            mode,
        })
        .await
    }

    pub async fn resolve(&self, episode: &Episode, mode: Mode) -> Result<Stream> {
        let url = page_url(&self.base, &format!("/{}", episode.id))?;
        let embeds: Vec<Url> = parser::parse(parser::Request::GgEmbeds {
            page: self.text(&url).await?,
            base: self.base.clone(),
            mode,
        })
        .await?;
        let mut last = None;
        for embed in embeds.into_iter().take(3) {
            let result = async {
                let variants: Vec<Variant> = parser::parse(parser::Request::GgSources {
                    page: self.text(&embed).await?,
                    base: self.base.clone(),
                })
                .await?;
                Ok::<_, anyhow::Error>(Stream {
                    variants,
                    referer: format!("{}/", self.base.origin().ascii_serialization()),
                    subtitle: None,
                })
            }
            .await;
            match result {
                Ok(stream) => return Ok(stream),
                Err(error) => last = Some(error),
            }
        }
        Err(last.context("AnimeGG has no usable embed")?)
            .context("AnimeGG stream resolution failed")
    }
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static selector")
}
fn text(element: ElementRef<'_>) -> String {
    clean(&element.text().collect::<Vec<_>>().join(" "))
}
fn page_url(base: &Url, path: &str) -> Result<Url> {
    let url = http_url(base.join(path)?.as_str())?;
    ensure!(
        url.origin() == base.origin(),
        "AnimeGG page points outside its catalogue"
    );
    Ok(url)
}

pub(super) fn search(page: &str, base: &Url) -> Result<Vec<Anime>> {
    let document = parser::document(page)?;
    let mut seen = HashSet::new();
    let mut results = Vec::new();
    for link in document.select(&selector("a.mse")) {
        let href = link.attr("href").context("AnimeGG result has no link")?;
        let url = page_url(base, href)?;
        let id = url
            .path()
            .strip_prefix("/series/")
            .filter(|id| !id.is_empty())
            .context("AnimeGG result has an invalid series ID")?;
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let title = link
            .select(&selector("h2, strong"))
            .next()
            .map(text)
            .filter(|s| !s.is_empty())
            .context("AnimeGG result has no title")?;
        let cover = link
            .select(&selector("img"))
            .next()
            .and_then(|e| e.attr("src"))
            .map(|src| -> Result<String> { Ok(http_url(base.join(src)?.as_str())?.into()) })
            .transpose()?;
        let mut info = link
            .select(&selector(".infoami"))
            .next()
            .map(text)
            .unwrap_or_default();
        if info.is_empty() {
            // The site nests divs in a paragraph; HTML parsing moves them into .first.
            info = link
                .select(&selector(".first > div"))
                .map(text)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" / ");
        }
        results.push(Anime {
            provider: Source::AnimeGg.id(),
            id: id.into(),
            title,
            cover,
            description: String::new(),
            info,
        });
    }
    ensure!(
        !results.is_empty() || document.select(&selector(".moose.page")).next().is_some(),
        "AnimeGG search markup changed"
    );
    Ok(results)
}

pub(super) fn synopsis(page: &str) -> Result<String> {
    ensure!(
        page.len() <= parser::BODY_LIMIT,
        "AnimeGG synopsis page exceeds size limit"
    );
    let paragraph = Regex::new(r#"(?s)<p\b[^>]*class=["'][^"']*\bptext\b[^"']*["'][^>]*>.*?</p>"#)?;
    let fragment = paragraph
        .find(page)
        .context("Full synopsis unavailable from AnimeGG")?;
    let document = parser::fragment(fragment.as_str())?;
    document
        .select(&selector(".ptext"))
        .next()
        .map(text)
        .filter(|s| !s.is_empty())
        .context("Full synopsis unavailable from AnimeGG")
}

pub(super) fn episodes(page: &str, base: &Url, mode: Mode) -> Result<Vec<Episode>> {
    ensure!(
        page.len() <= parser::BODY_LIMIT,
        "AnimeGG episode page exceeds size limit"
    );
    let list =
        Regex::new(r#"(?s)<ul\b[^>]*class=["'][^"']*\bnewmanga\b[^"']*["'][^>]*>(.*?)</ul>"#)?;
    let body = &list
        .captures(page)
        .context("AnimeGG episode list is missing")?[1];
    let entries = Regex::new(r"(?s)<li\b[^>]*>.*?</li>")?;
    let number = Regex::new(r"(?:^|\s)(\d+(?:\.\d+)?(?:-\d+)?)\s*$")?;
    let mut seen = HashSet::new();
    let mut episodes = Vec::new();
    let audio = selector(match mode {
        Mode::Sub => ".btn-subbed",
        Mode::Dub => ".btn-dubbed",
    });
    let mut found = false;
    for (index, entry) in entries.find_iter(body).enumerate() {
        ensure!(index < 4096, "AnimeGG episode list exceeds entry limit");
        // Large catalogues are parsed one bounded entry at a time inside the isolated worker.
        let document = parser::fragment(entry.as_str())?;
        let item = document
            .select(&selector("li"))
            .next()
            .context("Invalid AnimeGG episode entry")?;
        let Some(link) = item.select(&selector("a.anm_det_pop")).next() else {
            continue;
        };
        found = true;
        if item.select(&audio).next().is_none() {
            continue;
        }
        let url = page_url(
            base,
            link.attr("href").context("AnimeGG episode has no link")?,
        )?;
        let id = url.path().trim_start_matches('/');
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let label = text(link);
        let n = number
            .captures(&label)
            .context("AnimeGG episode number is unsupported")?[1]
            .to_owned();
        if let Some((first, last)) = n.split_once('-') {
            ensure!(
                first.parse::<u32>()? <= last.parse::<u32>()?,
                "AnimeGG episode range is reversed"
            );
        }
        let title = item
            .select(&selector(".anititle"))
            .next()
            .map(text)
            .filter(|s| !s.is_empty())
            .unwrap_or(label);
        episodes.push(Episode {
            provider: Source::AnimeGg.id(),
            id: id.into(),
            number: n,
            title,
        });
    }
    ensure!(
        found,
        "AnimeGG episode markup changed or the series has no episodes"
    );
    // Compare integer length before digits, then fractional digits; keep the original display number.
    episodes.sort_by(|a, b| {
        let parts = |n: &str| {
            let start = n.split_once('-').map_or(n, |(start, _)| start);
            let (whole, fraction) = start.split_once('.').unwrap_or((start, ""));
            (
                whole.trim_start_matches('0').to_owned(),
                fraction.trim_end_matches('0').to_owned(),
            )
        };
        let (aw, af) = parts(&a.number);
        let (bw, bf) = parts(&b.number);
        aw.len().cmp(&bw.len()).then(aw.cmp(&bw)).then(af.cmp(&bf))
    });
    ensure!(
        !episodes.is_empty(),
        "AnimeGG has no {} episodes for this series",
        mode.as_str().to_uppercase()
    );
    Ok(episodes)
}

pub(super) fn embeds(page: &str, base: &Url, mode: Mode) -> Result<Vec<Url>> {
    let document = parser::document(page)?;
    let expected = match mode {
        Mode::Sub => "subbed",
        Mode::Dub => "dubbed",
    };
    let mut embeds = Vec::new();
    for link in document.select(&selector("a[data-toggle=tab][data-id]")) {
        if link.attr("data-version") != Some(expected) {
            continue;
        }
        let id = link.attr("data-id").unwrap();
        ensure!(
            !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
            "AnimeGG embed ID is invalid"
        );
        let url = base.join(&format!("/embed/{id}"))?;
        if !embeds.contains(&url) {
            embeds.push(url);
        }
    }
    ensure!(
        !embeds.is_empty(),
        "AnimeGG has no {} source for this episode",
        mode.as_str().to_uppercase()
    );
    Ok(embeds)
}

pub(super) fn sources(page: &str, base: &Url) -> Result<Vec<Variant>> {
    ensure!(
        page.len() <= parser::BODY_LIMIT,
        "AnimeGG embed exceeds size limit"
    );
    let array = Regex::new(r"(?s)\bvar\s+videoSources\s*=\s*\[(.*?)\]\s*;")?;
    let body = &array
        .captures(page)
        .context("AnimeGG embed has no source list")?[1];
    let object = Regex::new(r"\{([^{}]*)\}")?;
    let file = Regex::new(r#"\bfile\s*:\s*("(?:[^"\\]|\\.)*")"#)?;
    let label = Regex::new(r#"\blabel\s*:\s*("(?:[^"\\]|\\.)*")"#)?;
    let mut variants = Vec::new();
    let mut seen = HashSet::new();
    for item in object.captures_iter(body) {
        let path: String = serde_json::from_str(
            &file
                .captures(&item[1])
                .context("AnimeGG source has no media URL")?[1],
        )?;
        let quality: String = serde_json::from_str(
            &label
                .captures(&item[1])
                .context("AnimeGG source has no quality label")?[1],
        )?;
        let quality = clean(&quality);
        ensure!(
            !quality.is_empty(),
            "AnimeGG source has an empty quality label"
        );
        let url: String = http_url(base.join(&path)?.as_str())?.into();
        if !seen.insert(url.clone()) {
            continue;
        }
        // AnimeGG labels are advertised qualities, not measurements of the media.
        let height = quality.trim_end_matches('p').parse().unwrap_or(0);
        variants.push(Variant {
            format: if Url::parse(&url)?.path().ends_with(".m3u8") {
                crate::model::MediaFormat::Hls
            } else {
                crate::model::MediaFormat::Direct
            },
            label: Some(quality),
            height,
            url,
            audio: None,
        });
    }
    variants.sort_by_key(|v| std::cmp::Reverse(v.height));
    ensure!(
        !variants.is_empty(),
        "AnimeGG embed has no playable sources"
    );
    Ok(variants)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SEARCH: &str = include_str!("../../tests/fixtures/animegg/search.html");
    const SERIES: &str = include_str!("../../tests/fixtures/animegg/series.html");
    const EPISODE: &str = include_str!("../../tests/fixtures/animegg/episode.html");
    const EMBED: &str = include_str!("../../tests/fixtures/animegg/embed.html");
    fn base() -> Url {
        Url::parse("https://example.com/").unwrap()
    }

    #[test]
    fn combined_releases_preserve_labels_and_cover_both_episode_numbers() {
        let page = SERIES.replace("Probe 10", "Probe 10-11");
        let episodes = episodes(&page, &base(), Mode::Sub).unwrap();
        assert_eq!(episodes[2].number, "10-11");
        for input in ["10", "11", "10-11"] {
            assert_eq!(crate::model::episode_range(input, &episodes).unwrap(), [2]);
        }
        assert!(crate::model::episode_range("12", &episodes).is_err());
    }

    #[test]
    fn long_catalogues_remain_bounded_without_a_single_large_dom() {
        let entries: String = (1..=1200).rev().map(|n| format!("<li><div><a class='anm_det_pop' href='/show-{n}'><strong>Show {n}</strong></a><i class='anititle'>Title</i><span class='btn-subbed'>SUB</span><span>HD</span><span>SUB</span><span>SD</span></div></li>")).collect();
        let page = format!("<p class='ptext'>Long series.</p><ul class='newmanga'>{entries}</ul>");
        assert_eq!(synopsis(&page).unwrap(), "Long series.");
        assert!(page.bytes().filter(|b| *b == b'<').count() > 20_000);
        let episodes = episodes(&page, &base(), Mode::Sub).unwrap();
        assert_eq!(episodes.len(), 1200);
        assert_eq!(episodes.last().unwrap().number, "1200");
        let hostile = format!(
            "<ul class='newmanga'><li>{}</li></ul>",
            "<div>".repeat(20_001)
        );
        assert!(super::episodes(&hostile, &base(), Mode::Sub).is_err());
    }

    #[test]
    fn catalogue_preserves_source_ids_titles_and_real_episode_numbers() {
        let results = search(SEARCH, &base()).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Probe & Friends");
        assert_eq!(results[0].provider, Source::AnimeGg.id());
        let repaired = SEARCH.replace("<p class=\"infoami\">Episodes: 3</p>", "<div class='first'><p class='infoami'><div>Episodes: 3</div><div>Status: Completed</div></p></div>");
        assert!(
            search(&repaired, &base()).unwrap()[0]
                .info
                .contains("Episodes: 3")
        );
        assert_eq!(
            results[0].cover.as_deref(),
            Some("https://example.com/cover.jpg")
        );
        assert_eq!(synopsis(SERIES).unwrap(), "A small & unusual adventure.");
        let sub = episodes(SERIES, &base(), Mode::Sub).unwrap();
        assert_eq!(
            sub.iter().map(|e| e.number.as_str()).collect::<Vec<_>>(),
            ["1", "1.5", "10"]
        );
        assert_eq!(sub[1].id, "probe-special");
        let dub = episodes(SERIES, &base(), Mode::Dub).unwrap();
        assert_eq!(dub.len(), 1);
        assert_eq!(dub[0].number, "1");
        assert!(episodes(&SERIES.replace("btn-dubbed", "absent"), &base(), Mode::Dub).is_err());
    }

    #[test]
    fn resolution_selects_requested_audio_and_preserves_advertised_quality() {
        assert!(
            embeds(EPISODE, &base(), Mode::Sub).unwrap()[0]
                .path()
                .ends_with("11")
        );
        assert!(
            embeds(EPISODE, &base(), Mode::Dub).unwrap()[0]
                .path()
                .ends_with("22")
        );
        assert!(embeds(&EPISODE.replace("dubbed", "missing"), &base(), Mode::Dub).is_err());
        let variants = sources(EMBED, &base()).unwrap();
        assert_eq!(
            variants.iter().map(|v| v.height).collect::<Vec<_>>(),
            [720, 360]
        );
        assert_eq!(variants[0].label.as_deref(), Some("720p"));
        let escaped = EMBED.replace("720p", r"720p\u001b");
        assert_eq!(
            sources(&escaped, &base()).unwrap()[0].label.as_deref(),
            Some("720p")
        );
        assert!(variants[1].url.ends_with("?token=a&b=2"));
        assert_eq!(
            sources(&EMBED.replace("720p", "HD"), &base()).unwrap()[1]
                .label
                .as_deref(),
            Some("HD")
        );
    }

    #[test]
    fn malformed_pages_and_external_catalogue_links_fail_without_evaluating_script() {
        assert!(search("<html>changed</html>", &base()).is_err());
        assert!(
            search("<div class='moose page'></div>", &base())
                .unwrap()
                .is_empty()
        );
        assert!(
            search(
                &SEARCH.replace("/series/probe", "https://other.example/series/probe"),
                &base()
            )
            .is_err()
        );
        assert!(sources("var videoSources = [runSomething()];", &base()).is_err());
        assert!(
            sources(
                &EMBED.replace("/media/high.mp4", "file:///etc/passwd"),
                &base()
            )
            .is_err()
        );
        assert!(embeds(&EPISODE.replace("11", "../other"), &base(), Mode::Sub).is_err());
        assert!(
            episodes(
                &SERIES.replace("Probe 10", "Probe 10-1"),
                &base(),
                Mode::Sub
            )
            .is_err()
        );
    }
}
