// Copyright (C) 2020 Daniel Mueller <deso@posteo.net>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;
use std::fs::create_dir_all;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;

use git2::Repository;

use serde::Deserialize;
use serde::Serialize;
use serde_json::from_reader;
use serde_json::to_writer_pretty;


#[derive(Debug, Serialize)]
pub struct Dep {
  /// Name of the dependency. If the dependency is renamed from the
  /// original package name, this is the new name. The original package
  /// name is stored in the `package` field.
  pub name: String,
  /// The semver requirement for this dependency.
  /// This must be a valid version requirement defined at
  /// https://github.com/steveklabnik/semver#requirements.
  pub req: String,
  /// Array of features (as strings) enabled for this dependency.
  pub features: Vec<String>,
  /// Boolean of whether or not this is an optional dependency.
  pub optional: bool,
  /// Boolean of whether or not default features are enabled.
  pub default_features: bool,
  /// The target platform for the dependency. null if not a target
  /// dependency. Otherwise, a string such as "cfg(windows)".
  pub target: Option<String>,
  /// The dependency kind.
  /// Note: this is a required field, but a small number of entries
  /// exist in the crates.io index with either a missing or null `kind`
  /// field due to implementation bugs.
  pub kind: String,
  /// The URL of the index of the registry where this dependency is from
  /// as a string. If not specified or null, it is assumed the
  /// dependency is in the current registry.
  pub registry: Option<String>,
  /// If the dependency is renamed, this is a string of the actual
  /// package name. If not specified or null, this dependency is not
  /// renamed.
  pub package: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Entry {
  /// The name of the package.
  /// This must only contain alphanumeric, '-', or '_' characters.
  pub name: String,
  /// The version of the package this row is describing. This must be a
  /// valid version number according to the Semantic Versioning 2.0.0
  /// spec at https://semver.org/.
  pub vers: String,
  /// Array of direct dependencies of the package.
  pub deps: Vec<Dep>,
  /// A SHA-256 checksum of the '.crate' file.
  pub cksum: String,
  /// Set of features defined for the package. Each feature maps to an
  /// array of features or dependencies it enables.
  pub features: BTreeMap<String, Vec<String>>,
  /// Boolean of whether or not this version has been yanked.
  pub yanked: bool,
  /// The `links` string value from the package's manifest, or null if
  /// not specified. This field is optional and defaults to null.
  pub links: Option<String>,
}


/// An object representing a config.json file inside the index.
#[derive(Debug, Default, Deserialize, Serialize)]
struct Config {
  dl: String,
  api: Option<String>,
}

/// A struct representing a crate index.
pub struct Index {
  /// The root directory of the index.
  root: PathBuf,
  /// The git repository inside the index.
  repository: Repository,
}

impl Index {
  pub fn new<P>(root: P, ip: &IpAddr, port: u16) -> Result<Self>
  where
    P: Into<PathBuf>,
  {
    fn inner(root: PathBuf, ip: &IpAddr, port: u16) -> Result<Index> {
      create_dir_all(&root)
        .with_context(|| format!("failed to create directory {}", root.display()))?;

      let repository = Repository::init(&root)
        .with_context(|| format!("failed to initialize git repository {}", root.display()))?;

      let config = root.join("config.json");
      let mut index = Index { root, repository };
      index.ensure_has_commit()?;
      index.ensure_config(&config, ip, port)?;

      Ok(index)
    }

    let root = root.into();
    inner(root, ip, port)
  }

  /// Add a file to the index. Note that this operation only stages the
  /// file. A commit will still be necessary to make it accessible.
  pub fn add(&mut self, file: &Path) -> Result<()> {
    let relative_path = if !file.is_relative() {
      file.strip_prefix(&self.root).with_context(|| {
        format!(
          "failed to make {} relative to {}",
          file.display(),
          self.root.display()
        )
      })?
    } else {
      file
    };

    let mut index = self
      .repository
      .index()
      .context("failed to retrieve git repository index")?;
    index
      .add_path(relative_path)
      .context("failed to add file to git index")?;
    index
      .write()
      .context("failed to write git repository index")?;
    Ok(())
  }

