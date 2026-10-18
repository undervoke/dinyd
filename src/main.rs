use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use dinyd::Error;
use dinyd::check::{self, Scope};
use dinyd::edit::{self, ApplyRequest, UpdateRequest, Updated};
use dinyd::git::{Basis, Repo};

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
    /// Judge every anchor; print one line per failing anchor.
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
        #[arg(long, conflicts_with = "reference")]
        bytes: bool,
        /// Write a delegation reference (dinyd-ref) without a digest.
        #[arg(long = "ref", id = "reference")]
        reference: bool,
        #[command(flatten)]
        basis: BasisArgs,
    },
    /// Recompute one anchor's digest with its recorded mode.
    Update {
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
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("dinyd: {e}");
            ExitCode::from(match e {
                Error::Refused(_) => 1,
                Error::Usage(_) | Error::Read(_) => 2,
            })
        }
    }
}

fn run(command: Command) -> Result<ExitCode, Error> {
    let repo = Repo::discover()?;
    match command {
        Command::Check {
            basis,
            changed,
            base,
        } => {
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
            reference,
            basis,
        } => {
            let text = edit::apply(
                &repo,
                ApplyRequest {
                    doc: &doc,
                    line,
                    selector: &selector,
                    bytes,
                    reference,
                    basis: basis.basis(),
                },
            )?;
            println!("{text}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Update {
            doc,
            line,
            expect_selector,
            basis,
        } => {
            let request = UpdateRequest {
                doc: &doc,
                line,
                expect_selector: &expect_selector,
                basis: basis.basis(),
            };
            match edit::update(&repo, request)? {
                Updated::Written(mode, value) => println!("{}:{value}", mode.as_str()),
                Updated::Unchanged => println!("unchanged"),
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
