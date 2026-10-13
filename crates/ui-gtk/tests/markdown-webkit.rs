//! Opt-in main-thread WebKit integration check (no Rust test worker thread).
#[path = "../src/markdown_view.rs"]
mod markdown_view;

fn main() {
    if !std::env::args().any(|argument| argument == "--ignored") {
        println!(
            "WebKit integration check skipped; run with --test markdown-webkit -- --ignored in a graphical session"
        );
        return;
    }
    markdown_view::tests::run_visual_check();
    println!("WebKit Markdown integration check passed");
}
