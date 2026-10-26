use std::sync::Arc;

use gpui_kit::{prelude::*, *};
use rsterm::{
    app::App as RstermApp, backend::backend_loop, channel::AppChannels, config::ConfigManager,
    ssh::SshConfigSource,
};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("rsterm=info".parse()?),
        )
        .init();
    let config = ConfigManager::new()?;
    let shortcuts = config.load_keybindings();
    let ssh_source = dirs::home_dir().map(|home| SshConfigSource {
        path: home.join(".ssh/config"),
        home,
    });
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );
    let AppChannels {
        command_tx,
        command_rx,
        event_tx,
        event_rx,
    } = AppChannels::new();
    let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
    runtime.spawn(async move {
        backend_loop(command_rx, event_tx).await;
        let _ = stopped_tx.send(());
    });

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            rsterm::ui::terminal::init(cx);
            let shutdown_sender = command_tx.clone();
            let shutdown_runtime = runtime.clone();
            let mut stopped_rx = Some(stopped_rx);
            cx.on_app_quit(move |_| {
                let sender = shutdown_sender.clone();
                let runtime = shutdown_runtime.clone();
                let stopped = stopped_rx.take();
                async move {
                    let _runtime = runtime;
                    let _ = sender.send(rsterm::backend::AppCommand::Shutdown).await;
                    if let Some(stopped) = stopped {
                        let _ = stopped.await;
                    }
                }
            })
            .detach();
            rsterm::shortcuts::init(shortcuts, cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1280.), px(820.)),
                    cx,
                ))),
                window_min_size: Some(size(px(860.), px(600.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("RSTerm".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            if let Err(error) = gpui_kit::open_window(options, cx, move |window, cx| {
                let app = cx.new(|cx| {
                    RstermApp::new(
                        config, ssh_source, command_tx, event_rx, runtime, window, cx,
                    )
                });
                app.update(cx, |app, cx| app.load(window, cx));
                app
            }) {
                tracing::error!(%error, "Unable to open RSTerm window");
                cx.quit();
            }
            cx.activate(true);
        });
    Ok(())
}
