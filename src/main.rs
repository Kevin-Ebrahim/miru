mod app;
mod artwork;
mod config;
mod download;
mod filter;
mod input;
mod jobs;
mod model;
mod network;
mod palette;
mod persistence;
mod player;
mod provider;
mod sakura;
mod state;
mod terminal;
mod theme;
mod ui;
mod view;

use anyhow::{Context, Result, ensure};
use clap::{CommandFactory, FromArgMatches, Parser, ValueEnum, parser::ValueSource};
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
    after_help = "Keys: / search, Enter select/play, s sub/dub, Q quality, ? help.\nThe shell ani-cli installation and history are left untouched."
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
    #[arg(short = 'd', long, conflicts_with_all = ["play", "list", "resolve"])]
    download: bool,
    /// Override the download folder for this run without changing the saved default
    #[arg(long)]
    download_dir: Option<PathBuf>,
    #[arg(short = 'q', long)]
    quality: Option<String>,
    #[arg(long)]
    exact: bool,
    /// Executable accepting mpv options (or VLC)
    #[arg(long, default_value = "mpv")]
    player: String,
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
    #[arg(long, conflicts_with = "list")]
    resolve: bool,
    /// Play directly, without opening the TUI
    #[arg(long, conflicts_with_all = ["list", "resolve"])]
    play: bool,
    #[arg(short = 'S', long, default_value_t = 1)]
    select_nth: usize,
    #[arg(short = 'e', long, default_value = "1", allow_hyphen_values = true)]
    episode: String,
    #[arg(long, env = "MIRU_PROVIDER", default_value = "https://hianime.at/")]
    provider: String,
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
        if matches.value_source("provider") == Some(ValueSource::DefaultValue)
            && let Some(value) = std::env::var_os("ANI_WATCH_PROVIDER")
        {
            cli.provider = value
                .into_string()
                .map_err(|_| anyhow::anyhow!("ANI_WATCH_PROVIDER is not valid UTF-8"))?;
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse_with_legacy_env()?;
    let signals = jobs::shutdown_signals()?;
    let provider = provider::Provider::new(&cli.provider)?;
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
        let text = tokio::fs::read_to_string(path)
            .await
            .context("Could not read ani-cli history")?;
        let entries = persistence::import_legacy(&text);
        ensure!(
            !entries.is_empty(),
            "No compatible entries found in {}",
            path.display()
        );
        let library = store.update(persistence::Change::Import(entries))?;
        println!(
            "History now contains {} entries. Source file unchanged.",
            library.entries.len()
        );
        return Ok(());
    }

    let library = store.load()?;
    if cli.history {
        println!("{}", serde_json::to_string_pretty(&library.entries)?);
        return Ok(());
    }

    let mode = if cli.dub {
        Mode::Dub
    } else if cli.sub {
        Mode::Sub
    } else {
        library.mode
    };
    let quality = cli
        .quality
        .clone()
        .unwrap_or_else(|| library.quality.clone());

    let download_dir = config::download_dir(cli.download_dir.as_deref())?;

    if cli.list || cli.resolve || cli.play || cli.download {
        if cli.play || cli.download {
            tokio::fs::create_dir_all(&state_dir).await?;
        }
        let (mut writer, mut library_rx) = persistence::Writer::new(store.clone());
        let mut runner = jobs::Jobs::new(
            provider.clone(),
            jobs::Options {
                player: cli.player.clone(),
                player_args: cli.player_arg.clone(),
                state_dir: state_dir.clone(),
                download_dir: download_dir.clone(),
                history: writer.sender(),
            },
        );

        let [mut interrupt, mut terminate, mut hangup] = signals;
        let result = tokio::select! {
            result = run_cli(&cli, &provider, mode, &quality, &mut runner) => result,
            _ = async { tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {}, _ = hangup.recv() => {} } } => {
                if cli.download { eprintln!("Download cancelled."); }
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
        return result;
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
    state.colorless = colorless;
    state.history.replace_entries(library.entries);
    if cli.resume {
        state.screen = state::Screen::History;
        state.focus = state::Focus::List;
        state.status = "Enter opens episodes with the next episode selected.".into();
    }
    let options = app::Options {
        player: cli.player,
        player_args: cli.player_arg,
        cache: cli
            .cache_dir
            .unwrap_or_else(|| directory("XDG_CACHE_HOME", ".cache", "miru/covers")),
        state_dir,
        images: images_enabled,
        download_dir,
    };
    app::App::new(state, provider, options)
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
    provider: &provider::Provider,
    mode: Mode,
    quality: &str,
    runner: &mut jobs::Jobs,
) -> Result<()> {
    let query = cli.query.join(" ");
    ensure!(!query.trim().is_empty(), "Provide a search query");
    let mut results = provider.search(&query).await?;
    if cli.exact {
        results.retain(|a| normalized(&a.title) == normalized(&query));
    }
    if cli.list {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    let anime = cli
        .select_nth
        .checked_sub(1)
        .and_then(|i| results.get(i))
        .context("Selected result does not exist (--select-nth is 1-based)")?;
    let episodes = provider.episodes(anime).await?;
    let indices = model::episode_range(&cli.episode, &episodes)?;
    for (id, i) in indices.into_iter().enumerate() {
        if cli.resolve {
            println!(
                "{}",
                serde_json::to_string(&provider.resolve(&episodes[i], mode).await?)?
            );
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
                    if let jobs::Output::Download(path) = result? {
                        println!("Saved {}", path.display());
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
