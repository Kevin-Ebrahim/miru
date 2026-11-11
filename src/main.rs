mod app;
mod artwork;
mod download;
mod model;
mod palette;
mod persistence;
mod player;
mod provider;
mod sakura;
mod theme;
mod ui;

use anyhow::{Context, Result, ensure};
use clap::{CommandFactory, FromArgMatches, Parser, ValueEnum, parser::ValueSource};
use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste},
    execute,
};
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
    #[arg(long, default_value = ".")]
    download_dir: PathBuf,
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

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            crossterm::terminal::EndSynchronizedUpdate,
            DisableBracketedPaste,
            DisableMouseCapture
        );
        ratatui::restore();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse_with_legacy_env()?;
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
    if cli.list || cli.resolve || cli.play || cli.download {
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
        if cli.play || cli.download {
            tokio::fs::create_dir_all(&state_dir).await?;
        }
        for i in indices {
            let stream = provider.resolve(&episodes[i], mode).await?;
            if cli.resolve {
                println!("{}", serde_json::to_string(&stream)?);
            } else if cli.download {
                let title = format!(
                    "{} Episode {} {}",
                    anime.title,
                    episodes[i].number,
                    mode.as_str().to_uppercase()
                );
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let progress = tokio::spawn(async move {
                    while let Some(text) = rx.recv().await {
                        eprintln!("{text}");
                    }
                });
                let log = state_dir.join("download.log");
                let download =
                    download::download(&stream, &quality, &title, &cli.download_dir, &log, tx);
                let path = tokio::select! {
                    result = download => result?,
                    _ = tokio::signal::ctrl_c() => { eprintln!("Download cancelled."); return Ok(()); }
                };
                progress.await?;
                println!("Saved {}", path.display());
            } else {
                let title = format!("{} / Episode {}", anime.title, episodes[i].number);
                let mut child = player::launch(
                    &cli.player,
                    &cli.player_arg,
                    &stream,
                    &quality,
                    &title,
                    &state_dir.join("player.log"),
                )?;
                store.update(persistence::Change::Record {
                    anime: anime.clone(),
                    episode: episodes[i].number.clone(),
                    mode,
                })?;
                let status = tokio::select! { status = child.wait() => status?, _ = tokio::signal::ctrl_c() => { child.kill().await?; return Ok(()); } };
                ensure!(
                    status.success(),
                    "Player failed ({status}); see {}/player.log",
                    state_dir.display()
                );
            }
        }
        return Ok(());
    }
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "The TUI needs an interactive terminal. Use --list or --resolve for JSON output"
    );
    tokio::fs::create_dir_all(&state_dir)
        .await
        .context("Could not create Miru state directory")?;
    // Register before entering raw mode, including the capability-query startup window.
    let signals = app::shutdown_signals()?;
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init()?;
    execute!(io::stdout(), EnableBracketedPaste)?;
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            io::stdout(),
            crossterm::terminal::EndSynchronizedUpdate,
            DisableBracketedPaste,
            DisableMouseCapture
        );
        original_hook(info);
    }));
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
    let mut state = app::State::new(
        query,
        mode,
        quality,
        cli.exact,
        cli.ascii,
        !cli.no_animation,
    );
    state.colorless = colorless;
    state.history = library.entries;
    if cli.resume {
        state.screen = app::Screen::History;
        state.focus = app::Focus::List;
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
        download_dir: cli.download_dir,
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
