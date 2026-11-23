# orchnes

npm distribution of **orchnes**, the [Orchnes 360](https://github.com/orchnesai/orchnes-360) command-line tool: install Agent Skills into any AI agent, validate them, export them to other agents' formats, route tasks to model tiers and call hosted or local models through one gateway.

```sh
npx orchnes --help
# or
npm install -g orchnes
```

The package contains a small launcher with no dependencies and no install script. On first run it downloads the prebuilt binary for your platform from the matching [GitHub Release](https://github.com/orchnesai/orchnes-cli/releases), checks its SHA-256 against the checksums shipped in this package, and caches it (`ORCHNES_CACHE_DIR` overrides the location).

Prebuilt binaries: Linux x64/arm64 (static musl), macOS x64/arm64, Windows x64. Elsewhere, install with Rust: `cargo install orchnes`.

Source, documentation and issues: https://github.com/orchnesai/orchnes-cli · MIT license.
