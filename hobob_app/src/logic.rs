use super::*;
use bevy::{
    app::AppExit,
    input::{
        keyboard::{KeyCode, KeyboardInput},
        ElementState,
    },
    tasks::{Task, TaskPool, TaskPoolBuilder},
};
use bilibili_api_rs::plugin::{ApiRequestEvent, ApiTaskResultEvent};
use chrono::naive::NaiveDateTime;
use futures_lite::future;
use hobob_bevy_widget::scroll;
use serde_json::json;
use std::ops::Deref;
use ui::following::{
    data::{self, Data},
    event::ParsedApiResult,
};

pub struct LogicPlugin();

impl Plugin for LogicPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_system(jump_button_system.system())
            .init_resource::<FaceTaskPool>()
            .add_system(show_face.system())
            .add_system(download_face.system())
            .add_system(video_info_api_result.system())
            .add_system(live_info_api_result.system())
            .add_system(first_parse_api_result.system())
            .add_system(ui.system())
            .add_system(handle_actions.system())
            .add_system(button_refresh.system())
            .add_system(nickname_api_result.system());
    }
}

fn ui(
    mut _commands: Commands,
    mut keyboard_ev: EventReader<KeyboardInput>,
    mut exit_ev: EventWriter<AppExit>,
    mut show_scroll_progression_query: Query<&mut Text, With<ShowScrollProgression>>,
    changed_scroll_progression_query: Query<
        &scroll::ScrollProgression,
        Changed<scroll::ScrollProgression>,
    >,
) {
    for ev in keyboard_ev.iter() {
        match ev {
            KeyboardInput {
                scan_code: _,
                key_code: Some(KeyCode::Escape),
                state: ElementState::Released,
            } => {
                info!("key ESC released");
                exit_ev.send(AppExit {});
            }
            _ => (),
        }
    }

    for p in changed_scroll_progression_query.iter() {
        for mut text in show_scroll_progression_query.iter_mut() {
            text.sections[0].value = format!("{}%", p.0);
        }
    }
}

fn handle_actions(
    mut action_chan: EventReader<ui::following::event::Action>,
    mut api_req_chan: EventWriter<ApiRequestEvent>,
    api_ctx: Res<api::Context>,
    visible_nickname_query: Query<(&ui::following::Nickname, &Visible)>,
) {
    for action in action_chan.iter() {
        match action {
            ui::following::event::Action::RefreshVisible => {
                refresh_visible(&mut api_req_chan, &api_ctx, &visible_nickname_query)
            }
            _ => error!("trigger not implemented action {:?}", action),
        }
    }
}

fn refresh_visible(
    api_req_chan: &mut EventWriter<ApiRequestEvent>,
    api_ctx: &Res<api::Context>,
    visible_nickname_query: &Query<(&ui::following::Nickname, &Visible)>,
) {
    for (nickname, visible) in visible_nickname_query.iter() {
        if visible.is_visible {
            let uid: u64 = nickname.0;
            api_req_chan.send(ApiRequestEvent {
                req: api_ctx.new_user(uid).get_info(),
                tag: json!({"uid": uid, "cmd": "refresh"}).into(),
            });
            api_req_chan.send(ApiRequestEvent {
                req: api_ctx.new_user(uid).video_list(1),
                tag: json!({"uid": uid, "cmd": "new-video"}).into(),
            });
        }
    }
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
                            },
                            None => data::NewVideo {
                                title: app_res.no_video_text.clone(),
                                date_time: Default::default(),
                            },
                        }),
                    });
                }
            }
            _ => error!("result with unimplemented cmd: {}", cmd),
        }
    }
}

fn nickname_api_result(
    mut nickname_query: Query<(&mut Text, &ui::following::Nickname)>,
    mut result_chan: EventReader<ParsedApiResult>,
) {
    for ParsedApiResult { uid, data } in result_chan.iter() {
        if let Data::Info(info) = data {
            for (mut text, nickname) in nickname_query.iter_mut() {
                if nickname.0 != *uid {
                    continue;
                }
                text.sections[0].value = info.nickname.clone();
                break;
            }
        }
    }
}

