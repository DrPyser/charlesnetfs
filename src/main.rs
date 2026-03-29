use std::env;
use std::io;
use std::io::Read;
mod pinger;

const DEFAULT_PORT: u32 = 22;

fn main(){
    // [hostname] [timeout]
    match &env::args().collect::<Vec<String>>()[1..] {
        [hostname, timeout] => {
            let mut data = String::new();
            let _ = io::stdin().read_to_string(&mut data);
            let address = format!("{}:{}", hostname, DEFAULT_PORT);
            let result = pinger::ping(address.as_str(), timeout.parse().unwrap(), &data);
            match result {
                Err(e) => {
                    eprintln!("{}", e);
                },
                Ok(result) => {
                    println!("{}", result);
                }
            }
        },
        _ => {
            eprintln!("bad");
        }
    }
}
