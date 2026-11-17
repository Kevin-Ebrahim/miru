mod app;
mod artwork;
mod config;
mod download;
mod filter;
mod input;
mod jobs;
mod model;
mod network;
mod output;
mod palette;
mod persistence;
mod player;
mod provider;
mod sakura;
mod settings;
mod state;
mod terminal;
mod theme;
mod ui;
mod view;

use anyhow::{Context, Result, ensure};
use clap::{ArgGroup, CommandFactory, FromArgMatches, Parser, ValueEnum, parser::ValueSource};
use model::{Mode, normalized};
use ratatui_image::picker::{Picker, ProtocolType, cap_parser::QueryStdioOptions};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
};

#[derive(Clone, Copy, ValueEnum)]
enum Images {
    Auto,
    Kitty,
    Sixel,
    Iterm2,
    Halfblocks,
    Off,
}

#[derive(Parser)]
#[command(
    version,
    about = "A terminal-native anime browser",
    group(ArgGroup::new("operation")
        .args(["resume", "history", "import_history", "download", "list", "resolve", "play"])
        .multiple(false)),
    after_help = concat!(
        "Operations are mutually exclusive: --continue, --history, --import-history,\n",
        "--list, --resolve, --play and --download. With none, open the TUI.\n",
        "--continue opens TUI history. Conflicts are rejected before any work starts.\n\n",
        "Modifiers remain accepted with every operation; only applicable options take effect.\n",
        "--exact controls searches; -S and -e select direct resolve/play/download results.\n",
        "--sub/--dub choose resolve/play/download and TUI language; they conflict.\n",
        "--quality sets playback/download quality and the TUI's initial quality.\n",
        "Player options apply to playback; --download-dir applies to downloads.\n",
        "Appearance options apply to the TUI. State/provider/cache options remain available.\n\n",
        "Keys: / search, Enter select/play, F2 provider, s sub/dub, Q quality, ? help.\n",
        "The shell ani-cli installation and history are left untouched."
    )
)]
struct Cli {
    /// Title to search for
    query: Vec<String>,
    #[arg(long)]
    dub: bool,
    #[arg(long, conflicts_with = "dub")]
    sub: bool,
    /// Open watch history and continue a series
    #[arg(short = 'c', long = "continue")]
    resume: bool,
    /// Print saved history as JSON
    #[arg(long)]
    history: bool,
    /// Import an ani-cli ani-hsts file without modifying it
    #[arg(long)]
    import_history: Option<PathBuf>,
    /// Download directly with ffmpeg instead of opening the TUI
    #[arg(short = 'd', long)]
    download: bool,
    /// Override the download folder for this run without changing the saved default
    #[arg(long)]
    download_dir: Option<PathBuf>,
    #[arg(short = 'q', long)]
    quality: Option<String>,
    #[arg(long)]
    exact: bool,
    /// Executable accepting mpv options (or VLC)
    #[arg(long)]
    player: Option<String>,
    /// Override argument style; inferred from the executable basename when omitted
    #[arg(long, value_enum)]
    player_backend: Option<player::Backend>,
    /// Additional player argument; repeat as needed (use = for leading --)
    #[arg(long)]
    player_arg: Vec<String>,
    #[arg(long, value_enum, default_value = "auto")]
    images: Images,
    /// Follow the desktop palette when available, or use terminal colors only
    #[arg(long, value_enum, default_value = "auto", env = "MIRU_THEME")]
    theme: theme::ThemeMode,
    #[arg(long)]
    ascii: bool,
    #[arg(long)]
    no_animation: bool,
    /// Print search results as JSON without opening the TUI
    #[arg(long)]
    list: bool,
    /// Resolve stream metadata as JSON without launching a player
    #[arg(long)]
    resolve: bool,
    /// Play directly, without opening the TUI
    #[arg(long)]
    play: bool,
    #[arg(short = 'S', long, default_value_t = 1)]
    select_nth: usize,
    #[arg(short = 'e', long, default_value = "0", allow_hyphen_values = true)]
    episode: String,
    /// Catalogue to search; saved items retain their own provider
    #[arg(long, value_enum, env = "MIRU_SOURCE", default_value = "hianime")]
    source: provider::Source,
    /// Override the selected source's base URL for this run
    #[arg(long, env = "MIRU_PROVIDER")]
    provider: Option<String>,
    #[arg(long, env = "MIRU_STATE_DIR")]
    state_dir: Option<PathBuf>,
    #[arg(long, env = "MIRU_CACHE_DIR")]
    cache_dir: Option<PathBuf>,
}