fn live_info_api_result(
    mut livetitle_query: Query<(&mut Text, &ui::following::LiveRoomTitle)>,
    mut livebutton_query: Query<&mut ui::following::LiveRoomOpenButton>,
    mut result_chan: EventReader<ParsedApiResult>,
    app_res: Res<AppResource>,
) {
    for ParsedApiResult { uid, data } in result_chan.iter() {
        if let Data::Info(info) = data {
            if matches!(info.live_open, None) {
                continue;
            }
            for mut button in livebutton_query.iter_mut() {
                if button.0 != *uid {
                    continue;
                }
                button.1 = info.live_room_url.clone();
                break;
            }
            for (mut text, livetitle) in livetitle_query.iter_mut() {
                if livetitle.0 != *uid {
                    continue;
                }
                if text.sections.len() != 3 {
                    text.sections = vec![
                        TextSection {
                            value: "".to_string(),
                            style: TextStyle {
                                font: app_res.font.clone(),
                                font_size: 15.0,
                                color: Color::WHITE,
                            },
                        },
                        TextSection {
                            value: "".to_string(),
                            style: TextStyle {
                                font: app_res.font.clone(),
                                font_size: 10.0,
                                color: Color::RED,
                            },
                        },
                        TextSection {
                            value: "".to_string(),
                            style: TextStyle {
                                font: app_res.font.clone(),
                                font_size: 15.0,
                                color: Color::BLUE,
                            },
                        },
                    ];
                }
                if let Some(true) = info.live_open {
                    text.sections[0].value = app_res.live_on_text.clone();
                    text.sections[0].style.color = Color::BLUE;
                    text.sections[1].value = info.live_entropy.to_string();
                    text.sections[1].style.color = Color::RED;
                } else {
                    text.sections[0].value = app_res.live_off_text.clone();
                    text.sections[0].style.color = Color::GRAY;
                    text.sections[1].value = info.live_entropy.to_string();
                    text.sections[1].style.color = Color::GRAY;
                }
                text.sections[2].value = info.live_room_title.clone();
                break;
            }
        }
    }
}

fn video_info_api_result(
    mut videoinfo_query: Query<(&mut Text, &ui::following::VideoInfo)>,
    mut result_chan: EventReader<ParsedApiResult>,
    app_res: Res<AppResource>,
) {
    for ParsedApiResult { uid, data } in result_chan.iter() {
        if let Data::NewVideo(info) = data {
            for (mut text, videoinfo) in videoinfo_query.iter_mut() {
                if videoinfo.0 != *uid {
                    continue;
                }

                if text.sections.len() != 2 {
                    text.sections = vec![
                        TextSection {
                            value: "".to_string(),
                            style: TextStyle {
                                font: app_res.font.clone(),
                                font_size: 15.0,
                                color: Color::GRAY,
                            },
                        },
                        TextSection {
                            value: "".to_string(),
                            style: TextStyle {
                                font: app_res.font.clone(),
                                font_size: 15.0,
                                color: Color::BLACK,
                            },
                        },
                    ];
                }
                text.sections[0].value = info.date_time.clone();
                text.sections[1].value = info.title.clone();
            }
        }
    }
}

fn button_refresh(
    app_res: Res<AppResource>,
    mut interaction_query: Query<
        (&Interaction, &mut Handle<ColorMaterial>),
        (
            With<Button>,
            Changed<Interaction>,
            With<ui::add::RefreshVisible>,
        ),
    >,
    mut action_chan: EventWriter<ui::following::event::Action>,
) {
    for (interaction, mut material) in interaction_query.iter_mut() {
        match *interaction {
            Interaction::Clicked => {
                info!("button refresh trigger!");
                *material = app_res.btn_press_col.clone();
                action_chan.send(ui::following::event::Action::RefreshVisible);
            }
            Interaction::Hovered => {
                *material = app_res.btn_hover_col.clone();
            }
            Interaction::None => {
                *material = app_res.btn_none_col.clone();
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
            if info.face_url.len() > 0 {
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
                None => (), // TODO alert
            }
            commands.entity(entity).despawn();
        }
    }
}

fn jump_button_system(
    app_res: Res<AppResource>,
    button_query: Query<
        (
            &Interaction,
            &ui::following::HoverPressShow,
            Option<&ui::following::HomepageOpenButton>,
            Option<&ui::following::LiveRoomOpenButton>,
        ),
        (Changed<Interaction>, With<Button>),
    >,
    mut shower_query: Query<(&ui::following::HoverPressShower, &mut Handle<ColorMaterial>)>,
) {
    for (interaction, show, opt_home, opt_live) in button_query.iter() {
        let (uid, url) = match (opt_home, opt_live) {
            (Some(home), None) => (home.0, format!("https://space.bilibili.com/{}/", home.0)),
            (None, Some(live)) => (live.0, live.1.to_string()),
            _ => panic!(
                "HoverPressShow widget invalid status: {:?} {:?}",
                opt_home, opt_live
            ),
        };
        if url.len() == 0 {
            continue;
        }
        let entity = show.0;
        let shower = shower_query
            .get_component::<ui::following::HoverPressShower>(entity)
            .expect("entity in shower_query must have component HoverPressShower");
        if shower.0 != uid {
            panic!("HoverPressShow(er) uid mismatch: {} {}", shower.0, uid);
        }

        match interaction {
            Interaction::Clicked => {
                let open_cmd = if cfg!(target_os = "linux") {
                    "xdg-open"
                } else {
                    "open"
                };
                let start = std::process::Command::new(open_cmd).arg(&url).spawn();
                match start {
                    Ok(_) => info!("open url ok: {}", url),
                    Err(e) => error!("open url error: {}", e),
                }
            }
            Interaction::Hovered | Interaction::None => {
                let mut material = shower_query
                    .get_component_mut::<Handle<ColorMaterial>>(entity)
                    .expect("entity in shower_query must have component Handle<ColorMaterial>");
                *material = if let Interaction::None = interaction {
                    app_res.item_bg_col.clone()
                } else {
                    app_res.item_to_jump_bg_col.clone()
                };
            }
        }
    }
}
