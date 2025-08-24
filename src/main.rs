use anyhow::Result;
use clap::{Arg, Command};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod app;
mod config;
mod duration_parser;
mod message_adapter;
mod outputs;
mod state;

use app::LastSignalApp;

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Command::new("lastsignal")
        .version(env!("CARGO_PKG_VERSION"))
        .author("LastSignal")
        .about("Automated safety check-in system")
        .subcommand(
            Command::new("run")
                .about("Start the LastSignal daemon")
        )
        .subcommand(
            Command::new("checkin")
                .about("Record a manual check-in")
        )
        .subcommand(
            Command::new("status")
                .about("Show current status and configuration")
        )
        .subcommand(
            Command::new("test")
                .about("Test all configured outputs")
        )
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("Custom config file path")
        )
        .get_matches();

    // Load config early to get log level
    let config = crate::config::Config::load()?;
    
    // Initialize logging with config log level
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.app.log_level))
        .unwrap();

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();

    // Handle commands
    match matches.subcommand() {
        Some(("run", _)) => {
            tracing::debug!("About to create LastSignalApp...");
            let mut app = LastSignalApp::from_config(config).await?;
            tracing::debug!("LastSignalApp created successfully, starting run...");
            app.run().await?;
        }
        Some(("checkin", _)) => {
            let mut app = LastSignalApp::from_config(config).await?;
            app.checkin().await?;
        }
        Some(("status", _)) => {
            let app = LastSignalApp::from_config(config).await?;
            app.status().await?;
        }
        Some(("test", _)) => {
            let app = LastSignalApp::from_config(config).await?;
            app.test_outputs().await?;
        }
        _ => {
            println!("LastSignal - Automated Safety Check-in System");
            println!("Version: {}", env!("CARGO_PKG_VERSION"));
            println!();
            println!("Available commands:");
            println!("  run      Start the LastSignal daemon");
            println!("  checkin  Record a manual check-in");
            println!("  status   Show current status and configuration");
            println!("  test     Test all configured outputs");
            println!();
            println!("Use 'lastsignal <command> --help' for more information on a command.");
            println!();
            println!("Configuration file should be located at: ~/.lastsignal/config.toml");
        }
    }

    Ok(())
}
