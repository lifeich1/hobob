use super::*;
use bevy::{
    app::AppExit,
    input::{
        keyboard::{KeyCode, KeyboardInput},
        mouse::{MouseScrollUnit, MouseWheel},
        ElementState,
    },
    tasks::{Task, TaskPool, TaskPoolBuilder},
};
use bilibili_api_rs::plugin::{ApiRequestEvent, ApiTaskResultEvent};
use chrono::naive::NaiveDateTime;
use clipboard::{ClipboardContext, ClipboardProvider};
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
            .add_system(button_add_following.system())
            .add_system(show_face.system())
            .add_system(download_face.system())
            .add_system(video_info_api_result.system())
            .add_system(live_info_api_result.system())
            .add_system(first_parse_api_result.system())
            .add_system(input.system())
            .add_system(show_scroll_progression.system())
            .add_system(handle_actions.system())
            .add_system(button_refresh.system())
            .add_system(nickname_api_result.system());
    }
}

fn input(
    mut keyboard_ev: EventReader<KeyboardInput>,
    mut mousewheel: EventReader<MouseWheel>,
    mut exit_ev: EventWriter<AppExit>,
    keyboard: Res<Input<KeyCode>>,
    mut scroll_widget_query: Query<&mut scroll::ScrollSimListWidget>,
    mut adding_following_query: Query<&mut Text, With<ui::add::AddFollowing>>,
) {
    let mut scroll_move: i32 = 0;
    let mut text_edit = Vec::<KeyCode>::new();
    for ev in keyboard_ev.iter() {
        match ev {
            KeyboardInput {
                key_code: Some(KeyCode::Escape),
                state: ElementState::Released,
                ..
            } => {
                info!("key ESC released");
                exit_ev.send(AppExit {});
            }
            KeyboardInput {
                key_code: Some(k @ (KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right)),
                state: ElementState::Released,
                ..
            } => {
                scroll_move = match k {
                    KeyCode::Up => -1,
                    KeyCode::Down => 1,
                    KeyCode::Left => -4,
                    KeyCode::Right => 4,
                    _ => panic!("match scroll_move at unexpected key: {:?}", k),
                };
            }
            KeyboardInput {
                key_code:
                    Some(
                        k
                        @
                        (KeyCode::Key0
                        | KeyCode::Key1
                        | KeyCode::Key2
                        | KeyCode::Key3
                        | KeyCode::Key4
                        | KeyCode::Key5
                        | KeyCode::Key6
                        | KeyCode::Key7
                        | KeyCode::Key8
                        | KeyCode::Key9
                        | KeyCode::Back
                        | KeyCode::Paste),
                    ),
                state: ElementState::Pressed,
                ..
            } => {
                text_edit.push(*k);
            }
            _ => (),
        }
    }

    if keyboard.pressed(KeyCode::LControl) && keyboard.just_pressed(KeyCode::V) {
        text_edit.push(KeyCode::Paste);
    }

    if scroll_move == 0 {
        for ev in mousewheel.iter() {
            if let MouseWheel {
                unit: MouseScrollUnit::Line,
                x: _,
                y,
            } = ev
            {
                if y.abs() > f32::EPSILON {
                    scroll_move -= (y.abs().ceil() * y.signum()) as i32;
                }
            }
        }
    }

    if scroll_move != 0 {
        for mut widget in scroll_widget_query.iter_mut() {
            widget.scroll_to(scroll_move);
        }
    }

    if !text_edit.is_empty() {
        for mut text in adding_following_query.iter_mut() {
            let v = &mut text.sections[0].value;
            for k in text_edit.iter() {
                match k {
                    KeyCode::Key0 => v.push('0'),
                    KeyCode::Key1 => v.push('1'),
                    KeyCode::Key2 => v.push('2'),
                    KeyCode::Key3 => v.push('3'),
                    KeyCode::Key4 => v.push('4'),
                    KeyCode::Key5 => v.push('5'),
                    KeyCode::Key6 => v.push('6'),
                    KeyCode::Key7 => v.push('7'),
                    KeyCode::Key8 => v.push('8'),
                    KeyCode::Key9 => v.push('9'),
                    KeyCode::Back => {
                        v.pop();
                    }
                    KeyCode::Paste => match try_get_pasted() {
                        Ok(s) => v.push_str(s.as_str()),
                        Err(e) => error!("get content from clipboard error: {}", e),
                    },
                    _ => panic!("match text edit op at unexpected key: {:?}", k),
                }
            }
        }
    }
}

fn try_get_pasted() -> Result<String, Box<dyn std::error::Error>> {
    ClipboardContext::new()?.get_contents()
}

fn show_scroll_progression(
    mut show_scroll_progression_query: Query<&mut Text, With<ShowScrollProgression>>,
    changed_scroll_progression_query: Query<
        &scroll::ScrollProgression,
        Changed<scroll::ScrollProgression>,
    >,
) {
    for p in changed_scroll_progression_query.iter() {
        for mut text in show_scroll_progression_query.iter_mut() {
            text.sections[0].value = format!("{}%", p.0);
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
            ui::following::event::Action::AddFollowingUid(uid) => {
                add_following(*uid, &mut cf, &app_res, &mut commands, &mut scroll_widget_query)
            }
        }
    }
}

fn add_following(
    uid: u64,
    cf: &mut ResMut<AppConfig>,
    app_res: &Res<AppResource>,
    commands: &mut Commands,
    scroll_widget_query: &mut Query<(Entity, &mut scroll::ScrollSimListWidget)>,
) {
    if !cf.add_following(uid) {
        info!("already following {}", uid);
        return;
    }
    for (entity, mut scroll_widget) in scroll_widget_query.iter_mut() {
        let widget = widget::create_following(commands, app_res, uid);
        commands.entity(entity).insert_children(0, &[widget]);
        scroll_widget.invalidate().scroll_to(0);
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

#[allow(clippy::type_complexity)]
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

#[allow(clippy::type_complexity)]
fn button_add_following(
    app_res: Res<AppResource>,
    mut interaction_query: Query<
        (&Interaction, &mut Handle<ColorMaterial>),
        (
            With<Button>,
            Changed<Interaction>,
            With<ui::add::AddFollowingButton>,
        ),
    >,
    add_query: Query<&Text, With<ui::add::AddFollowing>>,
    mut action_chan: EventWriter<ui::following::event::Action>,
) {
    for (interaction, mut material) in interaction_query.iter_mut() {
        match *interaction {
            Interaction::Clicked => {
                let mut uid: Option<u64> = None;
                for add in add_query.iter() {
                    if !add.sections.is_empty() {
                        uid = add.sections[0].value.parse::<u64>().ok();
                        if uid.is_some() {
                            break;
                        }
                    }
                }
                match uid {
                    Some(id) => {
                        info!("button add following trigger: {}", id);
                        action_chan.send(ui::following::event::Action::AddFollowingUid(id));
                    }
                    None => info!("parse input error: button add following"),
                }
                *material = app_res.btn_press_col.clone();
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

#[allow(clippy::type_complexity)]
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
        if url.is_empty() {
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
