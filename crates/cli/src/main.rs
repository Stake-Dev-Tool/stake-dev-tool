use clap::{Parser, Subcommand};

/// Stake Dev Tool CLI — drive the cloud platform from a terminal or CI.
#[derive(Parser)]
#[command(name = "sdt", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Push a math folder as a new revision (M2 — not implemented yet).
    Push,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Push => anyhow::bail!("not implemented yet — M2 in progress"),
    }
}
