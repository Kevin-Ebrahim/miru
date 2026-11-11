use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste},
    execute,
    terminal::{EndSynchronizedUpdate, EnterAlternateScreen, enable_raw_mode},
};
use std::io::{self, IsTerminal, Write};

pub type Terminal = ratatui::Terminal<ratatui::backend::CrosstermBackend<Output>>;

pub struct Output;

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match io::stdout().write(bytes) {
            // Ratatui also writes from its cursor destructor. A vanished terminal
            // must not turn that best-effort cleanup into a panic on stderr.
            Err(_) if is_disconnected() => Ok(bytes.len()),
            result => result,
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match io::stdout().flush() {
            Err(_) if is_disconnected() => Ok(()),
            result => result,
        }
    }
}

pub fn is_disconnected() -> bool {
    !io::stdout().is_terminal()
}

fn restore() {
    let _ = execute!(
        io::stdout(),
        EndSynchronizedUpdate,
        DisableBracketedPaste,
        DisableMouseCapture
    );
    let _ = ratatui::try_restore();
}

pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

pub fn init() -> io::Result<(Guard, Terminal)> {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        original_hook(info);
    }));
    let guard = Guard;
    // Construct directly so Ratatui does not install its error-printing panic hook.
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(Output))?;
    Ok((guard, terminal))
}
