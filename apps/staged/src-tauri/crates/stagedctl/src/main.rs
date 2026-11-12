use clap::Parser;

fn main() {
    let cli = match stagedctl::cli::Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            // Help and version render on stdout and exit 0; real usage
            // errors go to stderr with exit 1 (a command error — exit 2 is
            // reserved for transport faults).
            let code = i32::from(err.use_stderr());
            let _ = err.print();
            std::process::exit(code);
        }
    };

    let json = cli.json;
    match stagedctl::commands::run(cli) {
        Ok(output) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&output.json)
                        .expect("command output is always serializable")
                );
            } else {
                println!("{}", output.human);
            }
        }
        Err(err) => {
            // A multi-step write that failed part-way says what landed on
            // stdout, then the error on stderr, as a success would have.
            if let Some(partial) = err.partial_output(json) {
                println!("{partial}");
            }
            eprintln!("{err}");
            std::process::exit(err.exit_code());
        }
    }
}
