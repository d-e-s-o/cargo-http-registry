// Copyright (C) 2020-2023 The cargo-http-registry Developers
// SPDX-License-Identifier: GPL-3.0-or-later

use std::io::stdout;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::exit;

use anyhow::Context as _;
use anyhow::Result;

use structopt::StructOpt;
use tokio::runtime::Builder;

use tracing::subscriber::set_global_default as set_global_subscriber;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::time::SystemTime;
use tracing_subscriber::FmtSubscriber;

use cargo_http_registry::serve;


/// A struct defining the accepted arguments.
#[derive(Debug, StructOpt)]
pub struct Args {
  /// The root directory of the registry.
  #[structopt(name = "REGISTRY_ROOT", parse(from_os_str))]
  root: PathBuf,
  /// The address to serve on. By default we serve on 127.0.0.1 on an
  /// ephemeral port.
  #[structopt(short, long, default_value = "127.0.0.1:0")]
  addr: SocketAddr,
  /// Increase verbosity (can be supplied multiple times).
  #[structopt(short = "v", long = "verbose", global = true, parse(from_occurrences))]
  verbosity: usize,
}


fn run() -> Result<()> {
  let args = Args::from_args_safe()?;
  let level = match args.verbosity {
    0 => LevelFilter::WARN,
    1 => LevelFilter::INFO,
    2 => LevelFilter::DEBUG,
    _ => LevelFilter::TRACE,
  };

  let subscriber = FmtSubscriber::builder()
    .with_max_level(level)
    .with_timer(SystemTime)
    .finish();

  set_global_subscriber(subscriber).context("failed to set tracing subscriber")?;

  let rt = Builder::new_current_thread().enable_io().build().unwrap();
  let _guard = rt.enter();

  let (serve, _addr) = serve(&args.root, args.addr)?;
  rt.block_on(serve);
  Ok(())
}

fn main() {
  let exit_code = run()
    .map(|_| 0)
    .map_err(|e| eprintln!("{:?}", e))
    .unwrap_or(1);

  // We exit the process the hard way next, so make sure to flush
  // buffered content.
  let _ = stdout().flush();
  exit(exit_code)
}
