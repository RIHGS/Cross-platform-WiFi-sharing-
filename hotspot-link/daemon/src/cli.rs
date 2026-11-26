use clap::Subcommand;

#[derive(Subcommand)]
enum Commands {
    Status,
    Pair,
    Forget,
}