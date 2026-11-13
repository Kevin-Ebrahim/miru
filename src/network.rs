use anyhow::{Context, Result, bail, ensure};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::{net::IpAddr, sync::Arc, time::Duration};
use url::{Host, Url};

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
pub const MEDIA_PROTOCOLS: &str = "http,https,tcp,tls,crypto,httpproxy,data";
pub const MEDIA_INPUT_OPTIONS: [&str; 4] = [
    "-protocol_whitelist",
    MEDIA_PROTOCOLS,
    "-rw_timeout",
    "20000000",
];

pub fn http_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("Invalid provider URL")?;
    validate_url(&url)?;
    Ok(url)
}

fn validate_url(url: &Url) -> Result<()> {
    ensure!(
        matches!(url.scheme(), "http" | "https") && url.host().is_some(),
        "Only HTTP(S) URLs are supported"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "Credentials in provider URLs are not supported"
    );
    Ok(())
}

#[derive(Clone)]
struct Policy {
    local_provider: Option<String>,
}

impl Policy {
    fn new(base: &Url) -> Self {
        let local = match base.host() {
            Some(Host::Ipv4(ip)) => !public_ip(ip.into()),
            Some(Host::Ipv6(ip)) => !public_ip(ip.into()),
            Some(Host::Domain(host)) => local_name(host),
            None => false,
        };

        Self {
            local_provider: local.then(|| base.host_str().unwrap().to_owned()),
        }
    }

    fn check(&self, url: &Url) -> Result<()> {
        validate_url(url)?;

        if url.host_str() == self.local_provider.as_deref() {
            return Ok(());
        }

        let allowed = match url.host() {
            Some(Host::Ipv4(ip)) => public_ip(ip.into()),
            Some(Host::Ipv6(ip)) => public_ip(ip.into()),
            Some(Host::Domain(host)) => !local_name(host),
            None => false,
        };

        ensure!(
            allowed,
            "Provider URL points to a local or reserved network address"
        );
        Ok(())
    }

    fn redirect(&self, url: &Url, previous: &[Url]) -> Result<()> {
        ensure!(previous.len() < 10, "Too many provider redirects");

        ensure!(
            !previous
                .last()
                .is_some_and(|from| from.scheme() == "https" && url.scheme() != "https"),
            "Provider redirected HTTPS to an insecure URL"
        );
        self.check(url)
    }
}

fn local_name(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local")
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_broadcast()
                && !ip.is_multicast()
                && !ip.is_documentation()
                && a != 0
                && a < 240
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 198 && (18..=19).contains(&b))
                && !(a == 192
                    && ((b == 0 && ip.octets()[2] == 0) || (b == 88 && ip.octets()[2] == 99)))
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return public_ip(ip.into());
            }

            let segments = ip.segments();
            segments[0] & 0xe000 == 0x2000 && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    }
}

impl Resolve for Policy {
    fn resolve(&self, name: Name) -> Resolving {
        let local = self.local_provider.as_deref() == Some(name.as_str());
        let host = name.as_str().to_owned();

        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addresses.is_empty()
                || (!local && addresses.iter().any(|address| !public_ip(address.ip())))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Provider DNS resolved to a local or reserved network address",
                )
                .into());
            }

            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
    policy: Policy,
}

impl Http {
    pub fn new(base: &Url) -> Result<Self> {
        let policy = Policy::new(base);
        let redirects = policy.clone();
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .dns_resolver(Arc::new(policy.clone()))
            .redirect(reqwest::redirect::Policy::custom(
                move |attempt| match redirects.redirect(attempt.url(), attempt.previous()) {
                    Ok(()) => attempt.follow(),
                    Err(error) => attempt.error(error.to_string()),
                },
            ))
            .build()?;

        Ok(Self { client, policy })
    }

    pub fn validate(&self, value: &str) -> Result<Url> {
        let url = Url::parse(value).context("Invalid provider URL")?;
        self.policy.check(&url)?;
        Ok(url)
    }

    pub async fn bytes(&self, url: &str, referer: Option<&str>, limit: usize) -> Result<Vec<u8>> {
        let url = self.validate(url)?;
        let mut request = self.client.get(url.clone());
        if let Some(referer) = referer {
            request = request.header(reqwest::header::REFERER, referer);
        }

        let mut response = request
            .send()
            .await
            .context("Connection failed; check your network and retry")?;

        let status = response.status();
        if status.as_u16() == 403 {
            bail!(
                "{} refused access (HTTP 403). Try again later or check the provider in your browser",
                url.host_str().unwrap_or("Provider")
            );
        }

        ensure!(
            status.is_success(),
            "Request failed: HTTP {status} from {}",
            url.host_str().unwrap_or("provider")
        );

        ensure!(
            response.content_length().unwrap_or(0) <= limit as u64,
            "Response exceeds size limit"
        );

        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= limit,
                "Response exceeds size limit"
            );
            bytes.extend_from_slice(&chunk);
        }

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_sources_cannot_target_private_addresses_or_downgrade_redirects() {
        let policy = Policy::new(&http_url("https://example.org/").unwrap());
        for value in [
            "http://127.0.0.1/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::1]/",
            "http://10.0.0.1/",
            "http://169.254.169.254/",
            "http://localhost/",
            "http://localhost./",
        ] {
            assert!(policy.check(&http_url(value).unwrap()).is_err(), "{value}");
        }
        assert!(
            policy
                .check(&http_url("https://cdn.example.org/a.m3u8").unwrap())
                .is_ok()
        );
        assert!(
            policy
                .redirect(
                    &http_url("http://example.org/").unwrap(),
                    &[http_url("https://example.org/").unwrap()]
                )
                .is_err()
        );
        let local = Policy::new(&http_url("http://127.0.0.1:3000/").unwrap());
        assert!(
            local
                .check(&http_url("http://127.0.0.1:4000/").unwrap())
                .is_ok()
        );
        assert!(local.check(&http_url("http://10.0.0.1/").unwrap()).is_err());
        assert!(http_url("file:///tmp/video").is_err());
        assert!(http_url("https://user:password@example.org/").is_err());
    }

    #[tokio::test]
    async fn transport_checks_redirects_and_streamed_response_limits() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (response, message) in [
            (
                "HTTP/1.1 302 Found\r\nLocation: http://10.0.0.1/\r\nContent-Length: 0\r\n\r\n",
                "local or reserved",
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello",
                "size limit",
            ),
            (
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
                "size limit",
            ),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = http_url(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            let error = Http::new(&url)
                .unwrap()
                .bytes(url.as_str(), None, 4)
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains(message), "{error:#}");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn dns_cannot_hide_a_loopback_destination() {
        let policy = Policy::new(&http_url("https://example.org/").unwrap());
        assert!(policy.resolve("localhost".parse().unwrap()).await.is_err());
    }
}
