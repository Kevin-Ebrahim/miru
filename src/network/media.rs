use super::{Http, Policy, http_url};
use crate::model::Stream;
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsStr,
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader, copy_bidirectional},
    net::{TcpListener, TcpStream, UnixDatagram, UnixListener, UnixStream},
    process::{Child, Command},
    sync::oneshot,
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use url::Url;

const HELPER: &str = "--internal-media-proxy";
const SETUP_TIMEOUT: Duration = Duration::from_secs(20);
const HEADER_LIMIT: usize = 32 * 1024;
const CONNECTION_LIMIT: usize = 64;
type Denial = Arc<Mutex<Option<String>>>;

pub enum Tool {
    Player,
    Download,
}

/// A resolved stream and the only network route available to its media processes.
pub struct Session {
    stream: Stream,
    directory: tempfile::TempDir,
    service: JoinHandle<()>,
    denial: Denial,
    ready: UnixDatagram,
}

impl Session {
    pub fn new(stream: Stream, http: Http) -> Result<Self> {
        let directory = tempfile::Builder::new().prefix("miru-media-").tempdir()?;
        let listener = UnixListener::bind(directory.path().join("proxy.sock"))?;
        let ready = UnixDatagram::bind(directory.path().join("proxy.ready"))?;
        let denial = Denial::default();
        let errors = denial.clone();
        let service = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    Some(_) = connections.join_next(), if !connections.is_empty() => {},
                    accepted = listener.accept(), if connections.len() < CONNECTION_LIMIT => {
                        let Ok((client, _)) = accepted else { break };
                        let policy = http.policy.clone();
                        let denial = errors.clone();
                        connections.spawn(serve(client, policy, denial));
                    }
                }
            }
        });
        Ok(Self {
            stream,
            directory,
            service,
            denial,
            ready,
        })
    }

    pub fn stream(&self) -> &Stream {
        &self.stream
    }

    pub fn check(&self) -> Result<()> {
        if let Some(error) = self.denial.lock().unwrap().as_ref() {
            anyhow::bail!("Media destination blocked: {error}");
        }
        Ok(())
    }

    pub fn command(&self, program: &str, tool: Tool) -> Result<Command> {
        ensure!(
            cfg!(target_os = "linux"),
            "Protected media processes require Linux and bubblewrap"
        );
        let mut command = Command::new("bwrap");
        command.args([
            "--unshare-user",
            "--unshare-net",
            "--unshare-pid",
            "--die-with-parent",
            "--bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--dev-bind",
            "/dev",
            "/dev",
            "--",
        ]);
        command
            .arg(std::env::current_exe()?)
            .arg(HELPER)
            .arg(self.directory.path().join("proxy.sock"))
            .arg(match tool {
                Tool::Player => "player",
                Tool::Download => "download",
            })
            .arg(program);
        Ok(command)
    }

    pub async fn wait_started(
        &self,
        child: &mut Child,
        cancel: &mut oneshot::Receiver<()>,
    ) -> Result<bool> {
        let mut confirmation = [0];
        let result = tokio::select! {
            biased;
            _ = cancel => Ok(false),
            result = self.ready.recv(&mut confirmation) => match result {
                Ok(1) if confirmation == [1] => Ok(true),
                Ok(_) => Err(anyhow::anyhow!("Invalid media startup confirmation")),
                Err(error) => Err(error.into()),
            },
            result = child.wait() => match result {
                Ok(status) => Err(anyhow::anyhow!(
                    "Protected media startup failed ({status}); check bubblewrap support and the configured executable")),
                Err(error) => Err(error.into()),
            },
            _ = tokio::time::sleep(SETUP_TIMEOUT) => Err(anyhow::anyhow!("Protected media startup timed out")),
        };
        if !matches!(result, Ok(true)) {
            let _ = child.kill().await;
        }
        result
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Dropping the service's JoinSet also aborts every in-flight connection.
        self.service.abort();
    }
}

fn remember(denial: &Denial, error: impl std::fmt::Display) {
    denial
        .lock()
        .unwrap()
        .get_or_insert_with(|| error.to_string());
}

struct Request {
    target: Url,
    tunnel: bool,
    upstream_header: Vec<u8>,
}

