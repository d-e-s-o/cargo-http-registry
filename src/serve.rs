// Copyright (C) 2021-2026 The cargo-http-registry Developers
// SPDX-License-Identifier: GPL-3.0-or-later

use std::future::Future;
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context as _;
use anyhow::Error;
use anyhow::Result;
use anyhow::bail;

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


/// An address to serve on, consisting of a host/IP address and a port.
pub type Addr = (String, u16);


/// Parse an address of the form `<host>:<port>`.
///
/// The host may be an IPv4 address, a quoted IPv6 address such as
/// `[::1]:8080`, or a host name.
pub fn parse_addr(string: &str) -> Result<Addr> {
  let (host, port) = string
    .rsplit_once(':')
    .with_context(|| format!("failed to find port in address `{string}`"))?;

  let quoted = host.starts_with('[') && host.ends_with(']');
  if host.is_empty() || (host.contains(':') && !quoted) {
    bail!("invalid host `{host}` in address `{string}`")
  }

  let port = port
    .parse::<u16>()
    .with_context(|| format!("invalid port `{port}` in address `{string}`"))?;
  Ok((host.to_owned(), port))
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


/// Serve a registry at the given path on the given address.
pub fn serve(root: &Path, addr: Addr) -> Result<(impl Future<Output = ()>, Addr)> {
  // Unfortunately because of how we have to define our routes in order
  // to create our server and we need a server in order to bind it while
  // also needing to bind in order to have the necessary address for the
  // index we have a circular dependency that we can only resolve by use
  // of an `Option`. *sadpanda*
  let shared = Arc::new(Mutex::new(Option::<Index>::None));
  let copy = shared.clone();

  // Serve the contents of <root>/.git at /git.
  let index = warp::path("git").and(warp::fs::dir(root.join(".git")));
  //.with(warp::trace::request());
  // Serve the contents of <root>/ at /crates. This allows for directly
  // downloading the .crate files, to which we redirect from the
  // download handler below.
  let crates = warp::path("crates").and(warp::fs::dir(root.to_owned()));
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
    });
  let publish = warp::put()
    .and(warp::path("api"))
    .and(warp::path("v1"))
    .and(warp::path("crates"))
    .and(warp::path("new"))
    .and(warp::path::end())
    .and(warp::body::bytes())
    // We cap total body size to 20 MiB to have some upper bound. At the
    // time of last check, crates.io employed a limit of 10 MiB.
    .and(warp::body::content_length_limit(20 * 1024 * 1024))
    .map(move |body| {
      let mut index = copy.lock().unwrap();
      let index = index.as_mut().unwrap();
      publish_crate(body, index).map(|()| String::new())
    })
    .and_then(response);

  let (host, mut port) = addr;
  let original_port = port;
  // If the port is kernel-assigned then see if we can just use the
  // same one we used last time, to prevent needless updates of our
  // configuration file.
  if port == 0 {
    if let Ok(reused) = Index::try_read_port(root) {
      port = reused
    }
  }

  let (listener, bound_addr) = loop {
    // Binding wants a plain IPv6 address, but we keep ours quoted,
    // as that is the form our URLs need. Note that binding a host
    // name resolves it first, with every address yielded then tried
    // in turn by the standard library.
    let bind_addr = (host.trim_start_matches('[').trim_end_matches(']'), port);
    let result = TcpListener::bind(bind_addr)
      .and_then(|listener| listener.local_addr().map(|addr| (listener, addr)))
      .with_context(|| format!("failed to bind to {host}:{port}"));

    match result {
      Ok(result) => break result,
      Err(_) if port != original_port => {
        // We retry with the original port.
        port = original_port
      },
      Err(err) => return Err(err),
    }
  };

  let addr = (host, bound_addr.port());

  let routes = index
    .or(crates)
    .or(download)
    .or(publish)
    .with(warp::trace::request())
    // Box the filter to erase its concrete type. Without it, Tokio
    // task submission of our server future may fail to infer its
    // `Send` capability [1].
    //
    // [1] https://github.com/seanmonstar/warp/issues/1139
    .boxed();

  // Warp only accepts connections from a Tokio listener, but
  // converting ours requires a runtime context that we may not have
  // before the returned future is polled. Defer the conversion to
  // here, then. Note that the conversion also requires the listener
  // to be non-blocking.
  let serve = async move {
    let () = listener
      .set_nonblocking(true)
      .expect("failed to set listener to non-blocking mode");
    let listener =
      tokio::net::TcpListener::from_std(listener).expect("failed to create tokio listener");
    warp::serve(routes).incoming(listener).run().await
  };

  let index = Index::new(root, &addr).with_context(|| {
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

  use std::fs::File;

  use serde_json::Value;
  use serde_json::from_reader;
  use serde_json::to_string;

  use tempfile::tempdir;


  /// Check that we accept the addresses we want to support and
  /// reject the ones we do not.
  #[test]
  fn address_parsing() {
    assert_eq!(
      parse_addr("127.0.0.1:8080").unwrap(),
      ("127.0.0.1".to_string(), 8080)
    );
    assert_eq!(parse_addr("[::1]:80").unwrap(), ("[::1]".to_string(), 80));
    assert_eq!(
      parse_addr("example.com:8081").unwrap(),
      ("example.com".to_string(), 8081)
    );

    for invalid in [
      "",
      "localhost",
      ":80",
      "::1:80",
      "example.com:not-a-port",
      "http://example.com:80",
    ] {
      let result = parse_addr(invalid);
      assert!(result.is_err(), "{result:?}");
    }
  }

  /// Make sure the JSON encoding of [`RegistryErrors`] is as expected.
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

  /// Test that serving on an address with a host name preserves that
  /// name where desired.
  #[test]
  fn host_name_serving() {
    let root = tempdir().unwrap();
    let (_serve, addr) = serve(root.path(), parse_addr("localhost:0").unwrap()).unwrap();

    assert_eq!(addr.0, "localhost");
    assert_ne!(addr.1, 0);

    let file = File::open(root.path().join("config.json")).unwrap();
    let config = from_reader::<_, Value>(&file).unwrap();
    assert_eq!(config["api"], format!("http://localhost:{}", addr.1));
  }
}
