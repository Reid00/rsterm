// src/main.rs
use rsterm::app::App;
use rsterm::backend::backend_loop;
use rsterm::channel::AppChannels;

fn main() -> eframe::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("rsterm=debug".parse().unwrap()))
        .init();

    // Create channels
    let channels = AppChannels::new();

    // Create tokio runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime");

    // Spawn backend loop
    let event_tx = channels.event_tx.clone();
    let command_rx = channels.command_rx;
    runtime.spawn(async move {
        backend_loop(command_rx, event_tx).await;
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RSTerm")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    // Move remaining channel parts into App
    let command_tx = channels.command_tx;
    let event_rx = channels.event_rx;

    eframe::run_native(
        "RSTerm",
        options,
        Box::new(move |_cc| {
            // TODO: Load fonts
            Ok(Box::new(App::new(command_tx, event_rx, runtime)))
        }),
    )
}