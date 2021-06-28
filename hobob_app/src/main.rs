use bevy::prelude::*;
use hobob_app::HobobPlugin;

fn main() {
    App::build()
        .add_plugins(DefaultPlugins)
        .add_plugin(HobobPlugin {})
        .run()
}
