mod app;
mod artwork;
mod model;
mod player;
mod provider;
mod ui;

use anyhow::{Context, Result, ensure};
use clap::{Parser, ValueEnum};
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
};
use model::{Mode, normalized};
use ratatui_image::picker::{Picker, ProtocolType};
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
    after_help = "Keys: / search, Enter select/play, D sub/dub, Q quality, ? help.\nThe shell ani-cli installation and history are left untouched."
)]
struct Cli {
    /// Title to search for
    query: Vec<String>,
    #[arg(long)]
    dub: bool,
    #[arg(short = 'q', long, default_value = "best")]
    quality: String,
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
    #[arg(
        long,
        env = "ANI_WATCH_PROVIDER",
        default_value = "https://hianime.at/"
    )]
    provider: String,
    #[arg(long, env = "ANI_WATCH_STATE_DIR")]
    state_dir: Option<PathBuf>,
    #[arg(long, env = "ANI_WATCH_CACHE_DIR")]
    cache_dir: Option<PathBuf>,
}

fn directory(env: &str, fallback: &str, name: &str) -> PathBuf {
    std::env::var_os(env)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(fallback)
        })
        .join(name)
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        ratatui::restore();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let provider = provider::Provider::new(&cli.provider)?;
    let query = cli.query.join(" ");
    let mode = if cli.dub { Mode::Dub } else { Mode::Sub };
    let state_dir = cli
        .state_dir
        .unwrap_or_else(|| directory("XDG_STATE_HOME", ".local/state", "ani-watch"));
    if cli.list || cli.resolve || cli.play {
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
        if cli.play {
            tokio::fs::create_dir_all(&state_dir).await?;
        }
        for i in indices {
            let stream = provider.resolve(&episodes[i], mode).await?;
            if cli.resolve {
                println!("{}", serde_json::to_string(&stream)?);
            } else {
                let title = format!("{} / Episode {}", anime.title, episodes[i].number);
                let mut child = player::launch(
                    &cli.player,
                    &cli.player_arg,
                    &stream,
                    &cli.quality,
                    &title,
                    &state_dir.join("player.log"),
                )?;
                let status = child.wait().await?;
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
        .context("Could not create ani-watch state directory")?;
    let mut terminal = ratatui::try_init()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnableBracketedPaste)?;
    let mut picker = if matches!(cli.images, Images::Off | Images::Halfblocks) {
        Picker::halfblocks()
    } else {
        Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())
    };
    match cli.images {
        Images::Kitty => picker.set_protocol_type(ProtocolType::Kitty),
        Images::Sixel => picker.set_protocol_type(ProtocolType::Sixel),
        Images::Iterm2 => picker.set_protocol_type(ProtocolType::Iterm2),
        _ => {}
    }
    let state = app::State::new(
        query,
        mode,
        cli.quality,
        cli.exact,
        cli.ascii,
        !cli.no_animation,
    );
    let options = app::Options {
        player: cli.player,
        player_args: cli.player_arg,
        cache: cli
            .cache_dir
            .unwrap_or_else(|| directory("XDG_CACHE_HOME", ".cache", "ani-watch/covers")),
        state_dir,
        images: !matches!(cli.images, Images::Off),
    };
    app::App::new(state, provider, options)
        .run(&mut terminal, picker)
        .await
}
