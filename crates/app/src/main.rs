use gtk::{gio, prelude::*};
use tracing::{info, info_span};
use tracing_subscriber::EnvFilter;

const APP_ID: &str = "io.github.pathpilot.PathPilot";

fn main() -> gtk::glib::ExitCode {
    init_tracing();
    let startup = info_span!("application_startup");
    let _guard = startup.enter();

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::empty())
        .build();
    app.connect_activate(|app| {
        let started = std::time::Instant::now();
        let window = pathpilot_ui_gtk::build_window(app);
        window.present();
        info!(
            elapsed_ms = started.elapsed().as_millis(),
            "window presented"
        );
    });
    app.run()
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("pathpilot=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
