mod hianime;
mod parser;

pub use parser::run_helper as run_parser_helper;

use crate::{
    model::{Anime, Episode, Mode, ProviderId},
    network::{Http, ResolvedMedia},
};
use anyhow::{Result, bail};

#[derive(Clone)]
enum Adapter {
    HiAnime(hianime::HiAnime),
}

#[derive(Clone)]
pub struct Providers {
    adapters: Vec<(ProviderId, Adapter)>,
}

impl Providers {
    pub fn new(base: &str) -> Result<Self> {
        Ok(Self {
            adapters: vec![(
                ProviderId::default(),
                Adapter::HiAnime(hianime::HiAnime::new(base)?),
            )],
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
        }
    }

    pub async fn search(&self, id: &ProviderId, query: &str) -> Result<Vec<Anime>> {
        match self.adapter(id)? {
            Adapter::HiAnime(provider) => provider.search(query).await,
        }
    }

    pub async fn synopsis(&self, anime: &Anime) -> Result<String> {
        match self.adapter(&anime.provider)? {
            Adapter::HiAnime(provider) => provider.synopsis(anime).await,
        }
    }

    pub async fn episodes(&self, anime: &Anime, _mode: Mode) -> Result<Vec<Episode>> {
        match self.adapter(&anime.provider)? {
            Adapter::HiAnime(provider) => provider.episodes(anime).await,
        }
    }

    pub async fn resolve(&self, episode: &Episode, mode: Mode) -> Result<ResolvedMedia> {
        match self.adapter(&episode.provider)? {
            Adapter::HiAnime(provider) => {
                let stream = provider.resolve(episode, mode).await?;
                provider.http().resolve_media(stream)
            }
        }
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
            assert!(error.to_string().contains("Provider other is unavailable"));
        }
    }
}
