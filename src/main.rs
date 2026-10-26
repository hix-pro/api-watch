use clap::Parser;

use apiwatch::cli::{Cli, run};
use apiwatch::style;

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // clap exits 2 by default, but that collides with the documented
            // --fail-on-breaking exit code. Help and version stay successful.
            let code = match error.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => 0,
                _ => 1,
            };
            let _ = error.print();
            std::process::exit(code);
        }
    };

    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{} {error:#}", style::red("error:"));
            std::process::exit(1);
        }
    }
}
