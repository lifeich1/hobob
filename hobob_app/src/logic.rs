use super::*;
use bevy::{
    tasks::{Task, TaskPool, TaskPoolBuilder},
    utils::Duration,
};
use bilibili_api_rs::plugin::{ApiRequestEvent, ApiTaskResultEvent};
use chrono::naive::NaiveDateTime;
use futures_lite::future;
use hobob_bevy_widget::scroll;
use serde_json::json;
use std::{collections::VecDeque, ops::Deref};
use ui::following::{
    data::{self, Data},
    event::ParsedApiResult,
};

mod frontend;

pub struct LogicPlugin();

impl Plugin for LogicPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_plugin(frontend::input::ModPlugin())
            .add_plugin(frontend::display::ModPlugin())
            // backend
            .add_system(handle_actions.system())
            .insert_resource(AutoRefreshTimer::default())
            .add_system(periodly_refresh_all.system())
            // backend::parser
            .add_system(first_parse_api_result.system())
            .add_system(sort_key_api_result.system())
            // backend::face
            .init_resource::<FaceTaskPool>()
            .add_system(show_face.system())
            .add_system(download_face.system());
    }
}

struct AutoRefreshTimer {
    timer: Timer,
    queue: VecDeque<u64>,
}

impl AutoRefreshTimer {
    fn refill(&mut self, cf: &Res<AppConfig>) -> &mut Self {
        if self.queue.is_empty() {
            self.queue.extend(cf.followings_uid.clone());
        }
        self
    }

    fn drain(&mut self, max_size: usize) -> std::collections::vec_deque::Drain<u64> {
        self.queue.drain(..self.queue.len().min(max_size))
    }
}