impl Cli {
    fn parse_with_legacy_env() -> Result<Self> {
        let matches = Self::command().get_matches();
        let mut cli = Self::from_arg_matches(&matches)?;
        if matches.value_source("theme") == Some(ValueSource::DefaultValue)
            && let Some(value) = std::env::var_os("ANI_WATCH_THEME")
        {
            let value = value
                .to_str()
                .context("ANI_WATCH_THEME is not valid UTF-8")?;
            cli.theme = theme::ThemeMode::from_str(value, false)
                .map_err(|error| anyhow::anyhow!("Invalid ANI_WATCH_THEME: {error}"))?;
        }
        if cli.provider.is_none()
            && let Some(value) = std::env::var_os("ANI_WATCH_PROVIDER")
        {
            cli.provider = Some(
                value
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("ANI_WATCH_PROVIDER is not valid UTF-8"))?,
            );
        }
        cli.state_dir = cli
            .state_dir
            .or_else(|| std::env::var_os("ANI_WATCH_STATE_DIR").map(PathBuf::from));
        cli.cache_dir = cli
            .cache_dir
            .or_else(|| std::env::var_os("ANI_WATCH_CACHE_DIR").map(PathBuf::from));
        Ok(cli)
    }
}

fn directory(env: &str, fallback: &str, name: &str) -> PathBuf {
    std::env::var_os(env)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(fallback)
        })
        .join(name)
}

fn main() -> Result<()> {
    if provider::run_parser_helper()? {
        return Ok(());
    }
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run());
    output::handle_closed_pipe(result)
}

