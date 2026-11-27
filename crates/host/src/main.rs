mod api;
mod capture;
mod config;
mod push;
mod store;

use std::{error::Error, fs, path::PathBuf, sync::Arc};

use clap::{Parser, Subcommand};
use tokio::sync::watch;

use crate::{config::Config, store::{Kind, Store}};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the local surveillance host.
    Serve,
    /// Print the location of the host configuration and create it if missing.
    Init,
    /// Print a one-time pairing code. A phone must use it within five minutes.
    Pair,
    /// Revoke the paired phone and outstanding pairing code.
    Revoke,
    /// List available camera and microphone inputs.
    Devices,
    /// Record a test event and queue an alert for the paired phone.
    TestAlert,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let data = match cli.data_dir {
        Some(path) => path,
        None => config::data_dir()?,
    };
    let command = cli.command.unwrap_or(Command::Serve);
    if matches!(command, Command::Devices) {
        return capture::list_devices();
    }
    config::create_data_dir(&data)?;
    let config_path = data.join("config.toml");
    if matches!(command, Command::Init) {
        use std::io::Write;
        match fs::OpenOptions::new().write(true).create_new(true).open(&config_path) {
            Ok(mut file) => file.write_all(include_bytes!("../../../packaging/config.example.toml"))?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        println!("{}", config_path.display());
        return Ok(());
    }
    let store = Arc::new(Store::open(&data.join("events.sqlite3"))?);
    match command {
        Command::Pair => println!("{}", store.issue_code()?),
        Command::Revoke => {
            store.revoke()?;
            println!("Phone and pairing code revoked.");
        }
        Command::TestAlert => {
            let event = store.record(Kind::Test, 0.0, false)?;
            println!("{}", event.expect("test events are recorded while disarmed").id);
        }
        Command::Serve => serve(store, Config::load(&config_path)?, data).await?,
        Command::Devices | Command::Init => unreachable!(),
    }
    Ok(())
}

async fn serve(store: Arc<Store>, config: Config, data: PathBuf) -> Result<()> {
    let logfile = fs::OpenOptions::new().create(true).append(true).open(data.join("host.log"))?;
    tracing_subscriber::fmt().with_ansi(false).with_writer(std::sync::Mutex::new(logfile)).init();
    let push = config.firebase_credentials_file.as_deref().map(push::Fcm::load).transpose()?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let active = watch::channel(store.armed()?);
    let health = Arc::new(capture::Health::default());
    let state = api::State { store: store.clone(), health: health.clone(), armed: Arc::new(active.0), push_configured: push.is_some() };

    let mut workers = tokio::task::JoinSet::new();
    if let Some(input) = capture::Input::video(&config) {
        workers.spawn(capture::monitor(input, store.clone(), health.clone(), active.1.clone()));
    }
    if let Some(input) = capture::Input::audio(&config) {
        workers.spawn(capture::monitor(input, store.clone(), health, active.1));
    }
    workers.spawn(push::work(store, push, config.retention_days));
    tracing::info!(address = %listener.local_addr()?, "Surveillance host started");
    axum::serve(listener, api::router(state)).with_graceful_shutdown(shutdown()).await?;
    workers.shutdown().await;
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
