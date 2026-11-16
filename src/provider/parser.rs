use super::{parse_episodes, parse_search, parse_server, parse_synopsis};
use crate::model::Mode;
use anyhow::{Context, Result, bail, ensure};
use scraper::Html;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    ffi::OsStr,
    io::{Read, Write},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::Semaphore,
    time::timeout,
};
use url::Url;

const HELPER: &str = "--internal-provider-parser";
pub(super) const BODY_LIMIT: usize = 4 * 1024 * 1024;
const MARKUP_LIMIT: usize = 20_000;
const REQUEST_LIMIT: usize = 32 * 1024 * 1024;
const REPLY_LIMIT: usize = 8 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(2);
static WORKERS: Semaphore = Semaphore::const_new(2);

#[derive(Serialize, Deserialize)]
pub(super) enum Request {
    Search { page: String, base: Url },
    Synopsis { page: String },
    Episodes { page: String, slug: String },
    Server { page: String, mode: Mode },
}

fn check_markup(page: &str) -> Result<()> {
    ensure!(page.len() <= BODY_LIMIT, "Provider HTML exceeds size limit");
    ensure!(
        page.bytes()
            .filter(|&byte| byte == b'<')
            .take(MARKUP_LIMIT + 1)
            .count()
            <= MARKUP_LIMIT,
        "Provider HTML exceeds structural complexity limit"
    );
    Ok(())
}

pub(super) fn document(page: &str) -> Result<Html> {
    check_markup(page)?;
    Ok(Html::parse_document(page))
}

pub(super) fn fragment(page: &str) -> Result<Html> {
    check_markup(page)?;
    Ok(Html::parse_fragment(page))
}

pub(super) async fn parse<T: DeserializeOwned>(request: Request) -> Result<T> {
    let _permit = WORKERS.acquire().await?;
    let input = serde_json::to_vec(&request)?;
    ensure!(
        input.len() <= REQUEST_LIMIT,
        "Provider parser request exceeds size limit"
    );
    let child = Command::new(std::env::current_exe()?)
        .arg(HELPER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Could not start provider HTML parser")?;
    let output = exchange(child, input, DEADLINE).await?;
    serde_json::from_slice::<std::result::Result<T, String>>(&output)
        .context("Provider HTML parser returned an invalid response")?
        .map_err(anyhow::Error::msg)
}

async fn exchange(mut child: Child, input: Vec<u8>, deadline: Duration) -> Result<Vec<u8>> {
    let mut stdin = child.stdin.take().context("Missing parser input pipe")?;
    let mut stdout = child
        .stdout
        .take()
        .context("Missing parser output pipe")?
        .take((REPLY_LIMIT + 1) as u64);
    let work = async {
        let writing = async move {
            stdin.write_all(&input).await?;
            drop(stdin);
            Ok::<_, std::io::Error>(())
        };
        let reading = async {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await?;
            ensure!(
                bytes.len() <= REPLY_LIMIT,
                "Provider parser response exceeds size limit"
            );
            Ok::<_, anyhow::Error>(bytes)
        };
        let (_, output) = tokio::try_join!(
            async { writing.await.map_err(anyhow::Error::from) },
            reading
        )?;
        let status = child.wait().await?;
        ensure!(
            status.success(),
            "Provider HTML parser failed ({status}); the page may exceed parser resource limits"
        );
        Ok(output)
    };
    match timeout(deadline, work).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            bail!("Provider HTML parsing exceeded its time limit")
        }
    }
}

