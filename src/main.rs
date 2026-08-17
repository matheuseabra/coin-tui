mod api;
mod app;
mod config;
mod domain;
mod format;
mod log;
mod tui;
mod ui;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let config = match config::load() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("coin-tui: {message}");
            std::process::exit(2);
        }
    };
    app::run(config).await
}
