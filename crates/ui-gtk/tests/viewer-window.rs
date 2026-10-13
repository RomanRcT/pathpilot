//! GUI checks run on the process main thread, as required by WebKit.
// These shared modules expose additional browser methods not used by this harness.
#[allow(dead_code, unused_imports)]
#[path = "../src/directory_pane.rs"]
mod directory_pane;
#[allow(dead_code, unused_imports)]
#[path = "../src/markdown_view.rs"]
mod markdown_view;
#[allow(dead_code, unused_imports)]
#[path = "../src/preview_pane.rs"]
mod preview_pane;
#[allow(dead_code)]
#[path = "../src/viewer.rs"]
mod viewer;

fn main() {
    if !std::env::args().any(|argument| argument == "--ignored") {
        println!(
            "Viewer GUI check skipped; run with --test viewer-window -- --ignored in a graphical session"
        );
        return;
    }
    viewer::tests::run_gui_check();
    println!("Independent viewer integration check passed");
}
