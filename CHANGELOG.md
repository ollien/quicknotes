# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.1.0] - 2025-02-09

### Changed

- Make the offset in `quicknotes daily` a vararg, like `quicknotes new`'s title.
  You can now do `quicknotes daily 2 days ago`, rather than
  `quicknotes daily "2 days ago"`.


## [1.0.2] - 2025-01-15

### Changed

- Bump MSRV to 1.83 (https://github.com/ollien/quicknotes/pull/2; thanks @paulpr0!)

## [1.0.1] - 2025-01-13

### Fixed

- Updated `nucleo_picker` from alpha version.
- Update patch versions of all other dependencies.

## [1.0.0] - 2025-01-11

Initial project release

[1.1.0]: https://github.com/ollien/quicknotes/compare/v1.0.2...v1.1.0
[1.0.2]: https://github.com/ollien/quicknotes/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/ollien/quicknotes/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/ollien/quicknotes/releases/tag/v1.0.0
