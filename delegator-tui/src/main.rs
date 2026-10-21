//! delegator-tui: a terminal wallet for Zcash crosslink delegators.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use tokio::sync::mpsc;

use delegator_tui::app::{
    Action, Command, KeyMaterial, Network, Onboarding, OpenSpec, Phase, State, reduce,
};
use delegator_tui::backend::{self, BackendConfig};
use delegator_tui::cli::Cli;
use delegator_tui::{TICK_SECS, clipboard, input, settings, theme, ui};

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
    let network = match &cli.network {
        Some(name) => Network::parse(name).ok_or("unknown network")?,
        None => Network::Stable,
    };
    if let Some(ufvk) = &cli.ufvk {
        delegator_tui::app::onboarding::check_ufvk(ufvk)?;
    }
    let config = BackendConfig {
        data_dir: cli.data_dir.clone(),
        server: cli.server.clone(),
        seed_derivation: match cli.seed_derivation.as_str() {
            "crosslink" => zingolib::wallet::SeedDerivation::CrosslinkTruncated32,
            _ => zingolib::wallet::SeedDerivation::Zip32Standard,
        },
    };
    let wallet_dir = config.wallet_dir(network);
    std::fs::create_dir_all(&wallet_dir)
        .map_err(|e| format!("cannot create {}: {e}", wallet_dir.display()))?;

    init_logging(
        &cli.log_file
            .clone()
            .unwrap_or_else(|| wallet_dir.join("delegator-tui.log")),
    )?;
    tracing::info!("starting delegator-tui on {}", network.name());

    let now = Instant::now();
    let (phase, initial): (Phase, Command) = if let (Some(ufvk), Some(birthday)) =
        (cli.ufvk.clone(), cli.birthday)
    {
        let name = cli.wallet.clone().unwrap_or_else(|| "watch".to_string());
        let path = config.wallet_path(network, &name);
        if path.exists() {
            return Err(format!(
                "a wallet file already exists at {}; pass --wallet with another name, or omit --ufvk to open it",
                path.display()
            ));
        }
        (
            Phase::Loading("Creating wallet".into()),
            Command::Open(OpenSpec::Create {
                network,
                name,
                material: KeyMaterial::Ufvk(ufvk),
                birthday,
                server: cli.server.clone(),
            }),
        )
    } else if let Some(name) = cli.wallet.clone() {
        let path = config.wallet_path(network, &name);
        if !path.exists() {
            return Err(format!(
                "no wallet file at {}; run without --wallet to see the list",
                path.display()
            ));
        }
        (
            Phase::Loading("Opening wallet".into()),
            Command::Open(OpenSpec::Read {
                network,
                name,
                verified: false,
            }),
        )
    } else {
        (
            Phase::Onboarding(Box::new(Onboarding::new(network, cli.server.clone()))),
            Command::ListWallets,
        )
    };

    let mut state = State::new(phase, now);
    state.ascii_qr = cli.ascii_qr;
    state.dither = match cli.dither.as_str() {
        "braille" => theme::Dither::Braille,
        "off" => theme::Dither::Off,
        _ => theme::Dither::Blocks,
    };

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
    let backend = if cli.fake {
        backend::fake::spawn(network, cmd_rx, act_tx.clone())
    } else {
        backend::spawn(config, cmd_rx, act_tx.clone())
    };

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

    let _ = cmd_tx.send(initial);

    let mut tick = tokio::time::interval(Duration::from_secs_f32(TICK_SECS));
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