impl Default for AutoRefreshTimer {
    fn default() -> Self {
        let mut timer = Timer::from_seconds(30., true);
        timer.tick(
            timer
                .duration()
                .checked_sub(Duration::from_millis(100))
                .expect("there must be a pretty large refresh timer"),
        );
        Self {
            timer,
            queue: Default::default(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_actions(
    mut action_chan: EventReader<ui::following::event::Action>,
    mut api_req_chan: EventWriter<ApiRequestEvent>,
    api_ctx: Res<api::Context>,
    visible_nickname_query: Query<(&ui::following::Nickname, &Visible)>,
    mut cf: ResMut<AppConfig>,
    app_res: Res<AppResource>,
    mut commands: Commands,
    mut scroll_widget_query: Query<(Entity, &mut scroll::ScrollSimListWidget)>,
) {
    for action in action_chan.iter() {
        match action {
            ui::following::event::Action::RefreshVisible => {
                refresh_visible(&mut api_req_chan, &api_ctx, &visible_nickname_query)
            }
            ui::following::event::Action::AddFollowingUid(uid) => add_following(
                *uid,
                &mut cf,
                &app_res,
                &mut commands,
                &mut scroll_widget_query,
                &mut api_req_chan,
                &api_ctx,
            ),
        }
    }
}

fn add_following(
    uid: u64,
    cf: &mut ResMut<AppConfig>,
    app_res: &Res<AppResource>,
    commands: &mut Commands,
    scroll_widget_query: &mut Query<(Entity, &mut scroll::ScrollSimListWidget)>,
    api_req_chan: &mut EventWriter<ApiRequestEvent>,
    api_ctx: &Res<api::Context>,
) {
    if !cf.add_following(uid) {
        info!("already following {}", uid);
        return;
    }
    for (entity, mut scroll_widget) in scroll_widget_query.iter_mut() {
        let widget = widget::create_following(commands, app_res, uid);
        commands.entity(entity).insert_children(0, &[widget]);
        scroll_widget.scroll_to_top();
        refresh_user_info(api_req_chan, api_ctx, uid);
    }
}

fn refresh_visible(
    api_req_chan: &mut EventWriter<ApiRequestEvent>,
    api_ctx: &Res<api::Context>,
    visible_nickname_query: &Query<(&ui::following::Nickname, &Visible)>,
) {
    for (nickname, visible) in visible_nickname_query.iter() {
        if visible.is_visible {
            refresh_user_info(api_req_chan, api_ctx, nickname.0);
        }
    }
}

fn periodly_refresh_all(
    time: Res<Time>,
    mut timer: ResMut<AutoRefreshTimer>,
    mut api_req_chan: EventWriter<ApiRequestEvent>,
    api_ctx: Res<api::Context>,
    cf: Res<AppConfig>,
) {
    if timer.timer.tick(time.delta()).just_finished() {
        info!("refresh a batch of userinfo");
        for uid in timer.refill(&cf).drain(cf.refresh_batch_size) {
            refresh_user_info(&mut api_req_chan, &api_ctx, uid);
        }
    }
}

fn refresh_user_info(
    api_req_chan: &mut EventWriter<ApiRequestEvent>,
    api_ctx: &Res<api::Context>,
    uid: u64,
) {
    info!("refresh userinfo of {}", uid);
    api_req_chan.send(ApiRequestEvent {
        req: api_ctx.new_user(uid).get_info(),
        tag: json!({"uid": uid, "cmd": "refresh"}).into(),
    });
    api_req_chan.send(ApiRequestEvent {
        req: api_ctx.new_user(uid).video_list(1),
        tag: json!({"uid": uid, "cmd": "new-video"}).into(),
    });
}

fn first_parse_api_result(
    mut raw_result: EventReader<ApiTaskResultEvent>,
    mut parsed: EventWriter<ParsedApiResult>,
    app_res: Res<AppResource>,
) {
    for ev in raw_result.iter() {
        let resp = match ev.result.as_ref() {
            Ok(r) => r,
            Err(e) => {
                error!("api error: {}", e);
                continue;
            }
        };
        let uid = match ev.tag["uid"].as_u64() {
            Some(u) => u,
            None => {
                debug!("result without uid: {:?}", ev);
                continue;
            }
        };
        let cmd = match ev.tag["cmd"].as_str() {
            Some(s) => s,
            None => {
                debug!("result without cmd: {:?}", ev);
                continue;
            }
        };
        match cmd {
            "refresh" => parsed.send(ParsedApiResult {
                uid,
                data: Data::Info(data::Info {
                    nickname: resp["name"].as_str().unwrap_or_default().to_string(),
                    live_room_url: resp["live_room"]["url"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    live_room_title: resp["live_room"]["title"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    live_open: resp["live_room"]["liveStatus"].as_i64().map(|x| x != 0),
                    live_entropy: resp["live_room"]["online"].as_u64().unwrap_or_default(),
                    face_url: resp["face"].as_str().unwrap_or_default().to_string(),
                }),
            }),
            "new-video" => {
                if let Some(list) = resp["list"]["vlist"].as_array() {
                    let v = list.iter().reduce(|a, b| {
                        let t1 = a["created"].as_i64().unwrap_or_default();
                        let t2 = b["created"].as_i64().unwrap_or_default();
                        if t1 > t2 {
                            a
                        } else {
                            b
                        }
                    });
                    parsed.send(ParsedApiResult {
                        uid,
                        data: Data::NewVideo(match v {
                            Some(vid) => data::NewVideo {
                                title: vid["title"].as_str().unwrap_or("N/A").to_string(),
                                date_time: NaiveDateTime::from_timestamp(
                                    vid["created"].as_i64().unwrap_or_default(),
                                    0,
                                )
                                .format("%Y-%m-%d %H:%M ")
                                .to_string(),
                                timestamp_sec: vid["created"].as_u64().unwrap_or_default(),
                            },
                            None => data::NewVideo {
                                title: app_res.no_video_text.clone(),
                                date_time: Default::default(),
                                timestamp_sec: Default::default(),
                            },
                        }),
                    });
                }
            }
            _ => error!("result with unimplemented cmd: {}", cmd),
        }
    }
}

fn sort_key_api_result(
    mut sort_key_query: Query<(&mut ui::following::data::SortKey, &ui::following::data::Uid)>,
    mut result_chan: EventReader<ParsedApiResult>,
) {
    for ParsedApiResult { uid, data } in result_chan.iter() {
        if let Data::Face(_) = data {
            continue;
        }
        if let Some((mut key, _)) = sort_key_query.iter_mut().find(|(_, id)| id.0 == *uid) {
            match data {
                Data::Info(info) => {
                    key.live_entropy = if let Some(true) = info.live_open {
                        info.live_entropy
                    } else {
                        0
                    }
                }
                Data::NewVideo(vid) => key.video_pub_ts = vid.timestamp_sec,
                _ => panic!("unimplement handler of {:?}", data),
            }
        }
    }
}

struct DownloadFace(u64, Option<std::path::PathBuf>, Option<String>);

pub struct FaceTaskPool(TaskPool);

impl FromWorld for FaceTaskPool {
    fn from_world(_world: &mut World) -> Self {
        Self(
            TaskPoolBuilder::new()
                .thread_name("face".to_string())
                .build(),
        )
    }
}

impl Deref for FaceTaskPool {
    type Target = TaskPool;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[tokio::main]
async fn do_download<T: AsRef<Path>>(url: &str, p: T) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = reqwest::get(url).await?.bytes().await?;
    debug!("downloaded {}", url);
    let t = image::io::Reader::new(std::io::Cursor::new(bytes));
    debug!("read bytes {}", url);
    let t = t.with_guessed_format()?;
    debug!("guess format {}", url);
    let t = t.decode()?;
    debug!("decoded {}", url);
    let t = t.thumbnail(256, 256);
    debug!("thumbnailed {}", url);
    t.save(p)?;
    debug!("saved {}", url);

    Ok(())
}

fn download_face(
    mut commands: Commands,
    task_pool: Res<FaceTaskPool>,
    mut result_chan: EventReader<ParsedApiResult>,
    cf: Res<AppConfig>,
) {
    for ParsedApiResult { uid, data } in result_chan.iter() {
        if let Data::Info(info) = data {
            if !info.face_url.is_empty() {
                let id = *uid;
                let url = info.face_url.clone();
                let dir = cf.face_cache_dir.clone();
                let task = task_pool.spawn(async move {
                    let filename = &url[url.rfind('/').map(|x| x + 1).unwrap_or(0)..];
                    let p = Path::new(&dir).join(filename);
                    if !p.is_file() {
                        if let Err(e) = do_download(&url, &p) {
                            error!("download {} to {:?} error: {}", url, p, e);
                            return DownloadFace(id, None, Some(e.to_string()));
                        }
                    }
                    DownloadFace(id, Some(p), None)
                });
                commands.spawn().insert(task);
            }
        }
    }
}

fn show_face(
    mut commands: Commands,
    mut tasks_query: Query<(Entity, &mut Task<DownloadFace>)>,
    mut face_query: Query<(Entity, &mut Handle<ColorMaterial>, &ui::following::Face)>,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    for (entity, mut task) in tasks_query.iter_mut() {
        if let Some(result) = future::block_on(future::poll_once(&mut *task)) {
            match result.1 {
                Some(path) => {
                    for (entity, mut material, face) in face_query.iter_mut() {
                        if face.0 != result.0 {
                            continue;
                        }

                        *material = materials.add(asset_server.load(path).into());
                        commands.entity(entity).remove::<ui::following::Face>();
                        break;
                    }
                }
                None => error!(
                    "pull face: {}",
                    result.2.expect("should return error description")
                ), // TODO alert in ui
            }
            commands.entity(entity).despawn();
        }
    }
}
