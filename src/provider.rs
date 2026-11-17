mod animegg;
mod hianime;
mod hls;
mod parser;

pub use parser::run_helper as run_parser_helper;

use crate::{
    model::{Anime, Episode, Mode, ProviderId},
    network::{Http, ResolvedMedia},
};
use anyhow::{Context, Result, bail};

pub fn downloads_supported(id: &ProviderId) -> bool {
    *id == Source::HiAnime.id()
}

pub fn require_downloads(id: &ProviderId) -> Result<()> {
    anyhow::ensure!(
        downloads_supported(id),
        "Downloads are unavailable for provider {id}. Use HiAnime for downloads."
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Source {
    #[default]
    #[value(name = "hianime")]
    HiAnime,
    #[value(name = "animegg")]
    AnimeGg,
}

impl Source {
    pub fn id(self) -> ProviderId {
        self.name()
            .to_owned()
            .try_into()
            .expect("built-in provider ID")
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::HiAnime => "hianime",
            Self::AnimeGg => "animegg",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::HiAnime => "HiAnime",
            Self::AnimeGg => "AnimeGG",
        }
    }
    fn base(self) -> &'static str {
        match self {
            Self::HiAnime => "https://hianime.at/",
            Self::AnimeGg => "https://www.animegg.org/",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::HiAnime => Self::AnimeGg,
            Self::AnimeGg => Self::HiAnime,
        }
    }
}

#[derive(Clone)]
enum Adapter {
    HiAnime(hianime::HiAnime),
    AnimeGg(animegg::AnimeGg),
}

#[derive(Clone)]
pub struct Providers {
    adapters: Vec<(ProviderId, Adapter)>,
}

impl Providers {
    #[cfg(test)]
    pub fn new(base: &str) -> Result<Self> {
        Self::configured(Source::HiAnime, Some(base))
    }

    pub fn configured(source: Source, base: Option<&str>) -> Result<Self> {
        let endpoint = |kind: Source| {
            if kind == source {
                base.unwrap_or(kind.base())
            } else {
                kind.base()
            }
        };
        Ok(Self {
            adapters: vec![
                (
                    Source::HiAnime.id(),
                    Adapter::HiAnime(hianime::HiAnime::new(endpoint(Source::HiAnime))?),
                ),
                (
                    Source::AnimeGg.id(),
                    Adapter::AnimeGg(animegg::AnimeGg::new(endpoint(Source::AnimeGg))?),
                ),
            ],
        })
    }

    fn adapter(&self, id: &ProviderId) -> Result<&Adapter> {
        match self
            .adapters
            .iter()
            .find(|(registered, _)| registered == id)
        {
            Some((_, adapter)) => Ok(adapter),
            None => bail!(
                "Provider {id} is unavailable. Choose another provider and search for this title again; its history is preserved"
            ),
        }
    }

    pub fn http(&self, id: &ProviderId) -> Result<Http> {
        match self.adapter(id)? {
            Adapter::HiAnime(provider) => Ok(provider.http()),
            Adapter::AnimeGg(provider) => Ok(provider.http()),
        }
    }

    pub async fn search(&self, id: &ProviderId, query: &str) -> Result<Vec<Anime>> {
        async {
            match self.adapter(id)? {
                Adapter::HiAnime(provider) => provider.search(query).await,
                Adapter::AnimeGg(provider) => provider.search(query).await,
            }
        }
        .await
        .with_context(|| format!("{} catalogue search failed", id))
    }

    pub async fn synopsis(&self, anime: &Anime) -> Result<String> {
        async {
            match self.adapter(&anime.provider)? {
                Adapter::HiAnime(provider) => provider.synopsis(anime).await,
                Adapter::AnimeGg(provider) => provider.synopsis(anime).await,
            }
        }
        .await
        .with_context(|| format!("{} details failed", anime.provider))
    }

    pub fn mode_specific_episodes(&self, id: &ProviderId) -> bool {
        matches!(self.adapter(id), Ok(Adapter::AnimeGg(_)))
    }

    pub async fn episodes(&self, anime: &Anime, mode: Mode) -> Result<Vec<Episode>> {
        async {
            match self.adapter(&anime.provider)? {
                Adapter::HiAnime(provider) => provider.episodes(anime).await,
                Adapter::AnimeGg(provider) => provider.episodes(anime, mode).await,
            }
        }
        .await
        .with_context(|| format!("{} episode listing failed", anime.provider))
    }

    pub async fn resolve(&self, episode: &Episode, mode: Mode) -> Result<ResolvedMedia> {
        async {
            match self.adapter(&episode.provider)? {
                Adapter::HiAnime(provider) => {
                    let stream = provider.resolve(episode, mode).await?;
                    provider.http().resolve_media(stream)
                }
                Adapter::AnimeGg(provider) => {
                    let stream = provider.resolve(episode, mode).await?;
                    provider.http().resolve_media(stream)
                }
            }
        }
        .await
        .with_context(|| format!("{} resolution failed", episode.provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_keeps_provider_network_contexts_separate() {
        let local_id: ProviderId = "second".to_owned().try_into().unwrap();
        let mut providers = Providers::new("http://127.0.0.1:9/").unwrap();
        // Two configured contexts exercise routing before a second site adapter exists.
        providers.adapters.push((
            local_id.clone(),
            Adapter::HiAnime(hianime::HiAnime::new("http://127.0.0.2:9/").unwrap()),
        ));
        for (id, allowed, denied) in [
            (
                ProviderId::default(),
                "http://127.0.0.1/video",
                "http://127.0.0.2/video",
            ),
            (local_id, "http://127.0.0.2/video", "http://127.0.0.1/video"),
        ] {
            let http = providers.http(&id).unwrap();
            assert!(http.validate(allowed).is_ok());
            assert!(http.validate(denied).is_err());
        }
    }

    #[tokio::test]
    async fn unknown_provider_is_rejected_before_network_access() {
        let providers = Providers::new("http://127.0.0.1:9/").unwrap();
        let foreign: ProviderId = "other".to_owned().try_into().unwrap();
        let anime = Anime {
            provider: foreign.clone(),
            id: "series-1".into(),
            ..Default::default()
        };
        let episode = Episode {
            provider: foreign.clone(),
            id: "1".into(),
            number: "1".into(),
            title: String::new(),
        };
        for error in [
            providers.search(&foreign, "title").await.err().unwrap(),
            providers.synopsis(&anime).await.err().unwrap(),
            providers.episodes(&anime, Mode::Sub).await.err().unwrap(),
            providers.resolve(&episode, Mode::Sub).await.err().unwrap(),
            providers.http(&foreign).err().unwrap(),
        ] {
            assert!(format!("{error:#}").contains("Provider other is unavailable"));
        }
    }
}
