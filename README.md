[![pipeline](https://github.com/d-e-s-o/cargo-http-registry/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/d-e-s-o/cargo-http-registry/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/cargo-http-registry.svg)](https://crates.io/crates/cargo-http-registry)
[![rustc](https://img.shields.io/badge/rustc-1.60+-blue.svg)](https://blog.rust-lang.org/2022/04/07/Rust-1.60.0.html)

cargo-http-registry
===================

- [Changelog](CHANGELOG.md)

`cargo-http-registry` is a cargo registry allowing for quick publishing
of crates when using `crates.io` is just not desired.

The application can be used to host a local registry to which crates can
be published. Publishing of crates happens over a regular HTTP based API
and can be interfaced with through regular `cargo publish` command.
Crates are stored on the file system and no registry is necessary for
accessing them.

Usage
-----

To set up a local registry just run `cargo-http-registry` and provide
a path to the registry's root directory:
```sh
$ cargo-http-registry /tmp/my-registry
```

The directory will be created if it does not exist and is populated as
needed.

By default, the registry will listen only locally on `127.0.0.1`, but
command line options allow for overwriting this setting.

To make `cargo` aware of this registry, it needs to be made known in a
[`cargo` configuration file][cargo-config]. The registry can be accessed
via the local file system (by specifying the path to it) or over HTTP.
The HTTP address and port can be found in the registry's `config.json`
(e.g., `/tmp/my-registry/config.json` in the example; refer to the `api`
key contents).
Then open your `~/.cargo/config.toml` (or a per-project configuration) and
add the following lines:
```toml
[registries]
my-registry = { index = "http://127.0.0.1:35503/git" }
# Alternatively, access it via path:
my-registry = { index = "file:///tmp/my-registry" }
```

Also note that for HTTP access, you will need to enable the
[`net.git-fetch-with-cli` setting][cargo-net-git-cli]. That can be
accomplished via `config.toml` as well, for example by adding:
```toml
[net]
git-fetch-with-cli = true
```

With that, you can now publish your crates to the registry and pull them
from it.
```sh
$ cargo publish --registry my-registry
    Updating `/tmp/my-registry` index
   Packaging my-lib v0.1.0
   Verifying my-lib v0.1.0
   Compiling my-lib v0.1.0
    Finished dev [unoptimized + debuginfo] target(s) in 0.09s
   Uploading my-lib v0.1.0
```

The created registry does not require any token checks. As such, if
being asked to `cargo login` to the registry, any string may be used.

You can also adjust the crate to only allow publishing to a certain
registry, which will prevent accidental pushes to `crates.io`:
```diff
--- Cargo.toml
+++ Cargo.toml
@@ -1,9 +1,10 @@
 [package]
 name = "my-lib"
 version = "0.1.0"
 authors = ["Daniel Mueller <deso@posteo.net>"]
 edition = "2018"
+publish = ["my-registry"]

 # See more keys and their definitions at https://doc.rust-lang.org/cargo/reference/manifest.html

 [dependencies]
```

To consume the published crate from the local registry, simply set the
`registry` key for the dependency:
```diff
--- Cargo.toml
+++ Cargo.toml
@@ -8,3 +8,4 @@ edition = "2018"

 [dependencies.my-lib]
 version = "0.1"
+registry = "my-registry"
```

Note that `cargo-http-registry` is not meant to be a `cargo` subcommand
and cannot be used as such.

Note furthermore that the registry is meant to be used in a trusted
setting, such as on a single computer or local home network. The reason
being that, by design, it does not have any authentication scheme
present and no attempts of hardening the code have been undertaken.

[cargo-config]: https://doc.rust-lang.org/cargo/reference/config.html
[cargo-net-git-cli]: https://doc.rust-lang.org/cargo/reference/config.html#netgit-fetch-with-cli
[docs-rs]: https://docs.rs/crate/cargo-http-registry
