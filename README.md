# orchnes — Orchnes 360 CLI

[![crates.io](https://img.shields.io/crates/v/orchnes.svg)](https://crates.io/crates/orchnes) [![Downloads](https://img.shields.io/crates/d/orchnes.svg)](https://crates.io/crates/orchnes) [![CI](https://github.com/orchnesai/orchnes-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/orchnesai/orchnes-cli/actions/workflows/ci.yml) [![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE) ![Rust](https://img.shields.io/badge/Rust-2024-orange) [![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/orchnesai/orchnes-cli/badge)](https://scorecard.dev/viewer/?uri=github.com/orchnesai/orchnes-cli)

`orchnes` is the command-line companion of [**Orchnes 360**](https://github.com/orchnesai/orchnes-360), a library of [Agent Skills](https://agentskills.io)
(skill folders, each with a `SKILL.md` and an optional `references/` folder). It installs skills into any AI agent, validates them,
exports them to other agents' instruction formats, routes tasks to model tiers, and calls hosted or local models through one gateway.

It is the first step of the **runtime layer** in the Orchnes 360 [`ROADMAP.md`](https://github.com/orchnesai/orchnes-360/blob/main/ROADMAP.md):
the skills hold the knowledge; the runtime runs them across models.

```sh
cargo install orchnes
git clone https://github.com/orchnesai/orchnes-360 && cd orchnes-360
orchnes install --target agents      # Codex, Cursor, Copilot, Gemini CLI, Windsurf read .agents/skills/
orchnes ask --tier T2 "Plan a newsletter signup feature"
```

## Install

From [crates.io](https://crates.io/crates/orchnes):

```sh
cargo install orchnes
```

From source (latest `main`):

```sh
cargo install --git https://github.com/orchnesai/orchnes-cli
# or, inside a clone
cargo install --path .
```

Built and tested with Rust 1.97.

## Commands

Every command reads skills from `--skills-dir` (default `./skills`).

```sh
# Name + first sentence of each description
orchnes list --skills-dir ../orchnes-360/skills

# Print a skill's SKILL.md
orchnes show agent-orchestra

# Same checks as orchnes-360's scripts/validate.py; exit code 1 on errors
orchnes validate

# Routing matrix from agent-orchestra (each factor 1–3)
orchnes route --complexity 2 --risk 2 --context 2 --capability 1
# tier: T2
# reason: all factors ≤ 2, sum 7 (7–8 → T2)
orchnes route --complexity 1 --risk 3 --context 1 --capability 1 --sensitive
# tier: T1
# reason: risk = 3 (any factor 3 → T1)
# providers: local/compliant providers only

# Export to another tool's format
orchnes export --format cursor --out .

# Copy skill folders into an agent's native skills directory
orchnes install --target claude                 # ./.claude/skills/<name>/
orchnes install --target agents --scope user    # ~/.agents/skills/<name>/
orchnes install --target cursor --dest ../app agent-behavior memory
```

### Validation

`validate` checks that SKILL.md exists and has frontmatter that parses as YAML. The name must be 1–64 characters of `[a-z0-9-]`, with no leading or trailing hyphen and no `--`, and must match the folder name. The description must be non-empty and at most 1024 characters. SKILL.md must be 500 lines or fewer, and every `` `references/...` `` file it mentions must exist. Two repo-level checks run only when the skills folder's parent directory has them: `evals/<name>.md` must exist, and `README.md` must list `skills/<name>/SKILL.md`.

### Routing

The rule from `skills/agent-orchestra/references/routing-matrix.md`: if any factor is 3, the tier is T1. Otherwise a sum of 6 or less gives T3, and a sum of 7–8 gives T2. `--sensitive` restricts the task to local or compliant providers, whatever the tier.

### Export formats

| `--format` | Output (under `--out`) |
|---|---|
| `agents-md` | `AGENTS.md`: short intro and an index of skills with paths to each SKILL.md, relative to `--out` |
| `gemini` | `GEMINI.md`, with the same content as `agents-md` |
| `bundle` | `SKILLS.md`: table of contents plus every SKILL.md body, for system prompts or custom instructions |
| `aider` | `CONVENTIONS.md`, with the same content as `bundle` |
| `cursor` | `.cursor/rules/<name>.mdc` with `description`, empty `globs` and `alwaysApply: false` (`true` for `agent-behavior`) |
| `copilot` | `.github/copilot-instructions.md`: index of skills (pointing to `.github/skills/`) + `agent-behavior` body. Copilot loads skills natively, so install them with `orchnes install --target copilot` instead of per-file instructions that would load every skill on every request |
| `windsurf` | `.windsurf/rules/<name>.md` with `trigger: model_decision` (`always_on` for `agent-behavior`) and `description` |
| `continue` | `.continue/rules/<name>.md` with `name`, `description` and `alwaysApply` |
| `claude-zip` | `<name>.zip` per skill, with the skill folder at the archive root, for upload to Claude.ai |

Rule formats (`cursor`, `windsurf`, `continue`) contain only the SKILL.md body, with a note pointing to `.agents/skills/<name>/references/`; run `orchnes install --target agents` so those reference files exist. `AGENTS.md`/`GEMINI.md` index entries point to the same install location.

Gemini CLI can also read `AGENTS.md` directly if you add `{"context":{"fileName":["AGENTS.md","GEMINI.md"]}}` to its `settings.json`.

### Install targets

| `--target` | `--scope project` (under `--dest`, default `.`) | `--scope user` (under `--dest`, default `$HOME`) |
|---|---|---|
| `agents`, `codex` | `.agents/skills/` (read by Codex, Cursor, Copilot, Gemini CLI, Windsurf) | `~/.agents/skills/` |
| `claude` | `.claude/skills/` | `~/.claude/skills/` |
| `cline` | `.cline/skills/` | `~/.cline/skills/` |
| `cursor` | `.cursor/skills/` | `~/.cursor/skills/` |
| `copilot` | `.github/skills/` | `~/.copilot/skills/` |
| `gemini` | `.gemini/skills/` | `~/.gemini/skills/` |
| `windsurf` | `.windsurf/skills/` | `~/.agents/skills/` |

If a skill folder already exists at the destination, `install` refuses to continue. Pass `--force` to replace the folder.

## Model gateway

`orchnes ask` sends a prompt to a model through one interface and `orchnes models` shows what is configured.
Two adapters cover most models:

| `provider` | API | Works with |
|---|---|---|
| `openai` | OpenAI-compatible `POST {base_url}/chat/completions` | OpenAI, OpenRouter, LiteLLM, Ollama, vLLM, LM Studio, Gemini's OpenAI-compatible endpoint, … |
| `anthropic` | Anthropic Messages `POST {base_url}/v1/messages` (`anthropic-version: 2026-10-21`) | Claude models |

```sh
cp orchnes.example.yaml orchnes.yaml          # edit models, tiers, prices
export ANTHROPIC_API_KEY=...                    # keys only via the env vars named in the config
orchnes models                                 # tier, provider, local/hosted, credential env var set/MISSING (never the value)
orchnes ask --tier T2 "Summarize this in 3 bullets: ..."
orchnes ask --complexity 2 --risk 3 --context 1 --capability 2 --skill security-operations "Review this config: ..."
orchnes ask --tier T3 --sensitive - < patient-notes.txt   # only local/compliant models
orchnes ask --tier T1 --dry-run "..."          # print the request (keys masked) without sending
orchnes ask --tier T2 --log run.log "..."      # append an agent-orchestra run-log line
```

How it chooses and recovers:
- **Tier** comes from `--tier` or the routing matrix (`--complexity --risk --context --capability`).
- **Order**: models of that tier in config order, then higher tiers only (T3 → T2 → T1). A weaker model never silently takes a harder task.
- **Fallback**: any failure — HTTP error (429, 5xx, 529 overloaded…), timeout, refusal (`stop_reason: refusal`), content filter, empty answer, or an unset key env var — moves on to the next model; every attempt is reported on stderr.
- **Sensitive** (`--sensitive`): only models marked `local: true` or `compliant: true`; if none fit, the request fails instead of leaking data.
- **Cost**: `price_in`/`price_out` (USD per 1M tokens) give a per-call cost on stderr and in the run log.
- **Skills**: `--skill NAME` uses that skill's SKILL.md body as the system prompt.
- **Anthropic**: `server_fallback: true` adds Anthropic's server-side refusal fallback (`fallbacks: "default"` with the `server-side-fallback-2026-07-01` beta); only text blocks are returned (thinking blocks are skipped).
- **OpenAI-compatible**: `max_tokens_param` renames the output-limit field for providers that require `max_completion_tokens`.

Requests are blocking and non-streaming (default `--max-tokens 16000`, `timeout_secs` 600).

## Library

The crate is also a library (`orchnes`) with these modules for later runtime work:

- `skill`: parse frontmatter, load and check skills.
- `validate`: the validate.py checks.
- `routing`: the tier matrix (`Tier`, `Factors`, `route`).
- `contract`: `TaskContract` and `Verdict`, round-tripped through YAML. Fields follow agent-orchestra's `references/contracts.md`.
- `runlog`: the append-only run log line format from `contracts.md` (`Entry`, `append`, `read`).
- `export`, `install`: the formats and targets above.
- `gateway`: model config, request building/parsing per provider, candidate order, fallback (`complete`), with a `Transport` trait so it is testable without a network.

## Development

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Integration tests run the binary against temporary fixtures. `tests/gateway_http.rs` runs `orchnes ask` over real HTTP against a local fake provider (no external network). They also run against the real orchnes-360 repo, read-only, when it is at `../orchnes-360/skills` or at the path in `ORCHNES_SKILLS_DIR`. If neither exists, those tests print `SKIPPED` and pass.

## Not implemented yet (roadmap)

- **Gateway next steps**: streaming, tool use, per-run budgets and a hard spend limit, retries with backoff on the same model, dispatching `TaskContract`s.
- **Review loop**: automated checks, then `Verdict`s, then the REVISE/REASSIGN escalation ladder.
- **24/7 scheduling**: long-running and scheduled runs with a kill switch.
- **Agent messaging**: a structured task board or queue between agents, plus notifications.
- **Audit log**: `ask --log` writes run-log lines; still to do for every dispatched task in a multi-agent run.

## Releasing

Releases are automatic. Bump `version` in `Cargo.toml` (and add a `CHANGELOG.md` entry) in a pull request; when it merges to `main`, [`publish.yml`](.github/workflows/publish.yml) publishes the new version to [crates.io](https://crates.io/crates/orchnes) through Trusted Publishing (no stored token) and creates the matching GitHub Release `vX.Y.Z`. Versions already on crates.io are skipped.

## License

MIT © Ömer Faruk Alibal · [Orchnes](https://github.com/orchnesai). See [LICENSE](LICENSE). Changes: [CHANGELOG.md](CHANGELOG.md). Security: [SECURITY.md](SECURITY.md).
