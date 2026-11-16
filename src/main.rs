use anyhow::Result;
use clap::Parser;

use aigit::commands::{Cli, run};

#[tokio::main]
async fn main() -> Result<()> {
    run(Cli::parse()).await
}
