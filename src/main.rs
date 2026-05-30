// Copyright (C) 2020-2026 The cargo-http-registry Developers
// SPDX-License-Identifier: GPL-3.0-or-later

use std::env::var_os;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context as _;
use anyhow::Result;

use clap::ArgAction;
use clap::Parser;

use tokio::runtime::Builder;

use tracing::subscriber::set_global_default as set_global_subscriber;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::FmtSubscriber;

use cargo_http_registry::serve;


/// A struct defining the accepted arguments.
#[derive(Debug, Parser)]
pub struct Args {
  /// The root directory of the registry.
  #[clap(name = "REGISTRY_ROOT")]
  root: PathBuf,
  /// The address to serve on. By default we serve on 127.0.0.1 on an
  /// ephemeral port.
  #[clap(short, long, default_value = "127.0.0.1:0")]
  addr: SocketAddr,
  /// Increase verbosity (can be supplied multiple times).
  #[clap(short = 'v', long = "verbose", global = true, action = ArgAction::Count)]
  verbosity: u8,
}

fn init_logging(verbosity: u8) -> Result<()> {
  enum Filter {
    Level(LevelFilter),
    Env(String),
  }

  let filter = match verbosity {
    0 => {
      // Check if `RUST_LOG` is present and honor it if so.
      if let Some(env) = var_os(EnvFilter::DEFAULT_ENV) {
        let directive = env
          .into_string()
          .ok()
          .with_context(|| format!("env var `{}` is not valid UTF-8", EnvFilter::DEFAULT_ENV))?;

        Filter::Env(directive)
      } else {
        // Use 'warn' as the default level.
        Filter::Level(LevelFilter::WARN)
      }
    },
    1 => Filter::Level(LevelFilter::INFO),
    2 => Filter::Level(LevelFilter::DEBUG),
    _ => Filter::Level(LevelFilter::TRACE),
  };

  let builder =
    FmtSubscriber::builder().with_timer(ChronoLocal::new("%Y-%m-%dT%H:%M:%S%.3f%:z".to_string()));
  match filter {
    Filter::Level(level) => {
      let subscriber = builder.with_max_level(level).finish();
      let () =
        set_global_subscriber(subscriber).with_context(|| "failed to set tracing subscriber")?;
    },
    Filter::Env(directive) => {
      let subscriber = builder
        .with_env_filter(EnvFilter::try_new(directive).context("invalid tracing directive")?)
        .finish();
      let () =
        set_global_subscriber(subscriber).with_context(|| "failed to set tracing subscriber")?;
    },
  }
  Ok(())
}

fn run() -> Result<()> {
  let args = Args::parse();
  let () = init_logging(args.verbosity).context("failed to initialize logging infrastructure")?;

  let rt = Builder::new_current_thread().enable_io().build().unwrap();
  let _guard = rt.enter();

  let (serve, _addr) = serve(&args.root, args.addr)?;
  rt.block_on(serve);
  Ok(())
}

fn main() -> ExitCode {
  run()
    .map(|_| ExitCode::SUCCESS)
    .map_err(|e| eprintln!("{e:?}"))
    .unwrap_or(ExitCode::FAILURE)
}
