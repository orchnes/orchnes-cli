# Changelog

All notable changes to `orchnes` are documented here. Versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]
### Fixed
- `orchnes models` output and internal naming no longer look like credential logging (CodeQL `rust/cleartext-logging`, alert #1). Only the environment variable's name and whether it is set were ever printed; the config field stays `api_key_env`.

## [0.2.0] — 2026-11-22
### Added
- Model gateway: `orchnes ask` and `orchnes models`. OpenAI-compatible (OpenAI, OpenRouter, LiteLLM, Ollama, vLLM, …) and Anthropic Messages adapters; tier routing with upward-only fallback; `--sensitive` restricted to local/compliant models; per-call cost; run-log lines; `--dry-run` with masked keys; `--skill` as system prompt.
- `orchnes.example.yaml` gateway config.
### Changed
- Renamed from `skillctl` to `orchnes` (crate and binary), part of Orchnes 360.
- Errors print as one clean line; index exports point to `.agents/skills/`; Copilot export writes the index only.
- CI also checks out `orchnes/orchnes-360` so integration tests run against the real skills.

## [0.1.0] — 2026-11-22
### Added
- `list`, `show`, `validate`, `route`, `export` (9 formats), `install` (8 agent targets).