async fn run() -> Result<()> {
    network::run_helper().await?;
    let cli = Cli::parse_with_legacy_env()?;
    if cli.download {
        provider::require_downloads(&cli.source.id())?;
    }
    let provider = provider::Providers::configured(cli.source, cli.provider.as_deref())?;
    let query = cli.query.join(" ");
    let state_dir = cli
        .state_dir
        .clone()
        .unwrap_or_else(|| directory("XDG_STATE_HOME", ".local/state", "miru"));

    let store = persistence::Store::new(&state_dir);
    if cli.state_dir.is_none() {
        let old_dir = directory("XDG_STATE_HOME", ".local/state", "ani-watch");
        store.migrate_from(&persistence::Store::new(&old_dir))?;
    }

    if let Some(path) = &cli.import_history {
        let entries = persistence::import_file(path)?;
        ensure!(
            !entries.is_empty(),
            "No compatible entries found in {}",
            path.display()
        );
        let library = store.update(persistence::Change::Import(entries))?;
        output::line(format_args!(
            "History now contains {} entries. Source file unchanged.",
            library.entries.len()
        ))?;
        return Ok(());
    }

    let library = store.load()?;
    if cli.history {
        output::line(format_args!(
            "{}",
            serde_json::to_string_pretty(&library.entries)?
        ))?;
        return Ok(());
    }

    let config_path = config::path()?;
    let mut saved = match config::Config::load(&config_path) {
        Ok(saved) => saved,
        Err(error) if cli.download_dir.is_some() => {
            eprintln!(
                "Warning: {error:#}; using defaults with --download-dir. Configuration was not changed."
            );
            config::Config::default()
        }
        Err(error) => return Err(error),
    };
    saved.inherit(&library);
    let preferences = settings::Settings::new(
        saved,
        settings::Overrides {
            download: cli.download_dir.clone(),
            audio: if cli.dub {
                Some(Mode::Dub)
            } else if cli.sub {
                Some(Mode::Sub)
            } else {
                None
            },
            quality: cli.quality.clone(),
            player: cli.player.clone(),
            args: (!cli.player_arg.is_empty()).then(|| cli.player_arg.clone()),
            backend: cli.player_backend,
        },
        config_path,
    );
    let download_dir = preferences.download()?;
    // Keep normal signal termination during startup and offline storage operations.
    let signals = jobs::shutdown_signals()?;
    let mode = if cli.dub {
        Mode::Dub
    } else if cli.sub {
        Mode::Sub
    } else {
        preferences.saved.audio.unwrap_or(library.mode)
    };
    let quality = cli.quality.clone().unwrap_or_else(|| {
        preferences
            .saved
            .quality
            .clone()
            .unwrap_or_else(|| library.quality.clone())
    });

    if cli.list || cli.resolve || cli.play || cli.download {
        if cli.play || cli.download {
            tokio::fs::create_dir_all(&state_dir).await?;
        }
        let (mut writer, mut library_rx) = persistence::Writer::new(store.clone());
        let mut runner = jobs::Jobs::new(
            provider.clone(),
            jobs::Options {
                player: preferences.player(),
                state_dir: state_dir.clone(),
                download_dir: download_dir.clone(),
                history: writer.sender(),
            },
        );

        let [mut interrupt, mut terminate, mut hangup] = signals;
        let result = tokio::select! {
            result = run_cli(&cli, &provider, mode, &quality, &mut runner) => result,
            _ = async {
                tokio::select! {
                    _ = interrupt.recv() => {},
                    _ = terminate.recv() => {},
                    _ = hangup.recv() => {},
                }
            } => {
                if cli.download {
                    eprintln!("Download cancelled.");
                }
                Ok(())
            }
        };

        let stopped = runner.shutdown().await;
        let saved = writer.shutdown().await;
        stopped?;
        saved?;
        while let Ok(library) = library_rx.try_recv() {
            library?;
        }
        return result.with_context(|| format!(
            "Search another provider with: miru --source {} --list --{} <title>. Remove any provider URL override when switching sites. Choose the matching series and episode; history is preserved.",
            cli.source.next().name(), mode.as_str()
        ));
    }

    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "The TUI needs an interactive terminal. Use --list or --resolve for JSON output"
    );

    tokio::fs::create_dir_all(&state_dir)
        .await
        .context("Could not create Miru state directory")?;

    let (_guard, mut terminal) = terminal::init()?;
    let colorless = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let images_enabled = !matches!(cli.images, Images::Off) && !colorless;
    let mut picker = if !images_enabled || matches!(cli.images, Images::Halfblocks) {
        Picker::halfblocks()
    } else {
        Picker::from_query_stdio_with_options(QueryStdioOptions {
            terminal_background_color_osc: true,
            ..Default::default()
        })
        .unwrap_or_else(|_| Picker::halfblocks())
    };
    match cli.images {
        Images::Kitty => picker.set_protocol_type(ProtocolType::Kitty),
        Images::Sixel => picker.set_protocol_type(ProtocolType::Sixel),
        Images::Iterm2 => picker.set_protocol_type(ProtocolType::Iterm2),
        _ => {}
    }
    let mut state = state::State::new(
        query,
        mode,
        quality,
        cli.exact,
        cli.ascii,
        !cli.no_animation,
    );
    state.search.source = cli.source;
    state.colorless = colorless;
    state.history.replace_entries(library.entries);
    if cli.resume {
        state.screen = state::Screen::History;
        state.focus = state::Focus::List;
        state.status = "Enter opens episodes with the next episode selected.".into();
    }
    let options = app::Options {
        player: preferences.player(),
        cache: cli
            .cache_dir
            .unwrap_or_else(|| directory("XDG_CACHE_HOME", ".cache", "miru/covers")),
        state_dir,
        images: images_enabled,
        download_dir,
    };
    app::App::new(state, provider, options)
        .with_settings(preferences)
        .run(
            &mut terminal,
            picker,
            signals,
            theme::ThemeSource::new(cli.theme, colorless),
        )
        .await
}