  /// Create a commit.
  pub fn commit(&mut self, message: &str) -> Result<()> {
    let mut index = self
      .repository
      .index()
      .context("failed to retrieve git repository index object")?;
    let tree_id = index
      .write_tree()
      .context("failed to write git repository index tree")?;
    let tree = self
      .repository
      .find_tree(tree_id)
      .context("failed to find tree object in git repository")?;

    let empty = self
      .repository
      .is_empty()
      .context("unable to check git repository empty status")?;

    let signature = self
      .repository
      .signature()
      .context("failed to retrieve git signature object")?;

    if empty {
      self
        .repository
        .commit(Some("HEAD"), &signature, &signature, message, &tree, &[])
    } else {
      let oid = self
        .repository
        .refname_to_id("HEAD")
        .context("failed to map HEAD to git id")?;
      let parent = self
        .repository
        .find_commit(oid)
        .context("failed to find HEAD commit")?;

      self.repository.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &[&parent],
      )
    }
    .context("failed to create git commit")?;

    Ok(())
  }

  /// Ensure that an initial git commit exists.
  fn ensure_has_commit(&mut self) -> Result<()> {
    let empty = self
      .repository
      .is_empty()
      .context("unable to check git repository empty status")?;

    if empty {
      self
        .commit("Create new repository for cargo registry")
        .context("failed to create initial git commit")?;
    }
    Ok(())
  }

  /// Ensure that a valid `config.json` exists and that it is up-to-date.
  fn ensure_config(&mut self, path: &Path, ip: &IpAddr, port: u16) -> Result<()> {
    let result = OpenOptions::new().read(true).write(true).open(path);
    match result {
      Ok(file) => {
        let mut config = from_reader::<_, Config>(&file).context("failed to parse config.json")?;
        let dl = format!("file://{}/{{crate}}-{{version}}.crate", self.root.display());
        let api = format!("http://{}:{}", ip, port);
        if config.dl != dl || config.api.as_ref() != Some(&api) {
          config.dl = dl;
          config.api = Some(api);

          let file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)
            .context("failed to reopen config.json")?;
          to_writer_pretty(&file, &config).context("failed to update config.json")?;

          self
            .add(Path::new("config.json"))
            .context("failed to stage config.json file")?;
          self
            .commit("Update config.json")
            .context("failed to commit config.json")?;
        }
      },
      Err(err) if err.kind() == ErrorKind::NotFound => {
        let file = File::create(path).context("failed to create config.json")?;
        let config = Config {
          dl: format!("file://{}/{{crate}}-{{version}}.crate", self.root.display()),
          api: Some(format!("http://{}:{}", ip, port)),
        };
        to_writer_pretty(&file, &config).context("failed to write config.json")?;

        self
          .add(Path::new("config.json"))
          .context("failed to stage config.json file")?;
        self
          .commit("Add initial config.json")
          .context("failed to commit config.json")?;
      },
      Err(err) => return Err(err).context("failed to open/create config.json"),
    }
    Ok(())
  }

  /// Retrieve the path to the index' root directory.
  #[inline]
  pub fn root(&self) -> &Path {
    &self.root
  }
}


#[cfg(test)]
mod tests {
  use super::*;

  use std::io::Write as _;
  use std::str::FromStr;

  use git2::RepositoryState;

  use tempfile::tempdir;


  #[test]
  fn empty_index_repository() {
    let root = tempdir().unwrap();
    let ip = IpAddr::from_str("192.168.0.1").unwrap();
    let index = Index::new(root.as_ref(), &ip, 9999).unwrap();

    assert_eq!(index.repository.state(), RepositoryState::Clean);
    assert!(index.repository.head().is_ok());

    let file = index.root.join("config.json");
    let config = File::open(file).unwrap();
    let config = from_reader::<_, Config>(&config).unwrap();

    let expected = format!(
      "file://{}/{{crate}}-{{version}}.crate",
      root.as_ref().display()
    );
    assert_eq!(config.dl, expected);
    assert_eq!(config.api, Some("http://192.168.0.1:9999".to_string()));
  }

  #[test]
  fn prepopulated_index_repository() {
    let root = tempdir().unwrap();
    let mut file = File::create(root.as_ref().join("config.json")).unwrap();
    // We always assume some valid JSON in the config.
    file.write(br#"{"dl":"foobar"}"#).unwrap();

    let ip = IpAddr::from_str("254.0.0.0").unwrap();
    let index = Index::new(root.as_ref(), &ip, 1).unwrap();

    assert_eq!(index.repository.state(), RepositoryState::Clean);
    assert!(index.repository.head().is_ok());

    let file = index.root.join("config.json");
    let config = File::open(file).unwrap();
    let config = from_reader::<_, Config>(&config).unwrap();

    let expected = format!(
      "file://{}/{{crate}}-{{version}}.crate",
      root.as_ref().display()
    );
    assert_eq!(config.dl, expected);
    assert_eq!(config.api, Some("http://254.0.0.0:1".to_string()));
  }
}
