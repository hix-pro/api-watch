use clap::Parser;

use apiwatch::cli::{Cli, run};
use apiwatch::style;

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{} {error:#}", style::red("error:"));
            std::process::exit(1);
        }
    }
}
