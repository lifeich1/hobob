use bevy::{
    prelude::*,
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
};
use bilibili_api_rs::{api, plugin::ApiRuntimePlugin};
use hobob_bevy_widget::scroll;
use lazy_static::lazy_static;
use std::path::{Path, PathBuf};
use tokio::runtime;

lazy_static! {
    static ref CACHE_DIR: Box<Path> = Path::new(".cache").into();
    static ref FOLLOWING_CACHE: PathBuf = CACHE_DIR.join("followings.ron");
    static ref FACE_CACHE_DIR: PathBuf = CACHE_DIR.join("face");
}

mod logic;
mod startup;
pub mod ui;
mod widget;

pub struct HobobPlugin {}

impl HobobPlugin {
    fn startup_error<T: ToString + std::fmt::Display>(err: T) -> (AppContext, AppConfig) {
        error!("STARTUP ERROR: {}", err);
        (
            AppContext::default(),
            AppConfig {
                startup_error: Some(err.to_string()),
                ..Default::default()
            },
        )
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
        if let Err(e) = std::fs::DirBuilder::new()
            .recursive(true)
            .create(&*FACE_CACHE_DIR)
        {
            return Self::startup_error(e);
        }

        let mut cf = AppConfig::default();
        match load_cache(&*FOLLOWING_CACHE) {
            Ok(r) => cf.followings_uid = r,
            Err(e) => {
                warn!("open {:?} error: {}", *FOLLOWING_CACHE, e);
                if let Err(e) = commit_cache(&*FOLLOWING_CACHE, &cf.followings_uid) {
                    error!("commit cache to {:?} error: {}", *FOLLOWING_CACHE, e);
                }
            }
        }
        info!("init followings: {:?}", cf.followings_uid);
        (
            AppContext {
                rt: Some(rt.unwrap()),
                api_ctx: Some(api_ctx.unwrap()),
            },
            cf,
        )
    }
}

impl bevy::prelude::Plugin for HobobPlugin {
    fn build(&self, app: &mut AppBuilder) {
        //simple_logger::SimpleLogger::new().init().unwrap();

        let (ctx, cf) = Self::setup();
        app.init_resource::<AppResource>()
            .add_plugin(FrameTimeDiagnosticsPlugin::default())
            .add_plugin(LogDiagnosticsPlugin::default())
            .insert_resource(cf)
            .add_plugin(ApiRuntimePlugin::new(
                ctx.api_ctx.as_ref().unwrap(),
                ctx.rt.as_ref().unwrap(),
            ))
            .insert_resource(ctx)
            .add_plugin(scroll::ScrollWidgetsPlugin())
            .add_plugin(ui::ResourcePlugin())
            .add_plugin(logic::LogicPlugin())
            .add_startup_system(startup::ui.system());
    }
}

#[derive(Default)]
pub struct AppContext {
    rt: Option<runtime::Runtime>,
    api_ctx: Option<api::Context>,
}

#[derive(Clone)]
pub struct AppConfig {
    startup_error: Option<String>,
    followings_uid: Vec<u64>,
    face_cache_dir: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            startup_error: None,
            followings_uid: vec![15810, 10592068],
            face_cache_dir: FACE_CACHE_DIR.to_str().unwrap().to_string(),
        }
    }
}

pub struct AppResource {
    err_text_col: Color,
    progression_text_col: Color,

    none_col: Handle<ColorMaterial>,
    bg_col: Handle<ColorMaterial>,
    item_bg_col: Handle<ColorMaterial>,
    item_to_jump_bg_col: Handle<ColorMaterial>,

    face_none_img: Handle<ColorMaterial>,

    btn_press_col: Handle<ColorMaterial>,
    btn_hover_col: Handle<ColorMaterial>,
    btn_none_col: Handle<ColorMaterial>,

    btn_text_col: Color,

    live_on_text: String,
    live_off_text: String,
    no_video_text: String,

    font: Handle<Font>,
    progression_font_size: f32,
}

impl FromWorld for AppResource {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.get_resource::<AssetServer>().unwrap();
        let mut font: Handle<Font> = asset_server.load("fonts/FiraMono-Medium.ttf");

        let mut live_on_text = String::from("LIVE ON");
        let mut live_off_text = String::from("LIVE OFF");
        let mut no_video_text = String::from("no videos");

        if std::path::Path::new("assets/fonts/SourceHanSans-Bold.otf").is_file() {
            info!("use SourceHanSans-Bold");
            font = asset_server.load("fonts/SourceHanSans-Bold.otf");

            live_on_text = String::from("直播中");
            live_off_text = String::from("未直播");
            no_video_text = String::from("无上传视频");
        }

        let mut materials = world.get_resource_mut::<Assets<ColorMaterial>>().unwrap();
        Self {
            err_text_col: Color::RED,
            progression_text_col: Color::YELLOW,
            none_col: materials.add(Color::NONE.into()),
            bg_col: materials.add(Color::hex("90d7ec").unwrap().into()),
            item_bg_col: materials.add(Color::hex("7bbfea").unwrap().into()),
            item_to_jump_bg_col: materials.add(Color::WHITE.into()),
            btn_press_col: materials.add(Color::hex("2e3a1f").unwrap().into()),
            btn_hover_col: materials.add(Color::hex("726930").unwrap().into()),
            btn_none_col: materials.add(Color::hex("87843b").unwrap().into()),
            btn_text_col: Color::hex("181d4b").unwrap(),
            face_none_img: materials.add(Color::WHITE.into()),
            progression_font_size: 25.,
            font,
            live_on_text,
            live_off_text,
            no_video_text,
        }
    }
}

pub struct ShowScrollProgression {}

fn load_cache<P: AsRef<Path>, T: serde::de::DeserializeOwned>(
    p: P,
) -> Result<T, Box<dyn std::error::Error>> {
    Ok(ron::de::from_reader::<_, T>(std::fs::File::open(p)?)?)
}

fn commit_cache<P: AsRef<Path>, T: serde::ser::Serialize>(
    p: P,
    value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(dir) = p.as_ref().parent() {
        std::fs::DirBuilder::new().recursive(true).create(dir)?;
    }
    ron::ser::to_writer(std::fs::File::create(p)?, value)?;
    Ok(())
}
