mod window;

use libadwaita::prelude::*;

fn main() -> gtk4::glib::ExitCode {
    let app = libadwaita::Application::builder()
        .application_id("com.github.indyfive11.vm-curator-gtk")
        .build();
    app.connect_activate(|app| window::build_and_show(app));
    app.run()
}
