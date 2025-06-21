use clap::Parser;

#[derive(Parser, Debug)]
pub struct Config {
    #[clap(short, long, required = true)]
    pub interfaces: Vec<String>,
    #[clap(short, long, required = true)]
    pub sysname: String,
}

pub fn load_args() -> Config {
    Config::parse()
}