fn request(header: &[u8]) -> Result<Request> {
    let end = header
        .windows(2)
        .position(|s| s == b"\r\n")
        .context("Invalid proxy request")?;
    let first = std::str::from_utf8(&header[..end])?;
    let fields: Vec<_> = first.split_ascii_whitespace().collect();
    ensure!(
        fields.len() == 3 && matches!(fields[2], "HTTP/1.0" | "HTTP/1.1"),
        "Invalid proxy request line"
    );
    let tunnel = fields[0] == "CONNECT";
    let target = if tunnel {
        let target = http_url(&format!("https://{}/", fields[1]))?;
        ensure!(
            target.path() == "/" && target.query().is_none() && target.fragment().is_none(),
            "Invalid CONNECT authority"
        );
        target
    } else {
        ensure!(
            matches!(fields[0], "GET" | "HEAD"),
            "Unsupported media request method"
        );
        let target = http_url(fields[1])?;
        ensure!(target.scheme() == "http", "HTTPS media must use CONNECT");
        target
    };
    let mut upstream_header = Vec::new();
    if !tunnel {
        upstream_header.extend_from_slice(
            format!(
                "{} {} {}\r\n",
                fields[0],
                &target[url::Position::BeforePath..url::Position::AfterQuery],
                fields[2]
            )
            .as_bytes(),
        );
        for line in header[end + 2..].split(|&byte| byte == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() {
                break;
            }
            let name = line.split(|&byte| byte == b':').next().unwrap_or_default();
            if [
                b"Connection".as_slice(),
                b"Proxy-Connection",
                b"Proxy-Authorization",
            ]
            .iter()
            .any(|skip| name.eq_ignore_ascii_case(skip))
            {
                continue;
            }
            upstream_header.extend_from_slice(line);
            upstream_header.extend_from_slice(b"\r\n");
        }
        upstream_header.extend_from_slice(b"Connection: close\r\n\r\n");
    }
    Ok(Request {
        target,
        tunnel,
        upstream_header,
    })
}

async fn open(
    client: &mut BufReader<UnixStream>,
    policy: &Policy,
    denial: &Denial,
) -> Result<Option<TcpStream>> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        ensure!(
            header.len() < HEADER_LIMIT,
            "Media proxy request header exceeds size limit"
        );
        match client.read_u8().await {
            Ok(byte) => header.push(byte),
            Err(error)
                if header.is_empty() && error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        }
    }
    let request = request(&header)?;
    if let Err(error) = policy.check(&request.target) {
        remember(denial, &error);
        return Err(error);
    }
    let addresses = match policy
        .addresses(
            request.target.host_str().context("Missing media host")?,
            request
                .target
                .port_or_known_default()
                .context("Missing media port")?,
        )
        .await
    {
        Ok(addresses) => addresses,
        Err(error) => {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                remember(denial, &error);
            }
            return Err(error.into());
        }
    };
    // Connect to the checked answers themselves, never resolve the name a second time.
    let mut upstream = TcpStream::connect(addresses.as_slice()).await?;
    if request.tunnel {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
    } else {
        upstream.write_all(&request.upstream_header).await?;
    }
    Ok(Some(upstream))
}

async fn serve(client: UnixStream, policy: Policy, denial: Denial) {
    let mut client = BufReader::new(client);
    match timeout(SETUP_TIMEOUT, open(&mut client, &policy, &denial)).await {
        Ok(Ok(Some(mut upstream))) => {
            // A player may close a transfer early when seeking or shutting down.
            let _ = copy_bidirectional(&mut client, &mut upstream).await;
        }
        Ok(Ok(None)) => {}
        _ => {
            let _ = client
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                )
                .await;
        }
    }
}

