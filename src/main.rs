mod hypr;
mod json;
mod term;
mod tui;

fn main() {
    if std::env::args().len() > 1 {
        eprintln!("usage: fast-hyprmon\nterminal monitor layout manager for Hyprland");
        std::process::exit(2);
    }
    if let Err(e) = tui::run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
