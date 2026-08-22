mod api;
mod app;
mod config;
mod detail;
mod domain;
mod format;
mod http;
mod input;
mod log;
mod news;
mod pane;
mod refresh;
mod sentiment;
mod theme;
mod tui;
mod ui;
mod view;

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
