use bevy::prelude::*;
use bevy_slt::{SltAppExt, SltOutput, SltPlugin};

fn main() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, SltPlugin))
        .insert_slt_ui(|ui| {
            ui.text("hello from slt inside bevy");
        });

    app.update();

    let output = app.world().resource::<SltOutput>();
    println!("{}", output.text);
}
