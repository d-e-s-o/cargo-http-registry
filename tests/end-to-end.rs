// Copyright (C) 2021-2025 The cargo-http-registry Developers
// SPDX-License-Identifier: GPL-3.0-or-later

#![allow(clippy::ineffective_open_options)]

use std::env;
use std::fs::create_dir;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::bail;
use anyhow::Context as _;
use anyhow::Result;
use pathdiff::diff_paths;
use tempfile::tempdir;

use tokio::spawn;
use tokio::task::JoinHandle;

use cargo_http_registry::serve;

const REGISTRY: &str = "e2e-test-registry";


/// Escape all occurrences of `character` in `string`.
///
/// # Panics
/// The function panics if `character` is anything but a single ASCII
/// character string.
fn escape(character: &str, string: &str) -> String {
  debug_assert_eq!(
    character.len(),
    1,
    "string to escape (`{character}`) is not a single ASCII character"
  );

  // We escape characters by duplicating them.
  string.replace(character, &(character.to_owned() + character))
}


/// A locator for a registry.
enum Locator {
  /// A path on the file system to the root of the registry.
  Path(PathBuf),
  /// A socket address for HTTP based access of the registry.
  Socket(SocketAddr),
}


enum RegistryRootPath {
  Absolute,
  Relative,
}

/// Append data to a file.
fn append<B>(file: &Path, data: B) -> Result<()>
where
  B: AsRef<[u8]>,
{
  let mut file = OpenOptions::new()
    .create(true)
    .write(true)
    .append(true)
    .open(file)
    .context("failed to open file for writing")?;

  file.write(data.as_ref()).context("failed to append data")?;
  Ok(())
}


/// Set up the cargo home directory to use.
fn setup_cargo_home(root: &Path, registry_locator: Locator) -> Result<PathBuf> {
  let home = root.join(".cargo");
  create_dir(&home).context("failed to create cargo home directory")?;
  let config = home.join("config.toml");
  let data = match registry_locator {
    Locator::Path(path) => {
      format!(
        r#"
[registries.{registry}]
index = "file://{path}"
token = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
"#,
        registry = REGISTRY,
        // TODO: This is quite a ghetto way of escaping backslashes on,
        //       say, Windows paths. We could make it nice some day...
        path = escape("\\", &path.display().to_string()),
      )
    },
    Locator::Socket(addr) => {
      format!(
        r#"
[registries.{registry}]
index = "http://{addr}/git"
token = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"

[net]
git-fetch-with-cli = true
"#,
        registry = REGISTRY,
        addr = addr,
      )
    },
  };

  append(&config, data)?;
  Ok(home)
}


/// Run a cargo command.
async fn cargo<'s, I>(home: &Path, args: I) -> Result<()>
where
  I: IntoIterator<Item = &'s str>,
{
  let mut command = Command::new("cargo");
  command.env("CARGO_HOME", home).args(args);

  let handle = tokio::task::spawn_blocking(move || {
    let status = command.status().context("failed to execute cargo")?;
    if !status.success() {
      bail!("cargo failed execution")
    }
    Ok(())
  });
  handle.await.unwrap()
}

/// Run 'cargo init' with the provided arguments and some sensible
/// default ones.
async fn cargo_init<'s, I>(home: &Path, args: I) -> Result<()>
where
  I: IntoIterator<Item = &'s str>,
{
  let args = vec!["init", "--vcs", "none", "--registry", REGISTRY]
    .into_iter()
    .chain(args.into_iter());

  cargo(home, args).await
}

/// Run 'cargo publish' with the provided arguments and some sensible
/// default ones.
async fn cargo_publish<'s, I>(home: &Path, args: I) -> Result<()>
where
  I: IntoIterator<Item = &'s str>,
{
  let args = vec![
    "publish",
    "--locked",
    "--no-verify",
    "--allow-dirty",
    "--registry",
    REGISTRY,
  ]
  .into_iter()
  .chain(args.into_iter());

  cargo(home, args).await
}


/// Serve our registry.
fn serve_registry(root_path: RegistryRootPath) -> (JoinHandle<()>, PathBuf, SocketAddr) {
  let root = tempdir().unwrap();
  let path = match root_path {
    RegistryRootPath::Absolute => root.path().to_owned(),
    RegistryRootPath::Relative => diff_paths(root.path(), env::current_dir().unwrap()).unwrap(),
  };
  let addr = "127.0.0.1:0".parse().unwrap();

  let (serve, addr) = serve(&path, addr).unwrap();
  let serve = move || async {
    serve.await;
    // We need to reference `root` here to make sure that it is
    // moved into the closure so that it outlives us serving our
    // registry in there.
    drop(root);
  };
  let handle = spawn(serve());
  (handle, path, addr)
}


