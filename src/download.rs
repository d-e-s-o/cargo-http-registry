// Copyright (C) 2021 Daniel Mueller <deso@posteo.net>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs::File;
use std::io::Read as _;

use anyhow::Context as _;
use anyhow::Result;

use warp::hyper::body::Bytes;

use crate::index::Index;


/// Download a crate.
pub fn download_crate(name: &str, version: &str, index: &Index) -> Result<Bytes> {
  let file_name = format!("{}-{}.crate", name, version);
  let path = index.root().join(&file_name);
  let mut file =
    File::open(&path).with_context(|| format!("failed to create open file {}", path.display()))?;

  let size = file
    .metadata()
    .with_context(|| format!("failed to inquire size of file {}", path.display()))?
    .len();
  let mut buffer = Vec::with_capacity(size as usize);
  file
    .read_to_end(&mut buffer)
    .with_context(|| format!("failed to read contents of file {}", path.display()))?;

  Ok(Bytes::from(buffer))
}