/// Run only as bubblewrap's internal worker, before ordinary CLI initialization.
pub async fn run_helper() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(OsStr::new(HELPER)) {
        return Ok(());
    }
    let socket = arguments.next().context("Missing media proxy socket")?;
    let tool = arguments.next().context("Missing media tool kind")?;
    let program = arguments.next().context("Missing media executable")?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let proxy = format!("http://{}", listener.local_addr()?);
    let mut command = Command::new(program);
    if tool == "player" {
        command.arg(format!("--http-proxy={proxy}"));
    } else {
        ensure!(tool == "download", "Invalid media tool kind");
    }
    command
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    for name in [
        "http_proxy",
        "https_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
    ] {
        command.env(name, &proxy);
    }
    for name in ["no_proxy", "NO_PROXY"] {
        command.env(name, "");
    }
    let mut child = command
        .spawn()
        .context("Could not launch media executable")?;
    UnixDatagram::unbound()?
        .send_to(&[1], Path::new(&socket).with_extension("ready"))
        .await?;
    let mut connections = JoinSet::new();
    let status = loop {
        tokio::select! {
            result = child.wait() => break result?,
            Some(_) = connections.join_next(), if !connections.is_empty() => {},
            result = listener.accept(), if connections.len() < CONNECTION_LIMIT => {
                let (mut client, _) = result?;
                let socket = socket.clone();
                connections.spawn(async move {
                    if let Ok(Ok(mut server)) = timeout(SETUP_TIMEOUT, UnixStream::connect(Path::new(&socket))).await {
                        let _ = copy_bidirectional(&mut client, &mut server).await;
                    }
                });
            }
        }
    };
    connections.shutdown().await;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_preserves_signed_targets_ranges_and_origin_headers() {
        let parsed = request(b"GET http://cdn.example/video.ts?token=a%2Fb HTTP/1.1\r\nHost: cdn.example\r\nRange: bytes=42-99\r\nReferer: https://embed.example/\r\nProxy-Authorization: secret\r\nConnection: keep-alive\r\n\r\n").unwrap();
        assert!(!parsed.tunnel);
        assert_eq!(parsed.upstream_header, b"GET /video.ts?token=a%2Fb HTTP/1.1\r\nHost: cdn.example\r\nRange: bytes=42-99\r\nReferer: https://embed.example/\r\nConnection: close\r\n\r\n");
    }

    #[test]
    fn connect_requires_a_valid_http_authority_without_credentials_or_paths() {
        let parsed = request(b"CONNECT cdn.example:443 HTTP/1.1\r\n\r\n").unwrap();
        assert!(parsed.tunnel);
        assert_eq!(parsed.target.as_str(), "https://cdn.example/");
        for value in [
            "user:secret@cdn.example:443",
            "cdn.example:443/path",
            "cdn.example:443?query",
            "cdn.example:443#fragment",
        ] {
            assert!(request(format!("CONNECT {value} HTTP/1.1\r\n\r\n").as_bytes()).is_err());
        }
    }

    #[tokio::test]
    async fn dns_answers_are_checked_and_explicit_local_providers_still_connect() {
        let remote = Policy::new(&http_url("https://example.org/").unwrap());
        assert_eq!(
            remote.addresses("localhost", 80).await.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let local = Policy::new(&http_url("http://localhost/").unwrap());
        assert!(!local.addresses("localhost", 80).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn proxy_rejects_private_connect_before_contacting_the_server() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let (mut client, server) = UnixStream::pair().unwrap();
        let denial = Denial::default();
        let task = tokio::spawn(serve(
            server,
            Policy::new(&http_url("https://example.org/").unwrap()),
            denial.clone(),
        ));
        client
            .write_all(
                format!(
                    "CONNECT {} HTTP/1.1\r\n\r\n",
                    listener.local_addr().unwrap()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 502"));
        assert!(
            denial
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains("local or reserved")
        );
        assert!(
            timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn ending_a_media_session_closes_its_active_connections_and_socket() {
        let origin = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!("http://{}/", origin.local_addr().unwrap());
        let session = Session::new(
            Stream {
                variants: vec![],
                referer: String::new(),
                audio: None,
                subtitle: None,
            },
            Http::new(&http_url(&url).unwrap()).unwrap(),
        )
        .unwrap();
        let socket = session.directory.path().join("proxy.sock");
        let mut client = UnixStream::connect(&socket).await.unwrap();
        client
            .write_all(format!("GET {url} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let (mut server, _) = timeout(Duration::from_secs(1), origin.accept())
            .await
            .unwrap()
            .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(server.read_u8().await.unwrap());
        }
        assert!(header.starts_with(b"GET / HTTP/1.1\r\n"));
        drop(session);
        assert!(!socket.exists());
        let mut remaining = Vec::new();
        assert_eq!(
            timeout(Duration::from_secs(1), client.read_to_end(&mut remaining))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert!(
            timeout(Duration::from_secs(1), server.read_u8())
                .await
                .unwrap()
                .is_err()
        );
    }
}
