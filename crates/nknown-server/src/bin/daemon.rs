//! `nknownd` : démon local (voir [`nknown_server::daemon`]).

fn main() -> std::process::ExitCode {
    nknown_server::daemon::main()
}
