use bevy::{
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    prelude::*,
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
mod resource;
mod startup;
pub mod ui;
mod widget;

pub use resource::AppResource;

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
    refresh_batch_size: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            startup_error: None,
            followings_uid: vec![15810, 10592068],
            face_cache_dir: FACE_CACHE_DIR.to_str().unwrap().to_string(),
            refresh_batch_size: 30,
        }
    }
}

impl AppConfig {
    fn add_following(&mut self, uid: u64) -> bool {
        if self.followings_uid.iter().any(|x| *x == uid) {
            return false;
        }
        self.followings_uid.insert(0, uid);
        if let Err(e) = commit_cache(&*FOLLOWING_CACHE, &self.followings_uid) {
            error!("commit cache to {:?} error: {}", *FOLLOWING_CACHE, e);
        }
        true
    }
}

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
