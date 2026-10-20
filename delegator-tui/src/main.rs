//! delegator-tui: a watch-only terminal wallet for Zcash crosslink delegators.

mod app;
mod backend;
mod cli;
mod clipboard;
mod format;
mod input;
mod qr;
mod settings;
mod theme;
mod ui;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use tokio::sync::mpsc;

use app::{Action, Command, Network, OpenSpec, Phase, State, Wizard, reduce};
use backend::BackendConfig;
use cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    // the TLS stack ships two crypto backends; the process has to pick one before the first
    // connection, or rustls panics on the sync task
    if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("error: cannot set up TLS: {e:?}");
        return ExitCode::FAILURE;
    }
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn init_logging(path: &std::path::Path) -> Result<(), String> {
    use tracing_subscriber::EnvFilter;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("cannot open log file {}: {e}", path.display()))?;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init()
        .map_err(|e| format!("cannot start logging: {e}"))
}

async fn run(cli: Cli) -> Result<(), String> {
    let flag_network = match &cli.chain {
        Some(chain) => Some(Network::parse(chain).ok_or("unknown chain")?),
        None => None,
    };
    // a key names its own network; the flag only has to agree with it
    let network = match (&cli.ufvk, flag_network) {
        (Some(ufvk), flag) => {
            let from_key = Network::from_ufvk(ufvk).ok_or(
                "not a unified full viewing key: expected a prefix of uview1, uviewtest1, or uviewregtest1",
            )?;
            if let Some(flag) = flag
                && flag != from_key
            {
                return Err(format!(
                    "--chain {} does not match the key, which is for {}",
                    flag.name(),
                    from_key.name()
                ));
            }
            from_key
        }
        (None, flag) => flag.unwrap_or(Network::Mainnet),
    };
    let config = BackendConfig {
        network,
        data_dir: cli.data_dir.clone(),
        wallet_name: cli.wallet_name.clone(),
        server: cli.server.clone(),
    };
    let wallet_dir = config.wallet_dir(network);
    std::fs::create_dir_all(&wallet_dir)
        .map_err(|e| format!("cannot create {}: {e}", wallet_dir.display()))?;
    let wallet_path = config.wallet_path(network);
    let exists = wallet_path.exists();

    if cli.ufvk.is_some() && exists {
        return Err(format!(
            "a wallet file already exists at {}; remove it or omit --ufvk to open it",
            wallet_path.display()
        ));
    }

    init_logging(
        &cli.log_file
            .clone()
            .unwrap_or_else(|| wallet_dir.join("delegator-tui.log")),
    )?;
    tracing::info!("starting delegator-tui on {}", network.name());

    let now = Instant::now();
    let (phase, initial): (Phase, Option<Command>) = if exists {
        (
            Phase::Loading("Opening wallet".into()),
            Some(Command::Open(OpenSpec::Read)),
        )
    } else if let (Some(ufvk), Some(birthday)) = (cli.ufvk.clone(), cli.birthday) {
        (
            Phase::Loading("Creating wallet".into()),
            Some(Command::Open(OpenSpec::Create {
                network,
                server: cli
                    .server
                    .clone()
                    .unwrap_or_else(|| network.default_server().to_string()),
                ufvk,
                birthday,
            })),
        )
    } else {
        (Phase::Wizard(Wizard::new(cli.server.clone())), None)
    };

    let mut state = State::new(phase, now);
    state.ascii_qr = cli.ascii_qr;

    // flag, then saved preference, then NO_COLOR, then the default
    let config_path = settings::default_path();
    let saved = config_path
        .as_deref()
        .map(settings::Settings::read)
        .unwrap_or_default();
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    state.theme = theme::choose(cli.theme.as_deref(), saved.theme.as_deref(), no_color);
    // NO_COLOR is already applied above by starting in monochrome. crossterm would otherwise
    // also strip every colour on its own, silently overriding an explicit or saved theme.
    crossterm::style::force_color_output(true);
    state.color_depth =
        theme::ColorDepth::from_flag(&cli.color_depth).unwrap_or_else(theme::ColorDepth::detect);
    tracing::info!(
        "theme {} at {:?}",
        theme::THEMES[state.theme].id,
        state.color_depth
    );

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (act_tx, mut act_rx) = mpsc::unbounded_channel();
    let backend = backend::spawn(config, cmd_rx, act_tx.clone());

    // terminal
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
        ratatui::restore();
        default_hook(info);
    }));
    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste);
    if let Ok(size) = terminal.size() {
        state.size = (size.width, size.height);
    }

    // input thread
    let input_tx = act_tx.clone();
    std::thread::spawn(move || {
        loop {
            match crossterm::event::poll(Duration::from_millis(250)) {
                Ok(true) => match crossterm::event::read() {
                    Ok(event) => {
                        if let Some(action) = input::map_event(event)
                            && input_tx.send(action).is_err()
                        {
                            return;
                        }
                    }
                    Err(_) => return,
                },
                Ok(false) => {
                    if input_tx.is_closed() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });

    let dispatch = |state: &mut State, action: Action, cmd_tx: &mpsc::UnboundedSender<Command>| {
        for command in reduce(state, action) {
            match command {
                Command::Copy(text) => {
                    let ok = clipboard::copy(&text);
                    reduce(state, Action::Copied(ok));
                }
                Command::SaveTheme(id) => {
                    let saved = match &config_path {
                        Some(path) => settings::save_theme(path, id).map_err(|e| e.to_string()),
                        None => Err("this system has no configuration directory".to_string()),
                    };
                    if let Err(e) = saved {
                        reduce(state, Action::Notice(format!("theme not saved: {e}")));
                    }
                }
                Command::ForceQuit => state.exit = true,
                other => {
                    let _ = cmd_tx.send(other);
                }
            }
        }
    };

    if let Some(command) = initial {
        let _ = cmd_tx.send(command);
    }

    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let result = loop {
        if let Err(e) = terminal.draw(|frame| ui::view(frame, &state)) {
            break Err(format!("draw failed: {e}"));
        }
        tokio::select! {
            action = act_rx.recv() => match action {
                Some(action) => dispatch(&mut state, action, &cmd_tx),
                None => break Ok(()),
            },
            _ = tick.tick() => dispatch(&mut state, Action::Tick(Instant::now()), &cmd_tx),
        }
        // fold whatever else is already queued before drawing again
        while let Ok(action) = act_rx.try_recv() {
            dispatch(&mut state, action, &cmd_tx);
        }
        if state.exit {
            break Ok(());
        }
    };

    let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    drop(cmd_tx);
    // a graceful quit has already saved; give a forced one a moment to finish, then go
    let _ = tokio::time::timeout(Duration::from_secs(5), backend).await;
    result
}
