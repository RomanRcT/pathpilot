use adw::{gio, prelude::*};
use tracing::{info, info_span};
use tracing_subscriber::EnvFilter;

const APP_ID: &str = "io.github.RomanRcT.PathPilot";

fn main() -> gtk::glib::ExitCode {
    init_tracing();
    let startup = info_span!("application_startup");
    let _guard = startup.enter();

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::empty())
        .build();
    app.add_main_option(
        "new-instance",
        gtk::glib::Char::from(0),
        gtk::glib::OptionFlags::NONE,
        gtk::glib::OptionArg::None,
        "Run a separate instance instead of activating an already running version",
        None,
    );
    app.connect_handle_local_options(|app, options| {
        if options.contains("new-instance") {
            app.set_flags(gio::ApplicationFlags::NON_UNIQUE);
        }
        std::ops::ControlFlow::Continue(())
    });
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
