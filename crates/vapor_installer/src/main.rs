use std::process;

fn main() {
    if let Err(error) = vapor_installer::cli::run_from_env() {
        eprintln!("error: {error}");
        process::exit(1);
    }
}
