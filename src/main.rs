use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use orchnes::export::{self, Format};
use orchnes::gateway::{self, HttpTransport, Request};
use orchnes::install::{self, Scope, Target};
use orchnes::routing::{self, Factors, SENSITIVE_NOTE, Tier};
use orchnes::runlog;
use orchnes::skill::{self, Skill};
use orchnes::validate;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

/// Load, validate, route and export Agent Skills, and call models through one gateway.
#[derive(Parser)]
#[command(name = "orchnes", version)]
struct Cli {
    /// Folder containing one subfolder per skill.
    #[arg(long, global = true, default_value = "skills")]
    skills_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List skills: name and first sentence of the description.
    List,
    /// Print a skill's SKILL.md.
    Show { name: String },
    /// Check every skill like orchnes-360's scripts/validate.py; exit 1 on errors.
    Validate,
    /// Pick a model tier with the agent-orchestra routing matrix (factors 1–3).
    Route {
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
        complexity: u8,
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
        risk: u8,
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
        context: u8,
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
        capability: u8,
        /// Task handles privacy-sensitive data.
        #[arg(long)]
        sensitive: bool,
    },
    /// Write skills in another agent's instruction format.
    Export {
        #[arg(long, value_enum)]
        format: Format,
        #[arg(long)]
        out: PathBuf,
    },
    /// List models in the gateway config and whether their API key is set.
    Models {
        #[arg(long, default_value = "orchnes.yaml")]
        config: PathBuf,
    },
    /// Send a prompt through the model gateway (tier routing, fallback, cost).
    Ask {
        #[arg(long, default_value = "orchnes.yaml")]
        config: PathBuf,
        /// Model tier; or give all four routing factors instead.
        #[arg(long, value_enum)]
        tier: Option<Tier>,
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
        complexity: Option<u8>,
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
        risk: Option<u8>,
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
        context: Option<u8>,
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
        capability: Option<u8>,
        /// Privacy-sensitive: only local or compliant models.
        #[arg(long)]
        sensitive: bool,
        /// Use this skill's SKILL.md body as the system prompt.
        #[arg(long)]
        skill: Option<String>,
        #[arg(long, default_value_t = 16000)]
        max_tokens: u32,
        /// Print the request that would be sent (keys masked) and exit.
        #[arg(long)]
        dry_run: bool,
        /// Append a run-log line to this file.
        #[arg(long)]
        log: Option<PathBuf>,
        /// Prompt text, or `-` to read it from stdin.
        prompt: String,
    },
    /// Copy skill folders into an agent's native skills directory.
    Install {
        #[arg(long, value_enum)]
        target: Target,
        #[arg(long, value_enum, default_value = "project")]
        scope: Scope,
        /// Base directory (default: current dir for project scope, home for user scope).
        #[arg(long)]
        dest: Option<PathBuf>,
        /// Replace skill folders that already exist at the destination.
        #[arg(long)]
        force: bool,
        /// Skills to install (default: all).
        names: Vec<String>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let dir = &cli.skills_dir;
    match cli.command {
        Command::List => {
            for s in load_all(dir)? {
                println!(
                    "{:<26} {}",
                    s.name(),
                    skill::first_sentence(s.description())
                );
            }
        }
        Command::Show { name } => print!("{}", load_one(dir, &name)?.raw),
        Command::Validate => {
            let report = validate::validate(dir)?;
            for e in &report.errors {
                println!("ERROR {e}");
            }
            println!(
                "{} skills checked, {} errors",
                report.checked,
                report.errors.len()
            );
            if !report.errors.is_empty() {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Route {
            complexity,
            risk,
            context,
            capability,
            sensitive,
        } => {
            let f = Factors {
                complexity,
                risk,
                context,
                capability,
            };
            let r = routing::route(f, sensitive)?;
            println!("tier: {}\nreason: {}", r.tier, r.reason);
            if r.local_only {
                println!("providers: {SENSITIVE_NOTE}");
            }
        }
        Command::Models { config } => {
            let cfg = gateway::load_config(&config)?;
            for m in &cfg.models {
                // Report only whether the named variable is set — never read or print its value.
                let credential_status = match &m.credential_env {
                    Some(var) if env_set(var) => format!("env {var}: set"),
                    Some(var) => format!("env {var}: MISSING"),
                    None => "no credential".into(),
                };
                let place = if m.local {
                    "local"
                } else if m.compliant {
                    "compliant"
                } else {
                    "hosted"
                };
                println!(
                    "{:<20} {}  {:<9} {:<9} {:<28} {credential_status}",
                    m.id,
                    m.tier,
                    format!("{:?}", m.provider).to_lowercase(),
                    place,
                    m.model
                );
            }
        }
        Command::Ask {
            config,
            tier,
            complexity,
            risk,
            context,
            capability,
            sensitive,
            skill,
            max_tokens,
            dry_run,
            log,
            prompt,
        } => {
            let cfg = gateway::load_config(&config)?;
            let (tier, why) = match (tier, complexity, risk, context, capability) {
                (Some(t), None, None, None, None) => (t, format!("tier {t} requested")),
                (None, Some(complexity), Some(risk), Some(context), Some(capability)) => {
                    let f = Factors {
                        complexity,
                        risk,
                        context,
                        capability,
                    };
                    let r = routing::route(f, sensitive)?;
                    (r.tier, r.reason)
                }
                _ => bail!(
                    "give either --tier or all four of --complexity --risk --context --capability"
                ),
            };
            let prompt = if prompt == "-" {
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s)?;
                s
            } else {
                prompt
            };
            let system = match &skill {
                Some(name) => Some(load_one(dir, name)?.body),
                None => None,
            };
            let req = Request {
                system,
                prompt,
                max_tokens,
            };
            if dry_run {
                let models = gateway::candidates(&cfg, tier, sensitive);
                let Some(first) = models.first() else {
                    bail!(gateway::GatewayError::NoCandidate {
                        tier,
                        local_only: sensitive
                    });
                };
                let http = gateway::build(first, &req, Some("<api key>")).redacted();
                println!("model: {} (tier {}, {why})", first.id, first.tier);
                println!(
                    "order: {}",
                    models
                        .iter()
                        .map(|m| m.id.as_str())
                        .collect::<Vec<_>>()
                        .join(" → ")
                );
                println!("POST {}", http.url);
                for (k, v) in &http.headers {
                    println!("{k}: {v}");
                }
                println!("{}", serde_json::to_string_pretty(&http.body)?);
                return Ok(ExitCode::SUCCESS);
            }
            let transport = HttpTransport::new(Duration::from_secs(cfg.timeout_secs));
            let (c, attempts) = gateway::complete(&cfg, &transport, &req, tier, sensitive, &|v| {
                std::env::var(v).ok()
            })?;
            for a in &attempts {
                eprintln!("tried {}: {}", a.model_id, a.error);
            }
            let cost = c
                .cost_usd
                .map_or_else(|| "cost n/a".to_string(), |x| format!("${x:.6}"));
            eprintln!(
                "model={} tier={} tokens in={} out={} {cost}",
                c.model_id, c.tier, c.input_tokens, c.output_tokens
            );
            println!("{}", c.text);
            if let Some(path) = log {
                let entry = runlog::Entry {
                    timestamp: runlog::utc_timestamp(std::time::SystemTime::now()),
                    task: "ASK".into(),
                    role: skill.unwrap_or_else(|| "ask".into()),
                    model: c.model_id.clone(),
                    tier: c.tier.to_string(),
                    verdicts: "-".into(),
                    evidence: format!("{} failed before success", attempts.len()),
                    tokens: format!("{} tok", c.input_tokens + c.output_tokens),
                    cost,
                    why,
                };
                runlog::append(&path, &entry)?;
            }
        }
        Command::Export { format, out } => {
            let written = export::export(&load_all(dir)?, format, &out)?;
            println!("wrote {} files to {}", written.len(), out.display());
        }
        Command::Install {
            target,
            scope,
            dest,
            force,
            names,
        } => {
            let skills = if names.is_empty() {
                load_all(dir)?
            } else {
                names
                    .iter()
                    .map(|n| load_one(dir, n))
                    .collect::<Result<_>>()?
            };
            let base = match (dest, scope) {
                (Some(d), _) => d,
                (None, Scope::Project) => PathBuf::from("."),
                (None, Scope::User) => home()?,
            };
            let dest = base.join(install::skills_subdir(target, scope));
            let installed = install::install(&skills, &dest, force)?;
            println!("installed {} skills to {}", installed.len(), dest.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn load_all(dir: &Path) -> Result<Vec<Skill>> {
    skill::load_all(dir).with_context(|| format!("loading skills from {}", dir.display()))
}

fn load_one(dir: &Path, name: &str) -> Result<Skill> {
    if !skill::is_valid_name(name) {
        bail!("invalid skill name '{name}'");
    }
    Ok(skill::load_skill(&dir.join(name))?)
}

fn env_set(var: &str) -> bool {
    std::env::var_os(var).is_some_and(|v| !v.is_empty())
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("cannot find home directory (set HOME or pass --dest)")
}
