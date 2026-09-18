mod editor;

fn main() {
    dioxus_web::launch::launch(editor::app, Vec::new(), Vec::new());
}
