use std::process::ExitCode;

fn main() -> ExitCode {
    match imager_app::cli::main(std::env::args_os()) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
