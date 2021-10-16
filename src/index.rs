// Copyright (C) 2020-2021 Daniel Mueller <deso@posteo.net>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;
use std::fs::create_dir_all;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr as _;

use anyhow::anyhow;
use anyhow::ensure;
use anyhow::Context as _;
use anyhow::Result;

use git2::Repository;

use serde::Deserialize;
use serde::Serialize;
use serde_json::from_reader;
use serde_json::to_writer_pretty;


/// Parse the port from the given URL.
fn parse_port(url: &str) -> Result<u16> {
  let addr = url
    .split('/')
    .nth(2)
    .ok_or_else(|| anyhow!("provided URL {} has unexpected format", url))?;
  let addr =
    SocketAddr::from_str(addr).with_context(|| format!("failed to parse address {}", addr))?;
  Ok(addr.port())
}


/// Create a symbolic link for a directory.
fn symlink_dir<P, Q>(original: P, link: Q) -> io::Result<()>
where
  P: AsRef<Path>,
  Q: AsRef<Path>,
{
  #[cfg(unix)]
  use std::os::unix::fs::symlink;
  #[cfg(window)]
  use std::os::windows::fs::symlink_dir as symlink;

  symlink(original, link)
}


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
  pub fn new<P>(root: P, addr: &SocketAddr) -> Result<Self>
  where
    P: Into<PathBuf>,
  {
    fn inner(root: PathBuf, addr: &SocketAddr) -> Result<Index> {
      create_dir_all(&root)
        .with_context(|| format!("failed to create directory {}", root.display()))?;

      let repository = Repository::init(&root)
        .with_context(|| format!("failed to initialize git repository {}", root.display()))?;

      let mut index = Index { root, repository };
      index.ensure_has_commit()?;
      index.ensure_config(addr)?;
      index.ensure_index_symlink()?;
      index.update_server_info()?;

      Ok(index)
    }

    let root = root.into();
    inner(root, addr)
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

    self.update_server_info()?;
    Ok(())
  }

  /// Update information necessary for serving the repository in "dumb"
  /// mode.
  fn update_server_info(&self) -> Result<()> {
    // Neither the git2 crate nor libgit2 itself seem to provide similar
    // functionality, so we have to fall back to just running the
    // command.
    let status = Command::new("git")
      .current_dir(&self.root)
      .arg("update-server-info")
      .status()
      .context("failed to run git update-server-info")?;

    ensure!(status.success(), "git update-server-info failed");
    Ok(())
  }

  /// Try to read the port on which the index' API was served last time
  /// from the configuration file.
  pub fn try_read_port(root: &Path) -> Result<u16> {
    let config = root.join("config.json");
    let file = File::open(&config).context("failed to open config.json")?;
    let config = from_reader::<_, Config>(&file).context("failed to parse config.json")?;

    config
      .api
      .ok_or_else(|| anyhow!("no API URL present in config"))
      .and_then(|api| parse_port(&api))
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
  fn ensure_config(&mut self, addr: &SocketAddr) -> Result<()> {
    let path = self.root.join("config.json");
    let result = OpenOptions::new().read(true).write(true).open(&path);
    match result {
      Ok(file) => {
        let mut config = from_reader::<_, Config>(&file).context("failed to parse config.json")?;
        let dl = format!(
          "http://{}/api/v1/crates/{{crate}}/{{version}}/download",
          addr
        );
        let api = format!("http://{}", addr);
        if config.dl != dl || config.api.as_ref() != Some(&api) {
          config.dl = dl;
          config.api = Some(api);

          let file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
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
        let file = File::create(&path).context("failed to create config.json")?;
        let config = Config {
          dl: format!(
            "http://{}/api/v1/crates/{{crate}}/{{version}}/download",
            addr
          ),
          api: Some(format!("http://{}", addr)),
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

  /// Ensure that we have a recursive `index` symlink to the root of the
  /// directory which contains the index.
  fn ensure_index_symlink(&mut self) -> Result<()> {
    // For interoperability with cargo-local-registry, which expects
    // the index data to reside below index/, we create a symbolic
    // link here. This way, users are able to seamlessly switch
    // between the two.
    let result = symlink_dir(".", self.root.join("index"));
    match result {
      Ok(()) => {
        self
          .add(Path::new("index"))
          .context("failed to stage index symlink")?;
        self
          .commit("Add index symlink")
          .context("failed to commit index symlink")?;
      },
      Err(err) if err.kind() == ErrorKind::AlreadyExists => (),
      result => result.with_context(|| {
        format!(
          "failed to create index/ symbolic link below {}",
          self.root.display()
        )
      })?,
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
  fn url_port_parsing() {
    let port = parse_port("http://127.0.0.1:36527").unwrap();
    assert_eq!(port, 36527);

    let port = parse_port("https://192.168.0.254:1").unwrap();
    assert_eq!(port, 1);
  }

  #[test]
  fn empty_index_repository() {
    let root = tempdir().unwrap();
    let addr = SocketAddr::from_str("192.168.0.1:9999").unwrap();
    let index = Index::new(root.as_ref(), &addr).unwrap();

    assert_eq!(index.repository.state(), RepositoryState::Clean);
    assert!(index.repository.head().is_ok());

    let file = index.root.join("config.json");
    let config = File::open(file).unwrap();
    let config = from_reader::<_, Config>(&config).unwrap();

    assert_eq!(
      config.dl,
      "http://192.168.0.1:9999/api/v1/crates/{crate}/{version}/download"
    );
    assert_eq!(config.api, Some("http://192.168.0.1:9999".to_string()));
  }

  #[test]
  fn prepopulated_index_repository() {
    let root = tempdir().unwrap();
    let mut file = File::create(root.as_ref().join("config.json")).unwrap();
    // We always assume some valid JSON in the config.
    file.write_all(br#"{"dl":"foobar"}"#).unwrap();

    let addr = SocketAddr::from_str("254.0.0.0:1").unwrap();
    let index = Index::new(root.as_ref(), &addr).unwrap();

    assert_eq!(index.repository.state(), RepositoryState::Clean);
    assert!(index.repository.head().is_ok());

    let file = index.root.join("config.json");
    let config = File::open(file).unwrap();
    let config = from_reader::<_, Config>(&config).unwrap();

    assert_eq!(
      config.dl,
      "http://254.0.0.0:1/api/v1/crates/{crate}/{version}/download"
    );
    assert_eq!(config.api, Some("http://254.0.0.0:1".to_string()));
  }

  /// Test that we can create an `Index` in the same registry directory
  /// multiple times without problems.
  #[test]
  fn recreate_index() {
    let root = tempdir().unwrap();
    let addr = "127.0.0.1:0".parse().unwrap();

    {
      let _index = Index::new(root.path(), &addr).unwrap();
    }

    {
      let _index = Index::new(root.path(), &addr).unwrap();
    }
  }
}
