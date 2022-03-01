// Copyright (C) 2021-2022 Daniel Mueller <deso@posteo.net>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context as _;
use anyhow::Error;
use anyhow::Result;

use serde::Deserialize;
use serde::Serialize;

use tracing::error;
use tracing::info;

use warp::http::StatusCode;
use warp::http::Uri;
use warp::Filter as _;
use warp::Reply as _;

use crate::index::Index;
use crate::publish::crate_file_name;
use crate::publish::publish_crate;


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


/// Serve a registry at the given path on the given socket address.
pub fn serve(root: &Path, addr: SocketAddr) -> Result<(impl Future<Output = ()>, SocketAddr)> {
  // Unfortunately because of how we have to define our routes in order
  // to create our server and we need a server in order to bind it while
  // also needing to bind in order to have the necessary address for the
  // index we have a circular dependency that we can only resolve by use
  // of an `Option`. *sadpanda*
  let shared = Arc::new(Mutex::new(Option::<Index>::None));
  let copy = shared.clone();

  // Serve the contents of <root>/.git at /git.
  let index = warp::path("git")
    .and(warp::fs::dir(root.join(".git")))
    .with(warp::trace::request());
  // Serve the contents of <root>/ at /crates. This allows for directly
  // downloading the .crate files, to which we redirect from the
  // download handler below.
  let crates = warp::path("crates")
    .and(warp::fs::dir(root.to_owned()))
    .with(warp::trace::request());
  let download = warp::get()
    .and(warp::path("api"))
    .and(warp::path("v1"))
    .and(warp::path("crates"))
    .and(warp::path::param())
    .and(warp::path::param())
    .and(warp::path("download"))
    .map(move |name: String, version: String| {
      let path = format!("/crates/{}", crate_file_name(&name, &version));
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
    // We cap total body size to 20 MiB to have some upper bound. I
    // believe that's what crates.io does as well.
    .and(warp::body::content_length_limit(20 * 1024 * 1024))
    .map(move |body| {
      let mut index = copy.lock().unwrap();
      let index = index.as_mut().unwrap();
      publish_crate(body, index).map(|()| String::new())
    })
    .and_then(response)
    .with(warp::trace::request());

  let mut addr = addr;
  let original_port = addr.port();
  // If the port is kernel-assigned then see if we can just use the
  // same one we used last time, to prevent needless updates of our
  // configuration file.
  if addr.port() == 0 {
    if let Ok(port) = Index::try_read_port(root) {
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

  let index = Index::new(&root, &addr).with_context(|| {
    format!(
      "failed to create/instantiate crate index at {}",
      root.display()
    )
  })?;

  *shared.lock().unwrap() = Some(index);

  Ok((serve, addr))
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