async fn run_cli(
    cli: &Cli,
    provider: &provider::Providers,
    mode: Mode,
    quality: &str,
    runner: &mut jobs::Jobs,
) -> Result<()> {
    let query = cli.query.join(" ");
    ensure!(!query.trim().is_empty(), "Provide a search query");
    let mut results = provider.search(&cli.source.id(), &query).await?;
    if cli.exact {
        results.retain(|a| normalized(&a.title) == normalized(&query));
    }
    if cli.list {
        output::line(format_args!("{}", serde_json::to_string_pretty(&results)?))?;
        return Ok(());
    }
    let anime = cli
        .select_nth
        .checked_sub(1)
        .and_then(|i| results.get(i))
        .context("Selected result does not exist (--select-nth is 1-based)")?;
    let episodes = provider.episodes(anime, mode).await?;
    let indices = model::episode_range(&cli.episode, &episodes).with_context(|| {
        format!(
            "{} episode selection failed ({})",
            anime.provider,
            mode.as_str()
        )
    })?;
    for (id, i) in indices.into_iter().enumerate() {
        if cli.resolve {
            output::line(format_args!(
                "{}",
                serde_json::to_string(provider.resolve(&episodes[i], mode).await?.stream())?
            ))?;
            continue;
        }
        let title = if cli.download {
            format!(
                "{} Episode {} {}",
                anime.title,
                episodes[i].number,
                mode.as_str().to_uppercase()
            )
        } else {
            format!("{} / Episode {}", anime.title, episodes[i].number)
        };
        runner.start(jobs::Request {
            execution: None,
            id: id as u64,
            episode: episodes[i].clone(),
            anime: anime.clone(),
            mode,
            quality: quality.into(),
            title,
            kind: if cli.download {
                jobs::JobKind::Download
            } else {
                jobs::JobKind::Playback
            },
        });
        loop {
            let event = runner
                .recv()
                .await
                .context("Job worker stopped unexpectedly")?;
            if event.id != id as u64 {
                continue;
            }
            match event.update {
                jobs::Update::Progress(text) => eprintln!("{text}"),
                jobs::Update::Finished(result) => {
                    if let jobs::Output::Download(path) = result.with_context(|| {
                        format!(
                            "{} {} failed",
                            anime.provider,
                            if cli.download { "download" } else { "playback" }
                        )
                    })? {
                        output::line(format_args!("Saved {}", path.display()))?;
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::error::ErrorKind;

    const OPERATIONS: &[&[&str]] = &[
        &["--play"],
        &["--history"],
        &["--list"],
        &["--resolve"],
        &["--download"],
        &["--import-history", "source"],
        &["--continue"],
    ];

    fn parse(arguments: &[&str]) -> std::result::Result<Cli, clap::Error> {
        Cli::try_parse_from(
            [
                "miru",
                "--theme",
                "terminal",
                "--provider",
                "http://127.0.0.1:9",
            ]
            .into_iter()
            .chain(arguments.iter().copied()),
        )
    }

    #[test]
    fn every_operation_pair_conflicts_in_both_orders() {
        for (index, left) in OPERATIONS.iter().enumerate() {
            for right in &OPERATIONS[index + 1..] {
                for (first, second) in [(left, right), (right, left)] {
                    let mut arguments = first.to_vec();
                    arguments.extend_from_slice(second);
                    arguments.push("Probe");
                    let error = parse(&arguments).err().unwrap_or_else(|| {
                        panic!("Conflicting operations were accepted: {arguments:?}")
                    });
                    assert_eq!(
                        error.kind(),
                        ErrorKind::ArgumentConflict,
                        "{arguments:?}: {error}"
                    );
                }
            }
        }
    }

    #[test]
    fn no_operation_and_single_operations_keep_existing_modifiers() {
        for operation in std::iter::once(&[][..]).chain(OPERATIONS.iter().copied()) {
            let mut arguments = operation.to_vec();
            arguments.extend_from_slice(&[
                "--sub",
                "--quality",
                "720p",
                "--exact",
                "--player",
                "/path/to/player",
                "--player-backend",
                "mpv",
                "--player-arg=--no-config",
                "--images",
                "off",
                "--ascii",
                "--no-animation",
                "--download-dir",
                "downloads",
                "--cache-dir",
                "cache",
                "--state-dir",
                "state",
                "-S",
                "2",
                "-e",
                "0--1",
                "Probe",
            ]);
            assert!(
                parse(&arguments).is_ok(),
                "Valid modifiers were rejected: {arguments:?}"
            );
        }
    }

    #[test]
    fn aliases_and_language_conflicts_keep_the_same_validation() {
        for arguments in [
            &["-c", "--history"][..],
            &["-d", "--history"],
            &["-cd"],
            &["--play", "-c"],
            &["-c", "--import-history", "source"],
            &["--sub", "--dub"],
        ] {
            let error = parse(arguments)
                .err()
                .unwrap_or_else(|| panic!("Conflicting arguments were accepted: {arguments:?}"));
            assert_eq!(
                error.kind(),
                ErrorKind::ArgumentConflict,
                "{arguments:?}: {error}"
            );
        }
    }
}
