//! grandaddy — every coding agent on your Mac, in one place.
mod cli;
mod hub;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `grandaddy ls | head` should end quietly when the reader goes away, like any Unix tool,
    // instead of panicking on the closed pipe. Not for the long-running commands that write to
    // sockets (`daemon` serves the phone page, `run` takes prompts): a client hanging up there
    // must not take the process down with it.
    if !matches!(args.first().map(String::as_str), Some("daemon" | "run")) {
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    }
    std::process::exit(cli::main(args));
}
