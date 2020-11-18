[![crates.io](https://img.shields.io/crates/v/cargo-http-registry.svg)](https://crates.io/crates/cargo-http-registry)

cargo-http-registry
===================

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
[`cargo` configuration file][cargo-config]. E.g., open your
`~/.cargo/config` (or a per-project configuration) and add the following
lines:
```toml
[registries]
my-registry = { index = "file:///tmp/my-registry" }
```

With that, you can now publish your crates to the registry.
```sh
$ cargo publish --registry my-registry
    Updating `/tmp/my-registry` index
   Packaging my-lib v0.1.0
   Verifying my-lib v0.1.0
   Compiling my-lib v0.1.0
    Finished dev [unoptimized + debuginfo] target(s) in 0.09s
   Uploading my-lib v0.1.0
```

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

[cargo-config]: https://doc.rust-lang.org/cargo/reference/config.html
[docs-rs]: https://docs.rs/crate/cargo-http-registry
