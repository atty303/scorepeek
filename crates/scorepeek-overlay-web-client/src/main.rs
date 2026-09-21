mod browser;
mod editor;
mod transport;

fn main() {
    dioxus_web::launch::launch(editor::app, Vec::new(), Vec::new());
}
