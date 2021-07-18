use bevy::prelude::*;

mod backend;
mod frontend;

pub struct LogicPlugin();

impl Plugin for LogicPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_plugin(frontend::input::ModPlugin())
            .add_plugin(frontend::display::ModPlugin())
            .add_plugin(backend::ModPlugin());
    }
}
