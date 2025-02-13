Unreleased
----------
- Fixed handling of relative registry paths
- Bumped minimum supported Rust version to `1.64`
- Bumped `git2` dependency to `0.19`


0.1.6
-----
- Fixed handling of renamed packages
- Stop requiring Git user configuration to be present by using
  reasonable defaults
- Added `Dockerfile` and adjusted CI to build and publish Docker image
  to GHCR
- Include and manage `Cargo.lock` file in repository
- Bumped minimum supported Rust version to `1.63`


0.1.5
-----
- Fixed Windows build caused by misspelled `cfg` argument
- Reduced log spam by de-duplicating request traces
- Adjusted program to use Rust Edition 2021
- Added GitHub Actions workflow for publishing the crate
- Bumped `git2` dependency to `0.17`


0.1.4
-----
- Switched to using GitHub Actions as CI provider
- Bumped minimum supported Rust version to `1.60`
- Bumped `git2` dependency to `0.15`


0.1.3
-----
- Increased maximum publishable crate size to 20 MiB
- Bumped `git2` dependency to `0.14`
- Bumped `sha2` dependency to `0.10`
- Bumped `tracing-subscriber` dependency to `0.3`


0.1.2
-----
- Added recursive `index` link to registry directory
- Adjusted release build compile options to optimize binary for size
- Enabled CI pipeline comprising building, testing, and linting of the
  project
  - Added badge indicating pipeline status
- Bumped minimum supported Rust version to `1.53`


0.1.1
-----
- Added support for serving registry over HTTP
  - Require `net.git-fetch-with-cli` Cargo configuration
- Removed `http` dependency
- Bumped `tokio` dependency to `1.0`
- Bumped `tracing-subscriber` dependency to `0.2`
- Bumped `warp` dependency to `0.3`


0.1.0
-----
- Initial release