/// Check that we can publish a crate.
#[tokio::test]
async fn publish() {
  async fn test(root_path: RegistryRootPath) {
    let (_handle, _reg_root, addr) = serve_registry(root_path);

    let src_root = tempdir().unwrap();
    let src_root = src_root.path();
    let home = setup_cargo_home(src_root, Locator::Socket(addr)).unwrap();

    let my_lib = src_root.join("my-lib");
    cargo_init(&home, ["--lib", my_lib.to_str().unwrap()])
      .await
      .unwrap();

    cargo_publish(
      &home,
      [
        "--manifest-path",
        my_lib.join("Cargo.toml").to_str().unwrap(),
      ],
    )
    .await
    .unwrap();
  }

  test(RegistryRootPath::Absolute).await;
  test(RegistryRootPath::Relative).await;
}


/// Check that we can publish crates with a renamed dependency.
#[tokio::test]
async fn publish_renamed() {
  let (_handle, _reg_root, addr) = serve_registry(RegistryRootPath::Absolute);

  let src_root = tempdir().unwrap();
  let src_root = src_root.path();
  let home = setup_cargo_home(src_root, Locator::Socket(addr)).unwrap();

  let lib1 = src_root.join("lib1");
  cargo_init(&home, ["--lib", lib1.to_str().unwrap()])
    .await
    .unwrap();
  let lib1_toml = lib1.join("Cargo.toml");
  let lib1_toml = lib1_toml.to_str().unwrap();

  let lib2 = src_root.join("lib2");
  cargo_init(&home, ["--lib", lib2.to_str().unwrap()])
    .await
    .unwrap();
  let data =
    format!(r#"renamed_lib1 = {{package = "lib1", version = "*", registry = "{REGISTRY}"}}"#);
  append(&lib2.join("Cargo.toml"), data).unwrap();
  let lib2_toml = lib2.join("Cargo.toml");
  let lib2_toml = lib2_toml.to_str().unwrap();

  let lib3 = src_root.join("lib3");
  cargo_init(&home, ["--lib", lib3.to_str().unwrap()])
    .await
    .unwrap();
  let data = format!(r#"lib2 = {{version = "0.1.0", registry = "{REGISTRY}"}}"#);
  append(&lib3.join("Cargo.toml"), data).unwrap();
  let lib3_toml = lib3.join("Cargo.toml");
  let lib3_toml = lib3_toml.to_str().unwrap();

  cargo_publish(&home, ["--manifest-path", lib1_toml])
    .await
    .unwrap();

  cargo_publish(&home, ["--manifest-path", lib2_toml])
    .await
    .unwrap();

  cargo_publish(&home, ["--manifest-path", lib3_toml])
    .await
    .unwrap();

  cargo(&home, ["check", "--manifest-path", lib3_toml])
    .await
    .unwrap();
}


async fn test_publish_and_consume(registry_locator: Locator) {
  let src_root = tempdir().unwrap();
  let src_root = src_root.path();
  let home = setup_cargo_home(src_root, registry_locator).unwrap();

  // Create a library crate, my-lib, and have it export a function, foo.
  let my_lib = src_root.join("my-lib");
  cargo_init(&home, ["--lib", my_lib.to_str().unwrap()])
    .await
    .unwrap();
  let data = "pub fn foo() {}\n";
  append(&my_lib.join("src").join("lib.rs"), data).unwrap();

  cargo_publish(
    &home,
    [
      "--manifest-path",
      my_lib.join("Cargo.toml").to_str().unwrap(),
    ],
  )
  .await
  .unwrap();

  // Create a binary create, my-bin, and make it consume my-lib::foo.
  let my_bin = src_root.join("my-bin");
  let cargo_toml = my_bin.join("Cargo.toml");
  cargo_init(&home, ["--bin", my_bin.to_str().unwrap()])
    .await
    .unwrap();
  let data = format!(r#"my-lib = {{version = "*", registry = "{}"}}"#, REGISTRY);
  append(&cargo_toml, data).unwrap();

  let data = "#[allow(unused_imports)] use my_lib::foo;\n";
  append(&my_bin.join("src").join("main.rs"), data).unwrap();

  // Now check the program. If we were unable to pull my-lib from the
  // registry we'd get an error here.
  cargo(
    &home,
    ["check", "--manifest-path", cargo_toml.to_str().unwrap()],
  )
  .await
  .unwrap();
}


/// Check that we can consume a published crate over HTTP.
#[tokio::test]
async fn get_http() {
  let (_handle, _, addr) = serve_registry(RegistryRootPath::Absolute);
  test_publish_and_consume(Locator::Socket(addr)).await
}


/// Check that we can consume a published crate through the file system.
#[tokio::test]
async fn get_filesystem() {
  let (_handle, root, _) = serve_registry(RegistryRootPath::Absolute);
  test_publish_and_consume(Locator::Path(root)).await
}
