use bevy::prelude::*;
use bevy_slt::{SltHeadlessContext, SltHeadlessPlugin, SltOutput};

fn main() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, SltHeadlessPlugin::default()))
        .add_systems(Update, draw);

    app.update();

    let output = app.world().resource::<SltOutput>();
    println!("{}", output.text);
}

fn draw(mut context: NonSendMut<SltHeadlessContext>) -> Result {
    context.draw(|ui| {
        ui.text("hello from slt inside bevy");
    })?;
    Ok(())
}
