//! grandaddy — manage your vibe coding sessions. All in one place.
mod cli;
mod hub;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(cli::main(args));
}
