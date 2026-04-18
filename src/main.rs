use charlesnetfs::{config::Config, server::PingFS};
use clap::Parser;
use ninep::sync::server::{socket_path, Server};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "charlesnetfs")]
#[command(about = "9P network filesystem", long_about = None)]
struct Args {
    #[arg(short, long, default_value = "config.yaml")]
    config: PathBuf,

    #[arg(short, long)]
    socket: Option<PathBuf>,

    #[arg(short, long)]
    namespace: Option<PathBuf>,
}

fn resolve_socket_path(socket: Option<PathBuf>, namespace: Option<PathBuf>) -> PathBuf {
    if let Some(socket) = socket {
        return socket;
    }

    if let Some(ns) = namespace {
        return ns.join("charlesnetfs");
    }

    socket_path("charlesnetfs")
}

fn main() -> Result<(), String> {
    env_logger::init();

    let args = Args::parse();

    let file = std::fs::File::open(&args.config).map_err(|e| {
        format!(
            "failed to open config file {}: {}",
            args.config.display(),
            e
        )
    })?;
    let config = Config::load(file).map_err(|e| {
        format!(
            "failed to load config from {}: {}",
            args.config.to_string_lossy(),
            e
        )
    })?;

    let fs = PingFS::new(config);

    let socket_path = resolve_socket_path(args.socket, args.namespace);

    log::info!("Starting 9P server at {:?}", socket_path);

    let server = Server::new(fs);
    let _handle = server.serve_socket_with_custom_path(socket_path.clone());

    let mut signals = Signals::new([SIGINT, SIGTERM]).map_err(|e| e.to_string())?;

    // blocks until signal
    let _ = signals.forever().next();

    let _ = std::fs::remove_file(&socket_path);
    std::process::exit(0);
}
