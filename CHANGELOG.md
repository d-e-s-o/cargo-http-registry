Unreleased
----------
- Increased maximum publishable crate size to 20 MiB
- Bumped `sha2` dependency to `0.10`


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
