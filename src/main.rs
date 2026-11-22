#![forbid(unsafe_code)]
mod cli;
mod output;
mod policy;
mod transfer;
mod upload;

use std::{
    fmt::Display,
    io::{self, Write},
    process::ExitCode,
};

#[derive(Debug)]
struct Failure {
    code: u8,
    message: String,
}
impl Failure {
    fn new(code: u8, message: impl Display) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
    fn usage(message: impl Display) -> Self {
        Self::new(2, message)
    }
}

type Result<T> = std::result::Result<T, Failure>;

fn main() -> ExitCode {
    let mut opts = cli::Options::default();
    let args = std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|_| Failure::usage("arguments must be valid UTF-8"))
        })
        .collect::<Result<Vec<_>>>();
    let run = args.and_then(|args| opts.parse(args)).and_then(|()| {
        if opts.help {
            println!("{}", cli::HELP);
            return Ok(());
        }
        if opts.version {
            println!(
                "rill {} (HTTP/1.1, HTTPS via rustls)",
                env!("CARGO_PKG_VERSION")
            );
            return Ok(());
        }
        let mut status = 0;
        let mut output = output::Output::new(opts.output.as_deref(), opts.atomic_output);
        let transfer = transfer::run(&opts, &mut status, &mut output);
        if let Some(template) = &opts.write_out {
            let line = cli::format_status(template, status)?;
            let mut stdout = io::stdout().lock();
            stdout
                .write_all(line.as_bytes())
                .and_then(|_| stdout.flush())
                .map_err(|e| Failure::new(23, e))?;
        }
        transfer?;
        output.commit()
    });
    match run {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if e.code == 2 || !opts.silent || opts.show_error {
                eprintln!("rill: ({}) {}", e.code, e.message);
            }
            ExitCode::from(e.code)
        }
    }
}
