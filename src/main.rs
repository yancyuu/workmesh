use clap::Parser;
use workmesh::tasks::cli::{Cli, run};

fn main() {
    let json = std::env::args().any(|arg| arg == "--json");
    if let Err(error) = run(Cli::parse()) {
        if json {
            eprintln!("{}", serde_json::json!({"error": format!("{error:#}")}));
        } else {
            eprintln!("Error: {error:#}");
        }
        std::process::exit(1);
    }
}
