use bevy::prelude::*;
use hobob_bevy_widget::scroll;
use bilibili_api_rs::{
    api,
    plugin::ApiRuntimePlugin,
};
use tokio::runtime;

mod startup;

pub struct HobobPlugin{}


impl HobobPlugin {
    fn startup_error<T: ToString + std::fmt::Display>(err: T) -> (AppContext, AppConfig) {
        error!("STARTUP ERROR: {}", err);
        (AppContext::default(), AppConfig{
            startup_error: Some(err.to_string()),
            ..Default::default()
        })
    }

    fn setup() -> (AppContext, AppConfig) {
        let rt = runtime::Runtime::new();
        if let Err(e) = rt {
            return Self::startup_error(e);
        }

        let api_ctx = api::Context::new();
        if let Err(e) = api_ctx {
            return Self::startup_error(e);
        }

        let cf = AppConfig::default();
        (AppContext{
            rt: Some(rt.unwrap()),
            api_ctx: Some(api_ctx.unwrap()),
        }, cf)
    }
}

impl bevy::prelude::Plugin for HobobPlugin {
    fn build(&self, app: &mut AppBuilder) {
        let (ctx, cf) = Self::setup();
        app.init_resource::<AppResource>()
            .insert_resource(cf.clone())
            .add_plugin(ApiRuntimePlugin::new(
                    ctx.api_ctx.as_ref().unwrap(),
                    ctx.rt.as_ref().unwrap()))
            .insert_resource(ctx)
            .add_plugin(scroll::ScrollWidgetsPlugin())
            .add_startup_system(startup::ui.system())
            ;
    }
}

#[derive(Default)]
pub struct AppContext {
    rt: Option<runtime::Runtime>,
    api_ctx: Option<api::Context>,
}

#[derive(Default, Clone)]
pub struct AppConfig {
    startup_error: Option<String>,
}

pub struct AppResource {
    err_text_col: Color,
    progression_text_col: Color,

    bg_col: Handle<ColorMaterial>,
    item_bg_col: Handle<ColorMaterial>,

    font: Handle<Font>,
}

impl FromWorld for AppResource {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.get_resource::<AssetServer>().unwrap();
        let font: Handle<Font> = asset_server.load("fonts/FiraMono-Medium.ttf");

        let mut materials = world.get_resource_mut::<Assets<ColorMaterial>>().unwrap();
        Self {
            err_text_col: Color::RED,
            progression_text_col: Color::YELLOW,
            bg_col: materials.add(Color::hex("90d7ec").unwrap().into()),
            item_bg_col: materials.add(Color::hex("7bbfea").unwrap().into()),
            font,
        }
    }
}
