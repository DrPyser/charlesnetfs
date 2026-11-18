use charlesnetfs::{config::Config, server};
use clap::Parser;
use std::io;
use std::path::PathBuf;
use tokio::signal::unix::{signal, SignalKind};

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
        log::debug!("explicit socket path argument {}", socket.to_string_lossy());
        return socket;
    }

    if let Some(ns) = namespace {
        log::debug!("explicit namespace argument {}", ns.to_string_lossy());
        return ns.join("charlesnetfs");
    }

    // TODO: rely on plan9port to get namespace
    let namespace = std::env::var("NAMESPACE").unwrap_or_else(|_| {
        log::debug!("$NAMESPACE not set, computing from $USER and $DISPLAY");
        let user = std::env::var("USER").expect("need $NAMESPACE or $USER");
        std::env::var("DISPLAY")
            .map(|display| format!("/tmp/ns.{}.{}", user, display))
            .unwrap_or(format!("/tmp/ns.{}", user))
    });
    PathBuf::from(namespace).join("charlesnetfs")
}

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let args = Args::parse();
    let socket_path = resolve_socket_path(args.socket, args.namespace);
    log::info!("listening on unix socket {}", socket_path.to_string_lossy());
    log::info!("config path: {}", args.config.display());

    let file = std::fs::File::open(&args.config).map_err(|e| {
        io::Error::other(format!(
            "failed to open config file {}: {}",
            args.config.display(),
            e
        ))
    })?;
    let config = Config::load(file).map_err(|e| {
        io::Error::other(format!(
            "failed to load config from {}: {}",
            args.config.to_string_lossy(),
            e
        ))
    })?;

    log::info!(
        "loaded config: {} targets, default timeout {}ms",
        config.targets.len(),
        config.global_timeout_ms
    );
    for (name, config) in config.targets.iter() {
        log::debug!("\ttarget {}: {:?}", name, config);
    }

    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let fs = server::styx9p::PingFS::new(config);
    let server = styx9p::Server::new(fs);

    let serve = tokio::spawn(async move { server.serve_unix(listener).await });

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::select! {
        _ = sigint.recv() => log::info!("received SIGINT, shutting down"),
        _ = sigterm.recv() => log::info!("received SIGTERM, shutting down"),
        result = serve => log::info!("server exited: {:?}", result),
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}
