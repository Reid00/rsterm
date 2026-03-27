// src/main.rs
use rsterm::app::App;
use rsterm::backend::backend_loop;
use rsterm::channel::AppChannels;

fn main() -> eframe::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("rsterm=debug".parse().unwrap()),
        )
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

    // Check for renderer override via environment variable
    // Options: "wgpu" (default), "glow" (OpenGL)
    let renderer = match std::env::var("RSTERM_RENDERER")
        .unwrap_or_default()
        .as_str()
    {
        "glow" => eframe::Renderer::Glow,
        _ => eframe::Renderer::Wgpu,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RSTerm")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 600.0]),
        renderer,
        ..Default::default()
    };

    // Move remaining channel parts into App
    let command_tx = channels.command_tx;
    let event_rx = channels.event_rx;

    let result = eframe::run_native(
        "RSTerm",
        options,
        Box::new(move |cc| {
            // Set up fonts with CJK support
            let mut fonts = egui::FontDefinitions::default();

            // Try to load system CJK fonts at runtime
            if let Ok(cjk_font) = std::fs::read("C:/Windows/Fonts/msyh.ttc") {
                fonts.font_data.insert(
                    "cjk".to_owned(),
                    egui::FontData::from_owned(cjk_font),
                );
                // Add CJK to monospace font family as fallback
                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .push("cjk".to_owned());
                // Also add to proportional for UI text
                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .push("cjk".to_owned());
            }

            cc.egui_ctx.set_fonts(fonts);

            Ok(Box::new(App::new(command_tx, event_rx, runtime)))
        }),
    );

    // Provide helpful error message for rendering failures
    if let Err(e) = &result {
        let err_str = format!("{:?}", e);
        let err_display = e.to_string();
        if err_str.contains("NoSuitableAdapterFound")
            || err_display.contains("NoSuitableAdapterFound")
            || err_display.contains("opengl 2.0")
            || err_str.contains("opengl 2.0")
        {
            eprintln!("\n========================================");
            eprintln!("RSTerm requires a graphics driver to run.");
            eprintln!("\nDetected issue: No suitable GPU or OpenGL driver found.");
            eprintln!("\nPossible solutions:");
            eprintln!("  1. Run on a machine with a proper GPU/display driver");
            eprintln!("  2. For Remote Desktop: Enable GPU redirection or use local display");
            eprintln!("  3. Install Mesa3D software renderer (llvmpipe)");
            eprintln!("     Download from: https://github.com/pal1000/mesa-dist-win");
            eprintln!("  4. Try environment variables:");
            eprintln!("     set RSTERM_RENDERER=wgpu");
            eprintln!("     set WGPU_BACKEND=gl");
            eprintln!("========================================\n");
        }
    }

    result
}
