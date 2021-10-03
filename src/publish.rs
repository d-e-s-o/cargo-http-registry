// Copyright (C) 2020-2021 Daniel Mueller <deso@posteo.net>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;
use std::convert::TryInto as _;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Result as FmtResult;
use std::fs::create_dir_all;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::mem::size_of;
use std::ops::Deref as _;
use std::path::PathBuf;
use std::slice::from_ref as slice_from_ref;
use std::str::from_utf8 as str_from_utf8;

use anyhow::ensure;
use anyhow::Context as _;
use anyhow::Result;

use sha2::Digest as _;
use sha2::Sha256;

use serde::Deserialize;
use serde::Serialize;
use serde_json::from_slice;
use serde_json::to_writer;

use tracing::warn;

use warp::hyper::body::Bytes;

use crate::index::Entry;
use crate::index::Index;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
  Dev,
  Build,
  Normal,
}

impl Display for Kind {
  fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
    let s = match self {
      Kind::Dev => "dev",
      Kind::Build => "build",
      Kind::Normal => "normal",
    };
    f.write_str(s)
  }
}

#[derive(Debug, Deserialize)]
struct Dep {
  /// Name of the dependency. If the dependency is renamed from the
  /// original package name, this is the original name. The new package
  /// name is stored in the `explicit_name_in_toml` field.
  name: String,
  /// The semver requirement for this dependency.
  version_req: String,
  /// Array of features (as strings) enabled for this dependency.
  features: Vec<String>,
  /// Boolean of whether or not this is an optional dependency.
  optional: bool,
  /// Boolean of whether or not default features are enabled.
  default_features: bool,
  /// The target platform for the dependency. Null if not a target
  /// dependency. Otherwise, a string such as "cfg(windows)".
  target: Option<String>,
  /// The dependency kind.
  kind: Kind,
  /// The URL of the index of the registry where this dependency is from
  /// as a string. If not specified or null, it is assumed the
  /// dependency is in the current registry.
  registry: Option<String>,
  /// If the dependency is renamed, this is a string of the new package
  /// name. If not specified or null, this dependency is not renamed.
  explicit_name_in_toml: Option<String>,
}

impl From<Dep> for crate::index::Dep {
  fn from(source: Dep) -> Self {
    Self {
      name: source.name,
      req: source.version_req,
      features: source.features,
      optional: source.optional,
      default_features: source.default_features,
      target: source.target,
      kind: source.kind.to_string(),
      registry: source.registry,
      package: source.explicit_name_in_toml,
    }
  }
}

#[derive(Debug, Deserialize)]
struct MetaData {
  /// The name of the package.
  name: String,
  /// The version of the package being published.
  vers: String,
  /// Array of direct dependencies of the package.
  deps: Vec<Dep>,
  /// Set of features defined for the package. Each feature maps to an
  /// array of features or dependencies it enables. Cargo does not
  /// impose limitations on feature names, but crates.io requires
  /// alphanumeric ASCII, '_' or '-' characters.
  features: BTreeMap<String, Vec<String>>,
  /// List of strings of the authors.
  /// May be empty. crates.io requires at least one entry.
  authors: Vec<String>,
  /// Description field from the manifest. May be null. crates.io
  /// requires at least some content.
  description: Option<String>,
  /// String of the URL to the website for this package's documentation.
  /// May be null.
  documentation: Option<String>,
  /// String of the URL to the website for this package's home page. May
  /// be null.
  homepage: Option<String>,
  /// String of the content of the README file. May be null.
  readme: Option<String>,
  /// String of a relative path to a README file in the crate.
  /// May be null.
  readme_file: Option<String>,
  /// Array of strings of keywords for the package.
  keywords: Vec<String>,
  /// Array of strings of categories for the package.
  categories: Vec<String>,
  /// String of the license for the package. May be null. crates.io
  /// requires either `license` or `license_file` to be set.
  license: Option<String>,
  /// String of a relative path to a license file in the crate. May be
  /// null.
  license_file: Option<String>,
  /// String of the URL to the website for the source repository of this
  /// package. May be null.
  repository: Option<String>,
  /// Optional object of "status" badges. Each value is an object of
  /// arbitrary string to string mappings. crates.io has special
  /// interpretation of the format of the badges.
  badges: BTreeMap<String, BTreeMap<String, String>>,
  /// The `links` string value from the package's manifest, or null if
  /// not specified. This field is optional and defaults to null.
  links: Option<String>,
}

impl From<(MetaData, &[u8])> for Entry {
  fn from(source: (MetaData, &[u8])) -> Self {
    let (metadata, data) = source;

    Self {
      name: metadata.name,
      vers: metadata.vers,
      deps: metadata
        .deps
        .into_iter()
        .map(crate::index::Dep::from)
        .collect(),
      cksum: format!("{:x}", Sha256::digest(data)),
      features: metadata.features,
      yanked: false,
      links: metadata.links,
    }
  }
}

/// Craft the file name for a crate named `name` in version `version`.
pub fn crate_file_name(name: &str, version: &str) -> String {
  format!("{}-{}.crate", name, version)
}

/// Extract and parse a `u32` value from a `Bytes` object.
fn parse_u32(bytes: &mut Bytes) -> Result<u32> {
  ensure!(bytes.len() >= size_of::<u32>(), "not enough data for u32");

  let value = bytes.split_to(size_of::<u32>());
  // TODO: For some reason the value is not in network byte order (big
  //       endian). It's not clear whether it's unconditionally in
  //       little endian or always in host byte order, though.
  let value = u32::from_ne_bytes(value.deref().try_into().unwrap());
  Ok(value)
}

