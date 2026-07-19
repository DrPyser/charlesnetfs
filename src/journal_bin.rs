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
}

fn main() {
    env_logger::init();

    let args = Args::parse();

    match journal::fetch_journal(&args.host, args.port, args.limit) {
        Ok(entries) => {
            print_journal_entries(&entries);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn print_journal_entries(entries: &[JournalEntry]) {
    for entry in entries {
        println!("{}", entry);
    }
}
