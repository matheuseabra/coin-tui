mod api;
mod app;
mod domain;
mod format;
mod tui;
mod ui;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    app::run().await
}