/// Extract and parse the JSON metadata of a publish request from a `Bytes` object.
fn parse_metadata(bytes: &mut Bytes, json_length: usize) -> Result<MetaData> {
  ensure!(bytes.len() >= json_length, "insufficient data in body");
  let json_body = bytes.split_to(json_length);
  let metadata = from_slice::<MetaData>(&json_body).context("failed to parse JSON metadata")?;
  Ok(metadata)
}

/// Infer the path to a crate inside the index from its name.
fn crate_path(name: &str) -> PathBuf {
  // Should have been verified already at this point.
  debug_assert!(name.is_ascii());

  fn to_str(c: &u8) -> &str {
    str_from_utf8(slice_from_ref(c)).unwrap()
  }

  match name.as_bytes() {
    [] => unreachable!(),
    [_] => PathBuf::from("1"),
    [_, _] => PathBuf::from("2"),
    [c, _, _] => ["3", to_str(c)].iter().collect(),
    [c1, c2, c3, c4, ..] => [
      format!("{}{}", to_str(c1), to_str(c2)),
      format!("{}{}", to_str(c3), to_str(c4)),
    ]
    .iter()
    .collect(),
  }
}

/// Read the actual crate data from the request.
fn read_crate(bytes: &mut Bytes, crate_length: usize) -> Result<Bytes> {
  ensure!(bytes.len() >= crate_length, "not enough data for crate");

  let data = bytes.split_to(crate_length);
  Ok(data)
}

/// PUT handler for the `/api/v1/crates/new` endpoint.
// TODO: We may want to rollback earlier changes if we error out
//       somewhere in the middle.
// Note that in here we leak paths in errors. Right now that's by
// design, but if we ever were to change our security model and assume
// bad-faith actors attempting to publish and do other things, that may
// not be so wise.
pub fn publish_crate(mut body: Bytes, index: &mut Index) -> Result<()> {
  let json_length = parse_u32(&mut body)
    .context("failed to read JSON length")?
    .try_into()
    .unwrap();

  let metadata = parse_metadata(&mut body, json_length).context("failed to read JSON body")?;
  let crate_name = metadata.name.clone();
  let crate_vers = metadata.vers.clone();

  // TODO: Strictly speaking we should have more checks in place here.
  ensure!(!crate_name.is_empty(), "crate name cannot be empty");
  ensure!(
    crate_name.is_ascii(),
    "crate name contains non-ASCII characters"
  );

  let crate_meta_dir = index.root().join(crate_path(&crate_name));
  create_dir_all(&crate_meta_dir)
    .with_context(|| format!("failed to create directory {}", crate_meta_dir.display()))?;

  let crate_length = parse_u32(&mut body)
    .context("failed to read crate length")?
    .try_into()
    .unwrap();

  // TODO: We may want to sanitize `metadata.vers` somewhat.
  let data = read_crate(&mut body, crate_length).context("failed to read crate data")?;
  let crate_meta_path = crate_meta_dir.join(&crate_name);

  let mut file = OpenOptions::new()
    .write(true)
    .create(true)
    .append(true)
    .open(&crate_meta_path)
    .with_context(|| {
      format!(
        "failed to create crate index file {}",
        crate_meta_path.display()
      )
    })?;

  let entry = Entry::from((metadata, data.deref()));
  to_writer(&mut file, &entry).context("failed to write crate index meta data")?;
  writeln!(file).context("failed to append new line to crate index meta data file")?;

  let crate_file_name = crate_file_name(&crate_name, &crate_vers);
  let crate_path = index.root().join(&crate_file_name);
  let mut file = OpenOptions::new()
    .write(true)
    .create(true)
    .truncate(true)
    .open(&crate_path)
    .with_context(|| format!("failed to create crate file {}", crate_path.display()))?;

  file
    .write(&data)
    .with_context(|| format!("failed to write to crate file {}", crate_path.display()))?;

  index.add(&crate_meta_path).with_context(|| {
    format!(
      "failed to add {} to git repository",
      crate_meta_path.display()
    )
  })?;
  index
    .add(&crate_path)
    .with_context(|| format!("failed to add {} to git repository", crate_path.display()))?;
  index
    .commit(&format!("Add {} in version {}", crate_name, crate_vers))
    .context("failed to commit changes to index")?;

  if !body.is_empty() {
    warn!("body has {} bytes left", body.len());
  }
  Ok(())
}


#[cfg(test)]
mod tests {
  use super::*;

  use std::path::Path;


  #[test]
  fn parse_short_length() {
    let mut body = Bytes::from([255u8, 255, 255].as_ref());
    let err = parse_u32(&mut body).unwrap_err();
    assert_eq!(err.to_string(), "not enough data for u32");
  }

  #[test]
  fn parse_exact_length() {
    // The data used here is part from an actual (and confirmed valid)
    // body captured.
    let mut body = Bytes::from([44u8, 1, 0, 0].as_ref());
    let length = parse_u32(&mut body).unwrap();
    assert_eq!(length, 300);
    assert!(body.is_empty());
  }

  #[test]
  fn parse_longer_length() {
    let mut body = Bytes::from([142u8, 3, 0, 0, 123].as_ref());
    let length = parse_u32(&mut body).unwrap();
    assert_eq!(length, 910);
    // We should have left over one byte.
    assert_eq!(body.len(), 1);
  }

  #[test]
  fn crate_path_construction() {
    assert_eq!(&crate_path("r"), Path::new("1"));
    assert_eq!(&crate_path("xy"), Path::new("2"));
    assert_eq!(&crate_path("abc"), Path::new("3/a"));
    assert_eq!(&crate_path("abcd"), Path::new("ab/cd"));
    assert_eq!(&crate_path("ydasdayusiy"), Path::new("yd/as"));
  }
}
