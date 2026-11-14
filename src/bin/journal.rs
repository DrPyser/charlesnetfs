use charlesnetfs::journal::{self, JournalEntry};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "journal")]
#[command(about = "Read systemd-journal via journald-gateway HTTP API", long_about = None)]
struct Args {
    #[arg(default_value = "localhost")]
    host: String,

    #[arg(default_value_t = 19531)]
    port: u16,

    #[arg(short, long, default_value_t = 10)]
    limit: u64,

    #[arg(short, long, default_value_t = false)]
    follow: bool,
}

fn main() {
    env_logger::init();

    let args = Args::parse();

    if !args.follow {
        match journal::fetch_journal(&args.host, args.port, args.limit) {
            Ok(entries) => entries.iter().for_each(|e| println!("{}", e)),
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        log::debug!("following journal");
        match journal::fetch_journal(&args.host, args.port, args.limit) {
            Ok(entries) => entries.iter().for_each(|e| println!("{}", e)),
            Err(e) => {
                eprintln!("error: {}", e.to_string());
                std::process::exit(1);
            }
        };
        match journal::follow_journal(&args.host, args.port) {
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
            Ok(stream) => stream.for_each(|entry_or_error| match entry_or_error {
                Err(e) => eprintln!("error on journal entry: {}", e),
                Ok(entry) => println!("{}", entry),
            }),
        };
    }
}
