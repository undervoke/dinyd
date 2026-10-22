use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use dinyd::Error;
use dinyd::check::{self, Scope};
use dinyd::edit::{self, ApplyRequest, DigestRequest, Digested};
use dinyd::git::{Basis, Repo};
use dinyd::{guard, install, runtime};

#[derive(Parser)]
#[command(
    version,
    about = "Ties documentation claims to the code regions they restate"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Prepare the project runtime and chosen agent-host bindings.
    Init {
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        hosts: Option<String>,
    },
    /// Update this installation to the latest stable release.
    Update,
    /// Judge a host's proposed file edit before it executes.
    Guard {
        #[arg(long, value_parser = guard::HOSTS)]
        host: String,
    },
    /// Judge references and local links; print one line per failure.
    Check {
        #[command(flatten)]
        basis: BasisArgs,
        /// Only anchors of changed documents and of changed targets.
        #[arg(long)]
        changed: bool,
        /// Revision to compare against for --changed (default HEAD).
        #[arg(long, requires = "changed")]
        base: Option<String>,
    },
    /// Create one anchor with its mode and first digest.
    Apply {
        doc: PathBuf,
        #[arg(long)]
        line: usize,
        #[arg(long)]
        selector: String,
        /// Use b1 (bytes) instead of the format default.
        #[arg(long)]
        bytes: bool,
        #[command(flatten)]
        basis: BasisArgs,
    },
    /// Recompute one anchor's digest with its recorded mode.
    Digest {
        doc: PathBuf,
        #[arg(long)]
        line: usize,
        #[arg(long)]
        expect_selector: String,
        #[command(flatten)]
        basis: BasisArgs,
    },
}

#[derive(Args)]
#[group(multiple = false)]
struct BasisArgs {
    /// Read documents and targets from the index (default).
    #[arg(long)]
    index: bool,
    /// Read documents and targets from the working tree.
    #[arg(long)]
    worktree: bool,
    /// Read documents and targets from a commit.
    #[arg(long, value_name = "COMMIT")]
    rev: Option<String>,
}

impl BasisArgs {
    fn basis(self) -> Basis {
        match (self.worktree, self.rev) {
            (true, _) => Basis::Worktree,
            (_, Some(rev)) => Basis::Rev(rev),
            _ => Basis::Index,
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let host = if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        None
    } else {
        hook_host(&args)
    };
    let mut input = Vec::new();
    if let Some(host) = &host
        && io::stdin().read_to_end(&mut input).is_err()
    {
        return guard_reply(host, Some("dinyd: could not read hook input"));
    }
    let hook = host.as_deref().map(|host| (host, input.as_slice()));
    match runtime::dispatch(&args, hook) {
        Ok(Some(code)) => return ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Ok(None) => {}
        Err(error) => {
            if let Some(host) = &host {
                return guard_reply(host, Some(&format!("dinyd: {error}")));
            }
            return report(error);
        }
    }
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => {
            if let Some(host) = &host {
                return guard_reply(host, Some("dinyd: invalid guard arguments"));
            }
            error.exit();
        }
    };
    match run(cli.command, &input) {
        Ok(code) => code,
        Err(error) => report(error),
    }
}

fn hook_host(args: &[OsString]) -> Option<String> {
    if args.get(1).is_none_or(|arg| arg != OsStr::new("guard")) {
        return None;
    }
    for (index, arg) in args.iter().enumerate().skip(2) {
        if arg == "--host" {
            return args
                .get(index + 1)
                .and_then(|value| value.to_str())
                .map(str::to_owned);
        }
        if let Some(host) = arg.to_str().and_then(|value| value.strip_prefix("--host=")) {
            return Some(host.to_owned());
        }
    }
    None
}

fn guard_reply(host: &str, reason: Option<&str>) -> ExitCode {
    let (response, code) = guard::response(host, reason);
    println!("{response}");
    ExitCode::from(code)
}

fn report(error: Error) -> ExitCode {
    eprintln!("dinyd: {error}");
    ExitCode::from(match error {
        Error::Refused(_) => 1,
        Error::Usage(_) | Error::Read(_) => 2,
    })
}

fn run(command: Command, input: &[u8]) -> Result<ExitCode, Error> {
    match command {
        Command::Init { version, hosts } => {
            install::init(version.as_deref(), hosts.as_deref())?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Update => {
            runtime::self_update()?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Guard { host } => {
            let payload = match serde_json::from_slice(input) {
                Ok(payload) => payload,
                Err(_) => return Ok(guard_reply(&host, Some("dinyd: invalid hook input"))),
            };
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(_) => {
                    return Ok(guard_reply(
                        &host,
                        Some("dinyd: could not locate the workspace"),
                    ));
                }
            };
            let context = guard::context(&payload, &cwd);
            let reason = guard::evaluate(&host, &payload, &context).err();
            Ok(guard_reply(&host, reason.as_deref()))
        }
        Command::Check {
            basis,
            changed,
            base,
        } => {
            let repo = Repo::discover()?;
            let basis = basis.basis();
            let scope = match (changed, base) {
                (false, _) => Scope::All,
                (true, Some(base)) => Scope::Changed(base),
                (true, None) if matches!(basis, Basis::Rev(_)) => {
                    return Err(Error::Usage(
                        "--changed with --rev needs --base".to_string(),
                    ));
                }
                (true, None) => Scope::Changed("HEAD".to_string()),
            };
            let findings = check::check(&repo, basis, scope)?;
            for finding in &findings {
                println!("{finding}");
                if let Some(reason) = &finding.reason {
                    eprintln!("dinyd: {} → {}: {reason}", finding.doc, finding.selector);
                }
            }
            Ok(if findings.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Apply {
            doc,
            line,
            selector,
            bytes,
            basis,
        } => {
            let repo = Repo::discover()?;
            let (text, basis) = edit::apply(
                &repo,
                ApplyRequest {
                    doc: &doc,
                    line,
                    selector: &selector,
                    bytes,
                    basis: basis.basis(),
                },
            )?;
            println!("{text}");
            eprintln!("dinyd: target read from {basis}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Digest {
            doc,
            line,
            expect_selector,
            basis,
        } => {
            let repo = Repo::discover()?;
            let request = DigestRequest {
                doc: &doc,
                line,
                expect_selector: &expect_selector,
                basis: basis.basis(),
            };
            match edit::digest(&repo, request)? {
                Digested::Written(mode, value) => println!("{}:{value}", mode.as_str()),
                Digested::Unchanged => println!("unchanged"),
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
