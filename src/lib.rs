// Copyright (C) 2021-2023 The cargo-http-registry Developers
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
mod serve;

pub use serve::serve;
