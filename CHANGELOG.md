# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.22.1] - 2026-05-28

### Added
- `author` field on all npm packages.
- README badges (npm version, CI, license), a License section, a Node.js version note, and a changelog link.
- Per-package README for each `@xcraftmind/mmcg-*` platform package.

## [0.22.0] - 2026-05-28

### Added
- npm distribution: install via `npx` or `npm` with prebuilt native binaries — no Rust toolchain required.
- Seven prebuilt platform packages (`@xcraftmind/mmcg-*`) covering macOS (arm64, x64), Linux glibc and musl (x64, arm64), and Windows (x64). npm installs only the package matching the host's `os` / `cpu` / `libc`.
- Install-mode-aware `setup claude` that writes the correct MCP `command` form for npx, global, project-local, and cargo installs.

[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v0.22.1...HEAD
[0.22.1]: https://github.com/xcrft/mastermind/compare/npm-v0.22.0...npm-v0.22.1
[0.22.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v0.22.0
