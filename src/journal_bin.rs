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
            Ok(entries) => {
                print_journal_entries(entries);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        log::debug!("following journal");
        match journal::fetch_journal(&args.host, args.port, args.limit) {
            Ok(entries) => {
                print_journal_entries(entries);
            }
            Err(e) => {
                eprintln!("error: {}", e.to_string());
                std::process::exit(1);
            }
        };
        // now start following
        std::thread::scope(|s| {
            let (sender, receiver) = std::sync::mpsc::sync_channel(100);
            s.spawn(|| {
                if let Err(e) = journal::follow_journal(&args.host, args.port, sender) {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                };
            });
            print_journal_entries(receiver);
        });
    }
}

fn print_journal_entries<T>(entries: T) -> ()
where
    T: IntoIterator<Item = JournalEntry>,
{
    for entry in entries {
        println!("{}", entry.to_string());
    }
}
