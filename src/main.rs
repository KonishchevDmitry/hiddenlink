#[macro_use] mod core;

mod bindings;
mod cli;
mod config;
mod constants;
mod controller;
mod metrics;
mod transport;
mod tunnel;
mod util;

use std::future::Future;
use std::io::{self, Write};
use std::process;
use std::sync::Arc;

use log::error;
use prometheus_client::registry::Registry;

use crate::cli::{GlobalOptions, Parser};
use crate::config::Config;
use crate::controller::Controller;
use crate::core::EmptyResult;
use crate::metrics::ArcCollector;

fn main() {
    let mut parser = Parser::new();

    let global = parser.parse_global().unwrap_or_else(|e| {
        let _ = writeln!(io::stderr(), "{}.", e);
        process::exit(1);
    });

    // FIXME(konishchev): Errors metric
    if let Err(e) = easy_logging::init(module_path!().split("::").next().unwrap(), global.log_level) {
        let _ = writeln!(io::stderr(), "Failed to initialize the logging: {}.", e);
        process::exit(1);
    }

    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_panic_hook(info);
        std::process::abort();
    }));

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

    let controller = Arc::new(Controller::new(&config).await?);
    let mut metrics_server: Box<dyn Future<Output=_> + Unpin> = Box::new(std::future::pending());

    if let Some(metrics_bind_address) = config.metrics_bind_address {
        let mut registry = Registry::with_prefix("hiddenlink");
        registry.register_collector(ArcCollector::new(controller.clone()));
        metrics_server = Box::new(metrics::server::run(metrics_bind_address, registry).await?);
    }

    tokio::try_join!(
        controller.handle(),
        async move {
            metrics_server.as_mut().await.unwrap().map_err(|e| format!(
                "Metrics server has crashed: {e}").into())
        },
    )?;

    Ok(())
}