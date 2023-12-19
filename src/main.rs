#[macro_use] mod core;

mod cli;
mod config;
mod transport;
mod tunnel;
mod util;

use std::io::{self, Write};
use std::process;

use log::{Level, error};

use crate::cli::{GlobalOptions, Parser};
use crate::config::Config;
use crate::core::EmptyResult;
use crate::tunnel::Tunnel;

fn main() {
    let mut parser = Parser::new();

    let global = parser.parse_global().unwrap_or_else(|e| {
        let _ = writeln!(io::stderr(), "{}.", e);
        process::exit(1);
    });

    if let Err(e) = easy_logging::init(module_path!().split("::").next().unwrap(), global.log_level) {
        let _ = writeln!(io::stderr(), "Failed to initialize the logging: {}.", e);
        process::exit(1);
    }

    if let Err(e) = run(&global) {
        error!("{}.", e);
        process::exit(1);
    }
}

#[tokio::main]
async fn run(options: &GlobalOptions) -> EmptyResult {
    let config_path = &options.config_path;
    let config = Config::load(config_path).map_err(|e| format!(
        "Error while reading {:?} configuration file: {}", config_path, e))?;

    let tunnel = Tunnel::new(&config).await?;
    tunnel.handle().await
}