// Copyright (C) 2020-2021 Daniel Mueller <deso@posteo.net>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A crate providing a cargo registry accessible over HTTP.
//!
//! The official reference for registries can be found [here][]. This
//! crate does not necessarily aim to implement all aspects, as it aims
//! to be used in trusted contexts where authorization is unnecessary.
//!
//! [here]: https://doc.rust-lang.org/cargo/reference/registries.html

mod index;
mod publish;

use std::io::stdout;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context as _;
use anyhow::Error;
use anyhow::Result;

use serde::Deserialize;
use serde::Serialize;
use structopt::StructOpt;
use tokio::runtime::Builder;

use tracing::error;
use tracing::info;
use tracing::subscriber::set_global_default as set_global_subscriber;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::FmtSubscriber;

use warp::http::StatusCode;
use warp::http::Uri;
use warp::Filter as _;
use warp::Reply as _;


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

/// A single error that the registry returns.
#[derive(Debug, Default, Deserialize, Serialize)]
struct RegistryError {
  detail: String,
}

/// A list of errors that the registry returns in its response.
#[derive(Debug, Default, Deserialize, Serialize)]
struct RegistryErrors {
  errors: Vec<RegistryError>,
}

impl From<Error> for RegistryErrors {
  fn from(error: Error) -> Self {
    Self {
      errors: error
        .chain()
        .map(ToString::to_string)
        .map(|err| RegistryError { detail: err })
        .collect(),
    }
  }
}

/// Convert a result back into a response.
async fn response<T>(result: Result<T>) -> Result<impl warp::Reply, warp::Rejection>
where
  T: warp::Reply,
{
  let response = match result {
    Ok(inner) => {
      info!("request status: success");
      inner.into_response()
    },
    Err(err) => {
      error!("request status: error: {:#}", err);

      let errors = RegistryErrors::from(err);
      warp::reply::json(&errors).into_response()
    },
  };
  // Registries always respond with OK and use the JSON error array to
  // indicate problems.
  let reply = warp::reply::with_status(response, StatusCode::OK);
  Ok(reply)
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
    .with_timer(ChronoLocal::rfc3339())
    .finish();

  set_global_subscriber(subscriber).with_context(|| "failed to set tracing subscriber")?;

  // Unfortunately because of how we have to define our routes in order
  // to create our server and we need a server in order to bind it while
  // also needing to bind in order to have the necessary address for the
  // index we have a circular dependency that we can only resolve by use
  // of an `Option`. *sadpanda*
  let shared = Arc::new(Mutex::new(Option::<index::Index>::None));
  let copy = shared.clone();

  // Serve the contents of <root>/.git at /git.
  let index = warp::path("git")
    .and(warp::fs::dir(args.root.join(".git")))
    .with(warp::trace::request());
  // Serve the contents of <root>/ at /crates. This allows for directly
  // downloading the .crate files, to which we redirect from the
  // download handler below.
  let crates = warp::path("crates")
    .and(warp::fs::dir(args.root.clone()))
    .with(warp::trace::request());
  let download = warp::get()
    .and(warp::path("api"))
    .and(warp::path("v1"))
    .and(warp::path("crates"))
    .and(warp::path::param())
    .and(warp::path::param())
    .and(warp::path("download"))
    .map(move |name: String, version: String| {
      let path = format!("/crates/{}", publish::crate_file_name(&name, &version));
      // TODO: Ideally we shouldn't unwrap here. That's not that easily
      //       possible, though, because then we'd need to handle errors
      //       and we can't use the response function because it will
      //       overwrite the HTTP status even on success.
      path.parse::<Uri>().map(warp::redirect).unwrap()
    })
    .with(warp::trace::request());
  let publish = warp::put()
    .and(warp::path("api"))
    .and(warp::path("v1"))
    .and(warp::path("crates"))
    .and(warp::path("new"))
    .and(warp::path::end())
    .and(warp::body::bytes())
    // We cap total body size to 2 MiB to have some upper bound. I
    // believe that's what crates.io does as well.
    .and(warp::body::content_length_limit(2 * 1024 * 1024))
    .map(move |body| {
      let mut index = copy.lock().unwrap();
      let mut index = index.as_mut().unwrap();
      publish::publish_crate(body, &mut index).map(|()| String::new())
    })
    .and_then(response)
    .with(warp::trace::request());

  let rt = Builder::new_current_thread().enable_io().build().unwrap();
  rt.block_on(async move {
    let mut addr = args.addr;
    let original_port = addr.port();
    // If the port is kernel-assigned then see if we can just use the
    // same one we used last time, to prevent needless updates of our
    // configuration file.
    if addr.port() == 0 {
      if let Ok(port) = index::Index::try_read_port(&args.root) {
        addr.set_port(port)
      }
    }

    let (addr, serve) = loop {
      let routes = index
        .clone()
        .or(crates.clone())
        .or(download.clone())
        .or(publish.clone());
      // Despite the claim that this function "Returns [...] a Future that
      // can be executed on any runtime." not even the call itself can
      // happen outside of a tokio runtime. Boy.
      let result = warp::serve(routes)
        .try_bind_ephemeral(addr)
        .with_context(|| format!("failed to bind to {}", addr));

      match result {
        Ok(result) => break result,
        Err(_) if addr.port() != original_port => {
          // We retry with the original port.
          addr.set_port(original_port);
        },
        Err(err) => return Err(err),
      }
    };

    let index = index::Index::new(&args.root, &addr).with_context(|| {
      format!(
        "failed to create/instantiate crate index at {}",
        args.root.display()
      )
    })?;

    *shared.lock().unwrap() = Some(index);

    serve.await;
    Ok(())
  })
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


#[cfg(test)]
mod tests {
  use super::*;

  use serde_json::to_string;


  #[test]
  fn registry_error_encoding() {
    let expected = r#"{"errors":[{"detail":"error message text"}]}"#;
    let errors = RegistryErrors {
      errors: vec![RegistryError {
        detail: "error message text".to_string(),
      }],
    };

    assert_eq!(to_string(&errors).unwrap(), expected);
  }
}