fn resource_limits() -> std::io::Result<()> {
    for (resource, cap) in [
        (libc::RLIMIT_AS, 256 * 1024 * 1024),
        (libc::RLIMIT_CPU, 2),
        (libc::RLIMIT_CORE, 0),
    ] {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // These limits apply only in the fresh parser process, before reading input.
        if unsafe { libc::getrlimit(resource, &mut limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        limit.rlim_cur = limit.rlim_cur.min(cap as libc::rlim_t);
        limit.rlim_max = limit.rlim_max.min(cap as libc::rlim_t);
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

extern "C" fn resource_exit(_: libc::c_int) {
    // A rejected page should produce a normal worker failure, not a desktop crash notification.
    unsafe { libc::_exit(70) }
}

struct Limited<W> {
    writer: W,
    remaining: usize,
}

impl<W: Write> Write for Limited<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other("Parser output limit exceeded"));
        }
        let written = self.writer.write(bytes)?;
        self.remaining -= written;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

pub fn run_helper() -> Result<bool> {
    if std::env::args_os().nth(1).as_deref() != Some(OsStr::new(HELPER)) {
        return Ok(false);
    }
    resource_limits().context("Could not apply provider parser resource limits")?;
    for signal in [libc::SIGABRT, libc::SIGXCPU] {
        // The handler only calls the async-signal-safe _exit in this disposable worker.
        ensure!(
            unsafe { libc::signal(signal, resource_exit as *const () as libc::sighandler_t) }
                != libc::SIG_ERR,
            "Could not install provider parser resource handler"
        );
    }
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take((REQUEST_LIMIT + 1) as u64)
        .read_to_end(&mut input)?;
    ensure!(
        input.len() <= REQUEST_LIMIT,
        "Provider parser request exceeds size limit"
    );
    let request: Request = serde_json::from_slice(&input)?;
    let result = match request {
        Request::Search { page, base } => {
            parse_search(&page, &base).and_then(|value| Ok(serde_json::to_value(value)?))
        }
        Request::Synopsis { page } => {
            parse_synopsis(&page).and_then(|value| Ok(serde_json::to_value(value)?))
        }
        Request::Episodes { page, slug } => {
            parse_episodes(&page, &slug).and_then(|value| Ok(serde_json::to_value(value)?))
        }
        Request::Server { page, mode } => {
            parse_server(&page, mode).and_then(|value| Ok(serde_json::to_value(value)?))
        }
    }
    .map_err(|error| format!("{error:#}"));
    let mut output = Limited {
        writer: std::io::stdout().lock(),
        remaining: REPLY_LIMIT,
    };
    serde_json::to_writer(&mut output, &result)?;
    output.flush()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excessive_markup_is_rejected_at_every_html_boundary() {
        let page = "<div>".repeat(MARKUP_LIMIT + 1);
        let base = Url::parse("https://example.org/").unwrap();
        for error in [
            parse_search(&page, &base).unwrap_err(),
            parse_synopsis(&page).unwrap_err(),
            parse_episodes(&page, "probe-1").unwrap_err(),
            parse_server(&page, Mode::Sub).unwrap_err(),
        ] {
            assert!(error.to_string().contains("complexity limit"));
        }
        assert!(
            check_markup(&"x".repeat(BODY_LIMIT + 1))
                .unwrap_err()
                .to_string()
                .contains("size limit")
        );
    }

    #[test]
    fn synopsis_handles_deep_prose_without_recursive_traversal() {
        let page = format!(
            "<div class=film-description><div class=text>{}A <em>long</em> journey{}<p>Next paragraph</p></div></div>",
            "<div>".repeat(2_000),
            "</div>".repeat(2_000)
        );
        assert_eq!(
            parse_synopsis(&page).unwrap(),
            "A long journey\n\nNext paragraph"
        );
    }

    #[tokio::test]
    async fn a_stuck_worker_is_killed_and_reaped_at_the_deadline() {
        let child = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        assert!(
            exchange(child, vec![], Duration::from_millis(50))
                .await
                .unwrap_err()
                .to_string()
                .contains("time limit")
        );
        // The owned process has been reaped, rather than merely detached from a timed-out future.
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[tokio::test]
    async fn cancelling_a_parse_does_not_leave_its_worker_running() {
        let child = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let task = tokio::spawn(exchange(child, vec![], DEADLINE));
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let error = timeout(Duration::from_secs(2), async {
            loop {
                if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
                    break std::io::Error::last_os_error().raw_os_error();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(error, Some(libc::ESRCH));
    }
}
