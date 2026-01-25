#[macro_use] mod core;

mod bindings;
mod cli;
mod config;
mod constants;
mod controller;
mod crawler;
mod metrics;
mod transport;
mod tunnel;
mod util;

use std::future::Future;
use std::io::{self, Write};
use std::path::Path;
use std::process;
use std::sync::Arc;

use easy_logging::LoggingConfig;
use log::{LevelFilter, error};
use prometheus_client::{metrics::counter::Counter, registry::Registry};

use crate::config::Config;
use crate::controller::Controller;
use crate::core::EmptyResult;
use crate::crawler::Crawler;

fn main() {
    let args = cli::parse_args().unwrap_or_else(|e| {
        let _ = writeln!(io::stderr(), "{}.", e);
        process::exit(1);
    });

    let error_counter = Counter::default();

    let log_error_counter = error_counter.clone();
    let dispatch = LoggingConfig::new(module_path!().split("::").next().unwrap(), args.log_level).dispatch().chain(
        easy_logging::fern::Dispatch::new()
            .level(LevelFilter::Error)
            .chain(easy_logging::fern::Output::call(move |_| {
                log_error_counter.inc();
            }))
    );

    if let Err(e) = dispatch.apply() {
        let _ = writeln!(io::stderr(), "Failed to initialize the logging: {}.", e);
        process::exit(1);
    }

    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_panic_hook(info);
        std::process::abort();
    }));

    if let Err(e) = run(&args.config_path, error_counter) {
        error!("{}.", e);
        process::exit(1);
    }
}

#[tokio::main]
async fn run(config_path: &Path, error_counter: Counter) -> EmptyResult {
    let config = Config::load(config_path).map_err(|e| format!(
        "Error while reading {:?} configuration file: {}", config_path, e))?;

    let controller = Arc::new(Controller::new(&config).await?);
    let crawler = config.crawler.as_ref().map(Crawler::new).transpose()?;
    let mut metrics_server: Box<dyn Future<Output=_> + Unpin> = Box::new(std::future::pending());

    if let Some(metrics_bind_address) = config.metrics_bind_address {
        let mut registry = Registry::with_prefix("hiddenlink");

        registry.register("errors", "Error count", error_counter);
        registry.register_collector(Box::new(controller.clone()));

        metrics_server = Box::new(metrics::server::run(metrics_bind_address, registry).await?);
    }

    tokio::try_join!(
        controller.handle(),
        async move {
            if let Some(mut crawler) = crawler {
                crawler.run().await;
            }
            Ok(())
        },
        async move {
            metrics_server.as_mut().await.unwrap().map_err(|e| format!(
                "Metrics server has crashed: {e}").into())
        },
    )?;

    Ok(())
}