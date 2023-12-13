#[macro_use] mod core;
mod tunnel;
mod util;

use std::io::{self, Write};
use std::process;

use log::{Level, error};

use crate::core::EmptyResult;
use crate::tunnel::Tunnel;

#[tokio::main]
async fn run() -> EmptyResult {
    let tunnel = Tunnel::new("test")?;
    tunnel.handle().await
}

fn main() {
    if let Err(e) = easy_logging::init(module_path!().split("::").next().unwrap(), Level::Trace) {
        let _ = writeln!(io::stderr(), "Failed to initialize the logging: {}.", e);
        process::exit(1);
    }

    if let Err(e) = run() {
        error!("{}.", e);
        process::exit(1);
    }
}